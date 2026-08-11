mod apply;
mod evidence;
mod finished;
mod linked;

use std::io;

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{ControlDir, FsRoot},
    journal::FinishOutcome,
    options::WorktreeOptions,
    report::{RecoveryAction, RecoveryReport},
};

use super::{
    journal::{Record, Writer, read},
    recovery_model::parse_journal,
};
use apply::recover_state;
use evidence::cleanup_artifacts;

struct OperationLock {
    control: ControlDir,
    _file: std::fs::File,
}

pub(crate) fn recover_operation_transaction(
    root: &FsRoot,
    options: WorktreeOptions,
) -> Result<Option<RecoveryReport>, WorktreeError> {
    let Some(locked) = acquire(root)? else {
        return Ok(None);
    };
    let legacy = locked.control.open_journal().map_err(recovery_control_io)?;
    let operation = locked
        .control
        .open_operation_journal()
        .map_err(recovery_control_io)?;
    let undo = locked
        .control
        .open_undo_journal()
        .map_err(recovery_control_io)?;
    let active = [legacy.is_some(), operation.is_some(), undo.is_some()];
    if active.iter().filter(|journal| **journal).count() > 1 {
        return Err(WorktreeError::new(
            WorktreeErrorCode::JournalCorrupt,
            TransactionPhase::Recover,
            "edit, operation, and undo journals cannot be active together",
        )
        .requiring_recovery());
    }
    if let Some(file) = undo {
        return super::undo::recover_undo(&locked.control, root, file, options).map(Some);
    }
    let Some(file) = operation else {
        return Ok(None);
    };
    let entries = read(&file, options.limits.max_journal_bytes as u64).map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::JournalCorrupt,
            TransactionPhase::Recover,
            "failed to replay the operation journal",
            error,
        )
        .requiring_recovery()
    })?;
    if entries.is_empty() {
        locked
            .control
            .remove_operation_journal()
            .map_err(recovery_control_io)?;
        return Ok(Some(RecoveryReport::new(
            None,
            RecoveryAction::DiscardedStaging,
            Vec::new(),
            0,
        )));
    }
    let parsed = parse_journal(root, &entries, options)?;
    let transaction_id = parsed.transaction_id.clone();
    let mut writer =
        Writer::resume(file, options.limits.max_journal_bytes as u64).map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::JournalCorrupt,
                TransactionPhase::Recover,
                "failed to resume the operation journal",
                error,
            )
            .in_transaction(transaction_id.clone())
            .requiring_recovery()
        })?;
    let action = recover_state(&parsed, options, &mut writer)?;
    let retained = retained_receipt(&locked.control, &transaction_id, options)?;
    let keep_backups = retained.is_some() && action == RecoveryAction::FinishedCommitCleanup;
    let removed = cleanup_artifacts(&parsed, options, keep_backups)?;
    if keep_backups {
        super::undo::relocate_backups(
            &locked.control,
            root,
            retained.as_ref().expect("checked retained receipt"),
            options,
        )?;
    } else if retained.is_some() {
        // The receipt was transitional: the journal did not finish as
        // committed, so its backups were consumed or discarded above.
        locked
            .control
            .remove_undo_receipt(&transaction_id)
            .map_err(|error| {
                recovery_control_io(error)
                    .in_transaction(transaction_id.clone())
                    .requiring_recovery()
            })?;
    }
    locked.control.remove_operation_journal().map_err(|error| {
        recovery_control_io(error)
            .in_transaction(transaction_id.clone())
            .requiring_recovery()
    })?;
    Ok(Some(RecoveryReport::new(
        Some(transaction_id),
        action,
        Vec::new(),
        removed,
    )))
}

/// Reports whether a valid undo receipt is bound to the recovered
/// transaction; a corrupt receipt fails recovery closed.
fn retained_receipt(
    control: &ControlDir,
    transaction_id: &str,
    options: WorktreeOptions,
) -> Result<Option<super::undo::StoredReceipt>, WorktreeError> {
    if transaction_id.parse::<super::UndoId>().is_err() {
        return Ok(None);
    }
    super::undo::read_exact(control, transaction_id, options)
        .map_err(|error| error.in_transaction(transaction_id.to_owned()))
}

pub(super) fn append_finished(
    writer: &mut Writer,
    parsed: &super::recovery_model::ParsedJournal,
    outcome: FinishOutcome,
) -> Result<(), WorktreeError> {
    append_record(writer, parsed, &Record::Finished { outcome })
}

pub(super) fn append_record(
    writer: &mut Writer,
    parsed: &super::recovery_model::ParsedJournal,
    record: &Record,
) -> Result<(), WorktreeError> {
    writer.append(record).map(drop).map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::RecoveryRequired,
            TransactionPhase::Recover,
            "failed to synchronize an operation recovery record",
            error,
        )
        .in_transaction(parsed.transaction_id.clone())
        .requiring_recovery()
    })
}

fn acquire(root: &FsRoot) -> Result<Option<OperationLock>, WorktreeError> {
    let Some(control) = root.open_control(false).map_err(recovery_control_io)? else {
        return Ok(None);
    };
    let file = control.open_lock().map_err(recovery_control_io)?;
    match fs4::FileExt::try_lock(&file) {
        Ok(()) => Ok(Some(OperationLock {
            control,
            _file: file,
        })),
        Err(fs4::TryLockError::WouldBlock) => Err(WorktreeError::new(
            WorktreeErrorCode::RootBusy,
            TransactionPhase::Lock,
            "another worktree transaction holds the root lock",
        )),
        Err(fs4::TryLockError::Error(error)) => Err(WorktreeError::with_source(
            WorktreeErrorCode::Io,
            TransactionPhase::Lock,
            "failed to acquire the worktree lock for operation recovery",
            error,
        )),
    }
}

fn recovery_control_io(error: io::Error) -> WorktreeError {
    let code = if error.kind() == io::ErrorKind::InvalidData {
        WorktreeErrorCode::JournalCorrupt
    } else {
        WorktreeErrorCode::RecoveryRequired
    };
    WorktreeError::with_source(
        code,
        TransactionPhase::Recover,
        "operation recovery control I/O failed",
        error,
    )
    .requiring_recovery()
}
