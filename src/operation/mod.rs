mod commit;
mod contract;
mod journal;
mod model;
mod pending;
mod projection;
mod recovery;
mod recovery_model;
mod stage;
#[cfg(test)]
mod tests;
mod undo;

use crate::{
    WorktreePlan,
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{ControlDir, FsRoot},
    journal::FinishOutcome,
    options::WorktreeOptions,
    report::{OperationChange, WorktreeDryRunReport},
    transaction::acquire,
};

use journal::{Record, Writer};
use model::StagedPath;
use pending::ensure_no_pending;
pub use undo::{
    ParseUndoIdError, RetainedApplyReport, UndoId, UndoReceipt, UndoRetention, UndoRollbackReport,
    UndoStoreUsage, WorktreeSnapshotFingerprint,
};
pub(crate) use undo::{undo_discard, undo_receipts, undo_rollback, undo_usage};

/// A durably staged create/delete/modify/rename transaction holding the root lock.
#[must_use = "commit, abort, or later recover the prepared transaction"]
pub struct PreparedWorktreeTransaction {
    transaction_id: String,
    contract_hash: crate::Sha256Hash,
    operation: String,
    preview: WorktreeDryRunReport,
    operations: Vec<OperationChange>,
    paths: Vec<StagedPath>,
    options: WorktreeOptions,
    root: FsRoot,
    journal: Writer,
    control: ControlDir,
    _lock: std::fs::File,
}

impl core::fmt::Debug for PreparedWorktreeTransaction {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PreparedWorktreeTransaction")
            .field("transaction_id", &self.transaction_id)
            .field("operation", &self.operation)
            .field("prepared_paths", &self.paths.len())
            .finish_non_exhaustive()
    }
}

impl PreparedWorktreeTransaction {
    #[must_use]
    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    #[must_use]
    pub const fn preview(&self) -> &WorktreeDryRunReport {
        &self.preview
    }
}

pub(crate) fn dry_run_operation_plan(
    root: &FsRoot,
    options: WorktreeOptions,
    plan: &WorktreePlan,
) -> Result<WorktreeDryRunReport, WorktreeError> {
    let validated = contract::validate(plan, options)?;
    projection::project(root, options, &validated).map(|value| projection::preview(&value))
}

pub(crate) fn prepare_operation_plan(
    root: &FsRoot,
    options: WorktreeOptions,
    plan: &WorktreePlan,
) -> Result<PreparedWorktreeTransaction, WorktreeError> {
    let validated = contract::validate(plan, options)?;
    let locked = acquire(root)?;
    ensure_no_pending(&locked.control)?;
    let projected = projection::project(root, options, &validated)?;
    let preview = projection::preview(&projected);
    let path_count = projected.paths.len();
    let transaction_id = random_id()?;
    let file = locked.control.create_operation_journal().map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::Io,
            TransactionPhase::Prepare,
            "failed to create the exclusive operation journal",
            error,
        )
    })?;
    locked.control.sync().map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::DurabilityFailed,
            TransactionPhase::Prepare,
            "failed to synchronize the operation journal directory",
            error,
        )
        .requiring_recovery()
    })?;
    let mut journal =
        Writer::new(file, options.limits.max_journal_bytes as u64).map_err(|error| {
            operation_journal_error(
                TransactionPhase::Prepare,
                "invalid new operation journal",
                error,
            )
            .requiring_recovery()
        })?;
    append_header(
        &mut journal,
        plan,
        validated.fingerprint(),
        &transaction_id,
        path_count,
    )?;
    let (operation, operations, paths) =
        match stage::stage_all(projected, &transaction_id, options, &mut journal) {
            Ok(value) => value,
            Err(error) if error.recovery_required() => {
                return Err(error.in_transaction(transaction_id));
            }
            Err(error) => {
                finish_failed_prepare(&mut journal, &locked.control)?;
                return Err(error.in_transaction(transaction_id));
            }
        };
    journal
        .append(&Record::Prepared {
            operation_count: u32::try_from(operations.len())
                .map_err(|_| too_large("operation count does not fit the journal contract"))?,
            path_count: u32::try_from(paths.len())
                .map_err(|_| too_large("path count does not fit the journal contract"))?,
        })
        .map_err(|error| {
            operation_journal_error(
                TransactionPhase::Prepare,
                "failed to record durable operation preparation",
                error,
            )
            .requiring_recovery()
        })?;
    Ok(PreparedWorktreeTransaction {
        transaction_id,
        contract_hash: crate::Sha256Hash::parse(&validated.fingerprint().to_string())
            .expect("validated plan fingerprint is SHA-256"),
        operation,
        preview,
        operations,
        paths,
        options,
        root: root.try_clone().map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::Io,
                TransactionPhase::Prepare,
                "failed to retain the worktree root capability",
                error,
            )
            .requiring_recovery()
        })?,
        journal,
        control: locked.control,
        _lock: locked.file,
    })
}

pub(crate) use recovery::recover_operation_transaction;

fn append_header(
    journal: &mut Writer,
    plan: &WorktreePlan,
    contract_hash: weavatrix_refactor_plan::PlanFingerprint,
    transaction_id: &str,
    path_count: usize,
) -> Result<(), WorktreeError> {
    journal
        .append(&Record::Header {
            transaction_id: transaction_id.to_owned(),
            contract_hash: contract_hash.to_string(),
            operation: plan.operation.clone(),
            operation_count: u32::try_from(plan.operations.len())
                .map_err(|_| too_large("operation count does not fit the journal contract"))?,
            path_count: u32::try_from(path_count)
                .map_err(|_| too_large("path count does not fit the journal contract"))?,
        })
        .map_err(|error| {
            operation_journal_error(
                TransactionPhase::Prepare,
                "failed to synchronize the operation journal header",
                error,
            )
            .requiring_recovery()
        })?;
    Ok(())
}

fn finish_failed_prepare(journal: &mut Writer, control: &ControlDir) -> Result<(), WorktreeError> {
    journal
        .append(&Record::Finished {
            outcome: FinishOutcome::Aborted,
        })
        .map_err(|error| {
            operation_journal_error(
                TransactionPhase::Cleanup,
                "failed to record aborted operation preparation",
                error,
            )
            .requiring_recovery()
        })?;
    control.remove_operation_journal().map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::RecoveryRequired,
            TransactionPhase::Cleanup,
            "failed to remove the aborted operation journal",
            error,
        )
        .requiring_recovery()
    })
}

fn random_id() -> Result<String, WorktreeError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::Io,
            TransactionPhase::Prepare,
            "failed to generate an operation transaction identifier",
            error,
        )
    })?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(32), |mut value, byte| {
            value.push(char::from(HEX[usize::from(byte >> 4)]));
            value.push(char::from(HEX[usize::from(byte & 15)]));
            value
        }))
}

fn operation_journal_error(
    phase: TransactionPhase,
    message: &str,
    source: impl std::error::Error + Send + Sync + 'static,
) -> WorktreeError {
    WorktreeError::with_source(WorktreeErrorCode::JournalCorrupt, phase, message, source)
}

fn too_large(message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::TransactionTooLarge,
        TransactionPhase::Prepare,
        message,
    )
}
