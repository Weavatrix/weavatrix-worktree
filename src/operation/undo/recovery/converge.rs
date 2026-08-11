use std::io;

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{ControlDir, SlotEvidence, TargetAccess},
    options::WorktreeOptions,
};

use super::UndoReplay;
use crate::operation::journal::{Record, Writer};
use crate::operation::undo::receipt::{ReceiptPath, StoredReceipt};
use crate::operation::undo::rollback::{
    restore_path, restored_evidence, state_bytes, verify_restored,
};

/// Verifies that a journal finished as rolled back left every exact before
/// state in place before the receipt and journal are removed.
pub(super) fn verify_finished(
    _control: &ControlDir,
    receipt: &StoredReceipt,
    accesses: &[TargetAccess],
    replay: &UndoReplay,
    options: WorktreeOptions,
) -> Result<(), WorktreeError> {
    for (position, path) in receipt.paths().iter().enumerate() {
        let access = &accesses[position];
        if current(access, path, replay, options)? != restored(path, replay)? {
            return Err(foreign(
                path,
                replay,
                "finished undo no longer matches exact before evidence",
            ));
        }
        access.sync_parent().map_err(|error| {
            path_io(
                path,
                replay,
                "finished undo path did not synchronize",
                error,
            )
        })?;
    }
    Ok(())
}

/// Idempotently completes an interrupted undo in reverse index order.
pub(super) fn complete(
    control: &ControlDir,
    receipt: &StoredReceipt,
    accesses: &[TargetAccess],
    replay: &UndoReplay,
    writer: &mut Writer,
    options: WorktreeOptions,
) -> Result<usize, WorktreeError> {
    let mut removed = 0;
    for (position, path) in receipt.paths().iter().enumerate().rev() {
        removed += converge_path(control, &accesses[position], path, replay, writer, options)?;
    }
    Ok(removed)
}

fn converge_path(
    control: &ControlDir,
    access: &TargetAccess,
    path: &ReceiptPath,
    replay: &UndoReplay,
    writer: &mut Writer,
    options: WorktreeOptions,
) -> Result<usize, WorktreeError> {
    if linked_restore_is_exact(control, access, path, replay, options)? {
        return finish_linked_restore(control, access, path, replay, writer, options);
    }
    let actual = current(access, path, replay, options)?;
    let restored = restored(path, replay)?;
    if replay.rolled_back.contains(&path.index) {
        if actual == restored {
            return Ok(0);
        }
        return Err(foreign(
            path,
            replay,
            "a journaled restored path no longer matches its before state",
        ));
    }
    if actual == restored {
        if !replay.intents.contains(&path.index) {
            return Err(foreign(
                path,
                replay,
                "path was restored without a durable undo intent",
            ));
        }
        record(writer, replay, &Record::RolledBack { index: path.index })?;
        return Ok(consumed(path));
    }
    if actual != path.after {
        return Err(foreign(
            path,
            replay,
            "path matches neither its committed nor its restored evidence",
        ));
    }
    if !replay.intents.contains(&path.index) {
        record(
            writer,
            replay,
            &Record::RollbackIntent { index: path.index },
        )?;
    }
    restore_path(control, access, path, options)
        .map_err(|error| error.in_transaction(replay.rollback_id.clone()))?;
    record(writer, replay, &Record::RolledBack { index: path.index })?;
    Ok(consumed(path))
}

fn finish_linked_restore(
    control: &ControlDir,
    access: &TargetAccess,
    path: &ReceiptPath,
    replay: &UndoReplay,
    writer: &mut Writer,
    options: WorktreeOptions,
) -> Result<usize, WorktreeError> {
    if replay.rolled_back.contains(&path.index) || !replay.intents.contains(&path.index) {
        return Err(foreign(
            path,
            replay,
            "linked undo restore contradicts the journaled intents",
        ));
    }
    let (Some(name), Some(backup)) = (path.backup_name.as_deref(), path.backup) else {
        return Err(foreign(
            path,
            replay,
            "linked undo restore lacks its backup",
        ));
    };
    control
        .finish_linked_restore(access, name, backup.identity)
        .map_err(|error| {
            path_io(
                path,
                replay,
                "failed to finish a linked undo restore",
                error,
            )
        })?;
    verify_restored(access, path, options)
        .map_err(|error| error.in_transaction(replay.rollback_id.clone()))?;
    record(writer, replay, &Record::RolledBack { index: path.index })?;
    Ok(consumed(path))
}

fn linked_restore_is_exact(
    control: &ControlDir,
    access: &TargetAccess,
    path: &ReceiptPath,
    replay: &UndoReplay,
    options: WorktreeOptions,
) -> Result<bool, WorktreeError> {
    let (SlotEvidence::Present(_), Some(name), Some(backup)) =
        (path.before, path.backup_name.as_deref(), path.backup)
    else {
        return Ok(false);
    };
    match control.same_file_as_target(access, name) {
        Ok(false) => Ok(false),
        Ok(true) => {
            let evidence = control
                .linked_backup_evidence(access, name, state_bytes(options))
                .map_err(|error| {
                    path_io(
                        path,
                        replay,
                        "failed to verify a linked undo restore",
                        error,
                    )
                })?;
            if evidence == backup {
                Ok(true)
            } else {
                Err(foreign(
                    path,
                    replay,
                    "linked undo restore does not match retained evidence",
                ))
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_io(
            path,
            replay,
            "failed to inspect a linked undo restore",
            error,
        )),
    }
}

fn record(writer: &mut Writer, replay: &UndoReplay, record: &Record) -> Result<(), WorktreeError> {
    writer.append(record).map(drop).map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::RecoveryRequired,
            TransactionPhase::Recover,
            "failed to synchronize an undo recovery record",
            error,
        )
        .in_transaction(replay.rollback_id.clone())
        .requiring_recovery()
    })
}

fn current(
    access: &TargetAccess,
    path: &ReceiptPath,
    replay: &UndoReplay,
    options: WorktreeOptions,
) -> Result<SlotEvidence, WorktreeError> {
    access.slot_evidence(state_bytes(options)).map_err(|error| {
        path_io(
            path,
            replay,
            "failed to inspect an undo recovery target",
            error,
        )
    })
}

fn restored(path: &ReceiptPath, replay: &UndoReplay) -> Result<SlotEvidence, WorktreeError> {
    restored_evidence(path).map_err(|error| error.in_transaction(replay.rollback_id.clone()))
}

fn consumed(path: &ReceiptPath) -> usize {
    usize::from(path.backup_name.is_some())
}

fn foreign(path: &ReceiptPath, replay: &UndoReplay, message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::UndoConflict,
        TransactionPhase::Recover,
        message,
    )
    .at_path(path.path.clone())
    .at_file(path.index as usize)
    .in_transaction(replay.rollback_id.clone())
    .requiring_recovery()
}

fn path_io(
    path: &ReceiptPath,
    replay: &UndoReplay,
    message: &str,
    error: io::Error,
) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::RecoveryRequired,
        TransactionPhase::Recover,
        message,
        error,
    )
    .at_path(path.path.clone())
    .at_file(path.index as usize)
    .in_transaction(replay.rollback_id.clone())
    .requiring_recovery()
}
