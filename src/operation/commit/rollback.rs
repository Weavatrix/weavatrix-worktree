use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::SlotEvidence,
    journal::FinishOutcome,
};

use super::{PreparedWorktreeTransaction, StagedPath};
use crate::operation::{journal::Record, operation_journal_error, stage::cleanup};

use super::evidence::present;

pub(super) fn rollback_changed(
    transaction: &mut PreparedWorktreeTransaction,
    changed: &[usize],
) -> Result<(), WorktreeError> {
    for &index in changed.iter().rev() {
        let path = &transaction.paths[index];
        transaction
            .journal
            .append(&Record::RollbackIntent {
                index: path.stable_index,
            })
            .map_err(|error| {
                operation_journal_error(
                    TransactionPhase::Rollback,
                    "failed to synchronize operation rollback intent",
                    error,
                )
                .requiring_recovery()
            })?;
        restore(path, transaction).map_err(|error| {
            error
                .in_transaction(transaction.transaction_id.clone())
                .requiring_recovery()
        })?;
        transaction
            .journal
            .append(&Record::RolledBack {
                index: path.stable_index,
            })
            .map_err(|error| {
                operation_journal_error(
                    TransactionPhase::Rollback,
                    "restored path but rollback record did not synchronize",
                    error,
                )
                .requiring_recovery()
            })?;
    }
    transaction
        .journal
        .append(&Record::Finished {
            outcome: FinishOutcome::RolledBack,
        })
        .map_err(|error| {
            operation_journal_error(
                TransactionPhase::Rollback,
                "failed to record completed operation rollback",
                error,
            )
            .requiring_recovery()
        })?;
    cleanup(&transaction.paths)?;
    transaction
        .control
        .remove_operation_journal()
        .map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::RecoveryRequired,
                TransactionPhase::Cleanup,
                "rollback succeeded but operation journal cleanup failed",
                error,
            )
            .requiring_recovery()
        })
}

fn restore(
    path: &StagedPath,
    transaction: &PreparedWorktreeTransaction,
) -> Result<(), WorktreeError> {
    match path.before {
        SlotEvidence::Absent => {
            if let Some(stage) = &path.stage_name
                && path.access.same_file_as_artifact(stage).unwrap_or(false)
            {
                path.access
                    .rollback_linked_install(stage)
                    .map_err(|error| {
                        rollback_error(path, "failed to reverse linked install", error)
                    })?;
            } else {
                path.access
                    .remove_exact(
                        present(path.after).ok_or_else(|| {
                            rollback_logic(path, "absent source has no present output")
                        })?,
                        transaction.options.limits.max_output_bytes_per_file,
                    )
                    .map_err(|error| {
                        rollback_error(path, "failed to remove committed output", error)
                    })?;
            }
            path.access
                .sync_parent()
                .map_err(|error| rollback_error(path, "rollback did not synchronize", error))
        }
        SlotEvidence::Present(before) => {
            let backup = required(path.backup_name.as_deref(), path)?;
            match path
                .access
                .slot_evidence(transaction.options.limits.max_output_bytes_per_file)
            {
                Ok(SlotEvidence::Absent) => {
                    path.access.install_absent_from(backup).map_err(|error| {
                        rollback_error(path, "failed to reinstall exact backup", error)
                    })
                }
                Ok(SlotEvidence::Present(actual)) if Some(actual) == present(path.after) => path
                    .access
                    .replace_from(backup)
                    .map_err(|error| rollback_error(path, "failed to restore exact backup", error)),
                Ok(_) => Err(rollback_logic(
                    path,
                    "refusing to overwrite a foreign path state",
                )),
                Err(error) => Err(rollback_error(
                    path,
                    "failed to inspect rollback target",
                    error,
                )),
            }?;
            path.access
                .sync_parent()
                .map_err(|error| rollback_error(path, "rollback did not synchronize", error))?;
            let mut restored = before;
            restored.identity = path
                .backup
                .ok_or_else(|| rollback_logic(path, "backup evidence is missing"))?
                .identity;
            path.access
                .verify_slot(
                    SlotEvidence::Present(restored),
                    transaction.options.limits.max_source_bytes_per_file,
                )
                .map(drop)
                .map_err(|error| rollback_error(path, "restored backup failed verification", error))
        }
    }
}

pub(super) fn finish_without_changes(
    transaction: &mut PreparedWorktreeTransaction,
) -> Result<(), WorktreeError> {
    transaction
        .journal
        .append(&Record::Finished {
            outcome: FinishOutcome::Aborted,
        })
        .map_err(|error| {
            operation_journal_error(
                TransactionPhase::Cleanup,
                "failed to record unchanged operation abort",
                error,
            )
            .requiring_recovery()
        })?;
    cleanup(&transaction.paths)?;
    transaction
        .control
        .remove_operation_journal()
        .map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::RecoveryRequired,
                TransactionPhase::Cleanup,
                "failed to remove unchanged operation journal",
                error,
            )
            .requiring_recovery()
        })
}

pub(super) fn required<'a>(
    value: Option<&'a str>,
    path: &StagedPath,
) -> Result<&'a str, WorktreeError> {
    value.ok_or_else(|| rollback_logic(path, "required transaction artifact is missing"))
}

fn rollback_error(path: &StagedPath, message: &str, source: std::io::Error) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::RollbackFailed,
        TransactionPhase::Rollback,
        message,
        source,
    )
    .at_path(path.path.clone())
    .at_file(path.stable_index as usize)
}

fn rollback_logic(path: &StagedPath, message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::RollbackFailed,
        TransactionPhase::Rollback,
        message,
    )
    .at_path(path.path.clone())
    .at_file(path.stable_index as usize)
}
