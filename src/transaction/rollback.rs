use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    journal::{FinishOutcome, JournalRecord},
};

use super::{
    PreparedTransaction,
    stage::cleanup_staged,
    util::{fs_error, journal_error},
    verify::{verify_artifact, verify_target},
};

pub(crate) fn rollback_committed(
    transaction: &mut PreparedTransaction,
    committed: &[usize],
) -> Result<(), WorktreeError> {
    for &index in committed.iter().rev() {
        rollback_one(transaction, index)?;
    }
    transaction
        .journal
        .append(&JournalRecord::Finished {
            outcome: FinishOutcome::RolledBack,
        })
        .map_err(|error| {
            journal_error(
                TransactionPhase::Rollback,
                "failed to record completed rollback",
                error,
            )
            .requiring_recovery()
        })?;
    cleanup_staged(&transaction.files)?;
    transaction.control.remove_journal().map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::RecoveryRequired,
            TransactionPhase::Cleanup,
            "rollback succeeded but journal cleanup failed",
            error,
        )
        .requiring_recovery()
    })
}

fn rollback_one(transaction: &mut PreparedTransaction, index: usize) -> Result<(), WorktreeError> {
    let file = &transaction.files[index];
    verify_target(
        &file.access,
        file.new_hash,
        None,
        transaction.options.limits.max_output_bytes_per_file,
        file.original_index,
        TransactionPhase::Rollback,
    )
    .map_err(|error| recovery_error(&error))?;
    verify_artifact(
        &file.access,
        &file.backup_name,
        file.old_hash,
        transaction.options.limits.max_source_bytes_per_file,
        file.original_index,
        TransactionPhase::Rollback,
    )?;
    transaction
        .journal
        .append(&JournalRecord::RollbackIntent {
            index: file.stable_index,
        })
        .map_err(|error| {
            journal_error(
                TransactionPhase::Rollback,
                "failed to synchronize rollback intent",
                error,
            )
            .requiring_recovery()
        })?;
    file.access
        .rename_from(&file.backup_name)
        .and_then(|()| file.access.sync_parent())
        .map_err(|error| {
            fs_error(
                TransactionPhase::Rollback,
                file.access.path(),
                file.original_index,
                "failed to restore a backup",
                error,
            )
            .requiring_recovery()
        })?;
    transaction
        .journal
        .append(&JournalRecord::RolledBack {
            index: file.stable_index,
        })
        .map_err(|error| {
            journal_error(
                TransactionPhase::Rollback,
                "failed to record restored target",
                error,
            )
            .requiring_recovery()
        })?;
    Ok(())
}

fn recovery_error(error: &WorktreeError) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::RollbackFailed,
        TransactionPhase::Rollback,
        error.to_string(),
    )
    .requiring_recovery()
}
