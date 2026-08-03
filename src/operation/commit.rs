use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::SlotEvidence,
    journal::FinishOutcome,
    report::{AbortReport, WorktreeApplyReport},
};

mod evidence;
mod retained;
mod rollback;

use evidence::{classify, path_error, present, verify_all, verify_slot};
use rollback::{finish_without_changes, required, rollback_changed};

use super::{
    PreparedWorktreeTransaction, journal::Record, model::StagedPath, operation_journal_error,
    stage::cleanup,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MutationState {
    Unchanged,
    Changed,
    LinkedInstall,
    Ambiguous,
}

impl PreparedWorktreeTransaction {
    /// Revalidates every path and atomically commits each unique slot in path order.
    pub fn commit(mut self) -> Result<WorktreeApplyReport, WorktreeError> {
        if let Err(error) = verify_all(&self) {
            finish_without_changes(&mut self)?;
            return Err(error.in_transaction(self.transaction_id.clone()));
        }
        let mut changed = Vec::with_capacity(self.paths.len());
        for index in 0..self.paths.len() {
            if let Err((error, state)) = commit_one(&mut self, index) {
                match state {
                    MutationState::Changed | MutationState::LinkedInstall => changed.push(index),
                    MutationState::Unchanged => {}
                    MutationState::Ambiguous => {
                        return Err(error
                            .in_transaction(self.transaction_id.clone())
                            .requiring_recovery());
                    }
                }
                rollback_changed(&mut self, &changed)?;
                return Err(error.in_transaction(self.transaction_id.clone()));
            }
            changed.push(index);
        }
        self.journal
            .append(&Record::Finished {
                outcome: FinishOutcome::Committed,
            })
            .map_err(|error| {
                operation_journal_error(
                    TransactionPhase::Commit,
                    "all operation paths changed but final journal sync failed",
                    error,
                )
                .requiring_recovery()
            })?;
        cleanup(&self.paths)?;
        self.control.remove_operation_journal().map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::RecoveryRequired,
                TransactionPhase::Cleanup,
                "operation commit succeeded but journal cleanup failed",
                error,
            )
            .requiring_recovery()
        })?;
        Ok(WorktreeApplyReport::new(
            self.transaction_id,
            self.operation,
            self.operations,
            self.paths.len(),
        ))
    }

    /// Commits like [`Self::commit`] while retaining exact undo evidence.
    ///
    /// Backup artifacts survive the commit and a checksummed receipt bound to
    /// this transaction is durably written before the final journal record, so
    /// the committed state stays reversible through `Worktree::rollback_undo`.
    pub fn commit_retained(
        self,
        retention: crate::operation::UndoRetention,
    ) -> Result<crate::operation::RetainedApplyReport, WorktreeError> {
        retained::commit_retained(self, retention)
    }

    /// Discards a prepared operation plan without changing a worktree path.
    pub fn abort(mut self) -> Result<AbortReport, WorktreeError> {
        self.journal
            .append(&Record::Finished {
                outcome: FinishOutcome::Aborted,
            })
            .map_err(|error| {
                operation_journal_error(
                    TransactionPhase::Cleanup,
                    "failed to record explicit operation abort",
                    error,
                )
                .requiring_recovery()
            })?;
        let removed = cleanup(&self.paths)?;
        self.control.remove_operation_journal().map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::RecoveryRequired,
                TransactionPhase::Cleanup,
                "operation abort succeeded but journal cleanup failed",
                error,
            )
            .requiring_recovery()
        })?;
        Ok(AbortReport::new(
            self.transaction_id,
            self.paths.len(),
            removed,
        ))
    }
}

fn commit_one(
    transaction: &mut PreparedWorktreeTransaction,
    index: usize,
) -> Result<(), (WorktreeError, MutationState)> {
    let path = &transaction.paths[index];
    transaction
        .root
        .revalidate_parent(&path.access)
        .map_err(|error| {
            (
                path_error(
                    TransactionPhase::Commit,
                    path,
                    index,
                    "path parent changed before commit",
                    error,
                ),
                MutationState::Unchanged,
            )
        })?;
    verify_slot(path, path.before, transaction, index, "source path changed")
        .map_err(|error| (error, MutationState::Unchanged))?;
    transaction
        .journal
        .append(&Record::CommitIntent {
            index: path.stable_index,
        })
        .map_err(|error| {
            (
                operation_journal_error(
                    TransactionPhase::Commit,
                    "failed to synchronize operation commit intent",
                    error,
                )
                .requiring_recovery(),
                MutationState::Unchanged,
            )
        })?;
    if let Err(error) = mutate(path, transaction) {
        let state = classify(path, transaction);
        return Err((
            if state == MutationState::Ambiguous {
                error.requiring_recovery()
            } else {
                error
            },
            state,
        ));
    }
    path.access.sync_parent().map_err(|error| {
        (
            path_error(
                TransactionPhase::Commit,
                path,
                index,
                "path changed but parent synchronization failed",
                error,
            )
            .requiring_recovery(),
            classify(path, transaction),
        )
    })?;
    transaction
        .journal
        .append(&Record::Committed {
            index: path.stable_index,
        })
        .map_err(|error| {
            (
                operation_journal_error(
                    TransactionPhase::Commit,
                    "path changed but completion record did not synchronize",
                    error,
                )
                .requiring_recovery(),
                MutationState::Changed,
            )
        })?;
    Ok(())
}

fn mutate(
    path: &StagedPath,
    transaction: &PreparedWorktreeTransaction,
) -> Result<(), WorktreeError> {
    match (path.before, path.after) {
        (SlotEvidence::Absent, SlotEvidence::Present(_)) => path
            .access
            .install_absent_from(required(path.stage_name.as_deref(), path)?)
            .map_err(|error| {
                mutation_error(path, "failed to install an absent destination", error)
            }),
        (SlotEvidence::Present(_), SlotEvidence::Absent) => path
            .access
            .remove_exact(
                present(path.before).expect("present match arm"),
                transaction.options.limits.max_source_bytes_per_file,
            )
            .map_err(|error| mutation_error(path, "failed to remove an exact source", error)),
        (SlotEvidence::Present(_), SlotEvidence::Present(_)) => path
            .access
            .replace_from(required(path.stage_name.as_deref(), path)?)
            .map_err(|error| mutation_error(path, "failed to replace an exact path", error)),
        (SlotEvidence::Absent, SlotEvidence::Absent) => Err(WorktreeError::new(
            WorktreeErrorCode::InvalidPlan,
            TransactionPhase::Commit,
            "operation contains an empty path transition",
        )
        .at_path(path.path.clone())),
    }
}

fn mutation_error(path: &StagedPath, message: &str, source: std::io::Error) -> WorktreeError {
    path_error(
        TransactionPhase::Commit,
        path,
        path.stable_index as usize,
        message,
        source,
    )
}
