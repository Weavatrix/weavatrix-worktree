use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{ControlDir, FsRoot, SlotEvidence, TargetAccess},
    journal::FinishOutcome,
    options::WorktreeOptions,
};

use super::receipt::{ReceiptPath, StoredReceipt};
use super::types::{UndoId, UndoRollbackReport};
use crate::operation::journal::{Record, Writer};

mod verify;

use verify::verify_receipt_state;

/// Exactly restores one retained commit behind complete evidence verification.
///
/// Every path is compared with the receipt's committed after-state and every
/// retained artifact with its recorded evidence before the first mutation, so
/// any divergence fails with `UNDO_CONFLICT` while the tree is untouched.
pub(in crate::operation) fn rollback(
    control: &ControlDir,
    root: &FsRoot,
    receipt: &StoredReceipt,
    options: WorktreeOptions,
) -> Result<UndoRollbackReport, WorktreeError> {
    let accesses = verify_receipt_state(control, root, receipt, options)?;
    let rollback_transaction_id = crate::operation::random_id()?;
    let mut journal = open_journal(control, receipt, &rollback_transaction_id, options)?;
    for (position, path) in receipt.paths().iter().enumerate().rev() {
        append(&mut journal, &Record::RollbackIntent { index: path.index })?;
        restore_path(control, &accesses[position], path, options)
            .map_err(|error| error.in_transaction(rollback_transaction_id.clone()))?;
        append(&mut journal, &Record::RolledBack { index: path.index })?;
    }
    append(
        &mut journal,
        &Record::Finished {
            outcome: FinishOutcome::RolledBack,
        },
    )?;
    consume_receipt(control, receipt.id())?;
    Ok(UndoRollbackReport {
        undo_id: UndoId::from_transaction(receipt.id().to_owned()),
        rollback_transaction_id,
        restored_paths: receipt.paths().len(),
        artifacts_removed: consumed_artifacts(receipt),
    })
}

/// Restores one receipt path from the exact committed state to its before
/// state, consuming the retained backup artifact where one exists.
pub(super) fn restore_path(
    control: &ControlDir,
    access: &TargetAccess,
    path: &ReceiptPath,
    options: WorktreeOptions,
) -> Result<(), WorktreeError> {
    match (path.before, path.after) {
        (SlotEvidence::Absent, SlotEvidence::Present(after)) => access
            .remove_exact(after, options.limits.max_output_bytes_per_file)
            .map_err(|error| restore_error(path, "failed to remove a retained create", error))?,
        (SlotEvidence::Present(_), _) => {
            let backup = path
                .backup_name
                .as_deref()
                .ok_or_else(|| receipt_logic(path, "retained path lacks its backup name"))?;
            match current_state(access, path, options)? {
                SlotEvidence::Absent => control
                    .install_absent_target_from_backup(access, backup)
                    .map_err(|error| {
                    restore_error(path, "failed to reinstall the retained backup", error)
                })?,
                actual if actual == path.after => control
                    .replace_target_from_backup(access, backup)
                    .map_err(|error| {
                        restore_error(path, "failed to restore the retained backup", error)
                    })?,
                SlotEvidence::Present(_) => {
                    return Err(foreign(
                        path,
                        "refusing to overwrite a diverged retained path",
                    ));
                }
            }
        }
        (SlotEvidence::Absent, SlotEvidence::Absent) => {
            return Err(receipt_logic(path, "retained path has an empty transition"));
        }
    }
    access
        .sync_parent()
        .map_err(|error| restore_error(path, "retained restore did not synchronize", error))?;
    verify_restored(access, path, options)
}

/// Verifies that a restored path matches its exact before evidence, using the
/// backup artifact identity for present states as the restore preserves it.
pub(super) fn verify_restored(
    access: &TargetAccess,
    path: &ReceiptPath,
    options: WorktreeOptions,
) -> Result<(), WorktreeError> {
    access
        .verify_slot(
            restored_evidence(path)?,
            options.limits.max_source_bytes_per_file,
        )
        .map(drop)
        .map_err(|error| restore_error(path, "restored path failed exact verification", error))
}

/// Complete before-state evidence a finished restore must present.
pub(super) fn restored_evidence(path: &ReceiptPath) -> Result<SlotEvidence, WorktreeError> {
    match path.before {
        SlotEvidence::Absent => Ok(SlotEvidence::Absent),
        SlotEvidence::Present(before) => {
            let backup = path
                .backup
                .ok_or_else(|| receipt_logic(path, "retained path lacks backup evidence"))?;
            let mut restored = before;
            restored.identity = backup.identity;
            Ok(SlotEvidence::Present(restored))
        }
    }
}

pub(super) fn consumed_artifacts(receipt: &StoredReceipt) -> usize {
    receipt
        .paths()
        .iter()
        .filter(|path| path.backup_name.is_some())
        .count()
}

pub(super) fn state_bytes(options: WorktreeOptions) -> usize {
    options
        .limits
        .max_source_bytes_per_file
        .max(options.limits.max_output_bytes_per_file)
}

fn open_journal(
    control: &ControlDir,
    receipt: &StoredReceipt,
    transaction_id: &str,
    options: WorktreeOptions,
) -> Result<Writer, WorktreeError> {
    let file = control.create_undo_journal().map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::UndoFailed,
            TransactionPhase::Rollback,
            "failed to create the exclusive undo journal",
            error,
        )
    })?;
    control.sync().map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::DurabilityFailed,
            TransactionPhase::Rollback,
            "failed to synchronize the undo journal directory",
            error,
        )
        .requiring_recovery()
    })?;
    let mut journal = Writer::new(file, options.limits.max_journal_bytes as u64)
        .map_err(|error| journal_error("invalid new undo journal", error))?;
    journal
        .append(&Record::Header {
            transaction_id: transaction_id.to_owned(),
            contract_hash: receipt.checksum.to_string(),
            operation: receipt.id().to_owned(),
            operation_count: 0,
            path_count: u32::try_from(receipt.paths().len()).map_err(|_| {
                WorktreeError::new(
                    WorktreeErrorCode::UndoCorrupt,
                    TransactionPhase::Rollback,
                    "retained path count does not fit the journal contract",
                )
                .requiring_recovery()
            })?,
        })
        .map_err(|error| journal_error("failed to synchronize the undo journal header", error))?;
    Ok(journal)
}

fn append(journal: &mut Writer, record: &Record) -> Result<(), WorktreeError> {
    journal
        .append(record)
        .map(drop)
        .map_err(|error| journal_error("failed to synchronize an undo journal record", error))
}

fn consume_receipt(control: &ControlDir, id: &str) -> Result<(), WorktreeError> {
    control.remove_undo_receipt(id).map_err(cleanup_error)?;
    control.remove_undo_journal().map_err(cleanup_error)
}

fn cleanup_error(error: std::io::Error) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::UndoFailed,
        TransactionPhase::Cleanup,
        "undo rollback succeeded but retained-state cleanup failed",
        error,
    )
    .requiring_recovery()
}

fn journal_error(message: &str, source: crate::operation::journal::JournalError) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::JournalCorrupt,
        TransactionPhase::Rollback,
        message,
        source,
    )
    .requiring_recovery()
}

fn conflict(path: &ReceiptPath, message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::UndoConflict,
        TransactionPhase::Rollback,
        message,
    )
    .at_path(path.path.clone())
    .at_file(path.index as usize)
}

fn verify_io(path: &ReceiptPath, message: &str, error: std::io::Error) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::UndoFailed,
        TransactionPhase::Rollback,
        message,
        error,
    )
    .at_path(path.path.clone())
    .at_file(path.index as usize)
}

fn restore_error(path: &ReceiptPath, message: &str, error: std::io::Error) -> WorktreeError {
    verify_io(path, message, error).requiring_recovery()
}

fn foreign(path: &ReceiptPath, message: &str) -> WorktreeError {
    conflict(path, message).requiring_recovery()
}

fn receipt_logic(path: &ReceiptPath, message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::UndoCorrupt,
        TransactionPhase::Rollback,
        message,
    )
    .at_path(path.path.clone())
    .at_file(path.index as usize)
    .requiring_recovery()
}

fn current_state(
    access: &TargetAccess,
    path: &ReceiptPath,
    options: WorktreeOptions,
) -> Result<SlotEvidence, WorktreeError> {
    access
        .slot_evidence(state_bytes(options))
        .map_err(|error| restore_error(path, "failed to inspect a retained restore target", error))
}
