use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    journal::FinishOutcome,
    report::WorktreeApplyReport,
};

use super::{MutationState, commit_one, finish_without_changes, rollback_changed, verify_all};
use crate::operation::{
    PreparedWorktreeTransaction,
    journal::Record,
    operation_journal_error,
    undo::{RetainedApplyReport, UndoRetention, prepare_retention, retained_report},
};

/// Commits every path while keeping backups and a durable undo receipt.
///
/// The receipt is written after commit-time revalidation and before the first
/// target mutation. While the operation journal is active the receipt is
/// transitional: recovery keeps it only for a journal finished as committed
/// and removes it for every other outcome, so a crash anywhere between the
/// receipt write and the final `Finished` record resolves deterministically.
pub(super) fn commit_retained(
    mut transaction: PreparedWorktreeTransaction,
    retention: UndoRetention,
) -> Result<RetainedApplyReport, WorktreeError> {
    if let Err(error) = verify_all(&transaction) {
        finish_without_changes(&mut transaction)?;
        return Err(error.in_transaction(transaction.transaction_id.clone()));
    }
    let receipt = match prepare_retention(&transaction, retention) {
        Ok(receipt) => receipt,
        Err(error) => {
            finish_without_changes(&mut transaction)?;
            return Err(error.in_transaction(transaction.transaction_id.clone()));
        }
    };
    let mut changed = Vec::with_capacity(transaction.paths.len());
    for index in 0..transaction.paths.len() {
        if let Err((error, state)) = commit_one(&mut transaction, index) {
            match state {
                MutationState::Changed | MutationState::LinkedInstall => changed.push(index),
                MutationState::Unchanged => {}
                MutationState::Ambiguous => {
                    return Err(error
                        .in_transaction(transaction.transaction_id.clone())
                        .requiring_recovery());
                }
            }
            discard_transitional_receipt(&transaction)?;
            rollback_changed(&mut transaction, &changed)?;
            return Err(error.in_transaction(transaction.transaction_id.clone()));
        }
        changed.push(index);
    }
    finish_retained(&mut transaction)?;
    let touched = transaction.paths.len();
    let apply = WorktreeApplyReport::new(
        transaction.transaction_id,
        transaction.operation,
        transaction.operations,
        touched,
    );
    Ok(retained_report(apply, &receipt))
}

/// Removes the not-yet-final receipt before its backups are consumed by an
/// in-process rollback, mirroring what recovery does for aborted journals.
fn discard_transitional_receipt(
    transaction: &PreparedWorktreeTransaction,
) -> Result<(), WorktreeError> {
    transaction
        .control
        .remove_undo_receipt(&transaction.transaction_id)
        .map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::UndoFailed,
                TransactionPhase::Rollback,
                "failed to discard the transitional undo receipt",
                error,
            )
            .in_transaction(transaction.transaction_id.clone())
            .requiring_recovery()
        })
}

fn finish_retained(transaction: &mut PreparedWorktreeTransaction) -> Result<(), WorktreeError> {
    transaction
        .journal
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
    cleanup_stages(transaction)?;
    transaction
        .control
        .remove_operation_journal()
        .map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::RecoveryRequired,
                TransactionPhase::Cleanup,
                "retained commit succeeded but journal cleanup failed",
                error,
            )
            .requiring_recovery()
        })
}

/// Removes stage artifacts only; backup artifacts stay retained for undo.
fn cleanup_stages(transaction: &PreparedWorktreeTransaction) -> Result<(), WorktreeError> {
    for (index, path) in transaction.paths.iter().enumerate() {
        let Some(stage) = &path.stage_name else {
            continue;
        };
        path.access.remove_artifact(stage).map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::Io,
                TransactionPhase::Cleanup,
                "failed to remove a retained-commit stage artifact",
                error,
            )
            .at_path(path.path.clone())
            .at_file(index)
            .requiring_recovery()
        })?;
    }
    Ok(())
}
