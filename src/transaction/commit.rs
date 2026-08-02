use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    journal::{FinishOutcome, JournalRecord},
    report::{AbortReport, ApplyReport},
};

use super::{
    PreparedTransaction,
    rollback::rollback_committed,
    stage::cleanup_staged,
    util::{fs_error, journal_error},
    verify::{verify_artifact, verify_target},
};

enum CommitFailureState {
    Unchanged,
    Changed,
    Ambiguous,
}

impl PreparedTransaction {
    /// Revalidates and commits every staged target in deterministic path order.
    pub fn commit(mut self) -> Result<ApplyReport, WorktreeError> {
        let mut committed = Vec::with_capacity(self.files.len());
        if let Err(error) = verify_all_backups(&self) {
            return rollback_committed(&mut self, &committed).map_or_else(Err, |()| Err(error));
        }
        for index in 0..self.files.len() {
            if let Err((error, state)) = commit_one(&mut self, index) {
                match state {
                    CommitFailureState::Changed => committed.push(index),
                    CommitFailureState::Unchanged => {}
                    CommitFailureState::Ambiguous => return Err(error),
                }
                return rollback_committed(&mut self, &committed).map_or_else(Err, |()| Err(error));
            }
            committed.push(index);
        }
        self.journal
            .append(&JournalRecord::Finished {
                outcome: FinishOutcome::Committed,
            })
            .map_err(|error| {
                journal_error(
                    TransactionPhase::Commit,
                    "all targets changed but final journal sync failed",
                    error,
                )
                .requiring_recovery()
            })?;
        cleanup_staged(&self.files)?;
        self.control.remove_journal().map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::RecoveryRequired,
                TransactionPhase::Cleanup,
                "commit succeeded but journal cleanup failed",
                error,
            )
            .requiring_recovery()
        })?;
        Ok(ApplyReport::new(
            self.transaction_id,
            self.operation,
            self.files.iter().map(|file| file.change.clone()).collect(),
        ))
    }

    /// Discards a prepared transaction without changing a target.
    pub fn abort(mut self) -> Result<AbortReport, WorktreeError> {
        self.journal
            .append(&JournalRecord::Finished {
                outcome: FinishOutcome::Aborted,
            })
            .map_err(|error| {
                journal_error(
                    TransactionPhase::Cleanup,
                    "failed to record explicit abort",
                    error,
                )
                .requiring_recovery()
            })?;
        let removed = cleanup_staged(&self.files)?;
        self.control.remove_journal().map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::RecoveryRequired,
                TransactionPhase::Cleanup,
                "abort succeeded but journal cleanup failed",
                error,
            )
            .requiring_recovery()
        })?;
        Ok(AbortReport::new(
            self.transaction_id,
            self.files.len(),
            removed,
        ))
    }
}

fn verify_all_backups(transaction: &PreparedTransaction) -> Result<(), WorktreeError> {
    for file in &transaction.files {
        verify_artifact(
            &file.access,
            &file.backup_name,
            file.old_hash,
            transaction.options.limits.max_source_bytes_per_file,
            file.original_index,
            TransactionPhase::Commit,
        )?;
    }
    Ok(())
}

fn commit_one(
    transaction: &mut PreparedTransaction,
    index: usize,
) -> Result<(), (WorktreeError, CommitFailureState)> {
    let file = &transaction.files[index];
    let verify = || -> Result<(), WorktreeError> {
        verify_target(
            &file.access,
            file.old_hash,
            Some(file.identity),
            transaction.options.limits.max_source_bytes_per_file,
            file.original_index,
            TransactionPhase::Commit,
        )?;
        verify_artifact(
            &file.access,
            &file.stage_name,
            file.new_hash,
            transaction.options.limits.max_output_bytes_per_file,
            file.original_index,
            TransactionPhase::Commit,
        )
    };
    verify().map_err(|error| (error, CommitFailureState::Unchanged))?;
    transaction
        .journal
        .append(&JournalRecord::CommitIntent {
            index: file.stable_index,
        })
        .map_err(|error| {
            (
                journal_error(
                    TransactionPhase::Commit,
                    "failed to synchronize commit intent",
                    error,
                )
                .requiring_recovery(),
                CommitFailureState::Unchanged,
            )
        })?;
    if let Err(source) = file.access.rename_from(&file.stage_name) {
        let state = classify_failed_rename(transaction, index);
        let mut error = fs_error(
            TransactionPhase::Commit,
            file.access.path(),
            file.original_index,
            "failed to replace target with staged output",
            source,
        );
        if matches!(state, CommitFailureState::Ambiguous) {
            error = error.requiring_recovery();
        }
        return Err((error, state));
    }
    file.access.sync_parent().map_err(|error| {
        (
            WorktreeError::with_source(
                WorktreeErrorCode::DurabilityFailed,
                TransactionPhase::Commit,
                "target changed but its parent directory did not synchronize",
                error,
            )
            .at_path(file.access.path().to_owned())
            .at_file(file.original_index)
            .requiring_recovery(),
            CommitFailureState::Changed,
        )
    })?;
    transaction
        .journal
        .append(&JournalRecord::Committed {
            index: file.stable_index,
        })
        .map_err(|error| {
            (
                journal_error(
                    TransactionPhase::Commit,
                    "target changed but its completion record did not synchronize",
                    error,
                )
                .requiring_recovery(),
                CommitFailureState::Changed,
            )
        })?;
    Ok(())
}

fn classify_failed_rename(transaction: &PreparedTransaction, index: usize) -> CommitFailureState {
    let file = &transaction.files[index];
    if verify_target(
        &file.access,
        file.new_hash,
        None,
        transaction.options.limits.max_output_bytes_per_file,
        file.original_index,
        TransactionPhase::Commit,
    )
    .is_ok()
    {
        CommitFailureState::Changed
    } else if verify_target(
        &file.access,
        file.old_hash,
        Some(file.identity),
        transaction.options.limits.max_source_bytes_per_file,
        file.original_index,
        TransactionPhase::Commit,
    )
    .is_ok()
    {
        CommitFailureState::Unchanged
    } else {
        CommitFailureState::Ambiguous
    }
}
