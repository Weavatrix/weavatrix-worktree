use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{ControlDir, FsRoot},
    options::WorktreeOptions,
    report::WorktreeApplyReport,
};

use super::types::{RetainedApplyReport, UndoId, UndoReceipt, UndoRollbackReport, UndoStoreUsage};
use super::{StoredReceipt, discard, inspect, rollback, store_usage};

struct UndoSession {
    control: ControlDir,
    _lock: std::fs::File,
}

/// Binds a finished retained commit to its public receipt.
pub(in crate::operation) fn retained_report(
    apply: WorktreeApplyReport,
    receipt: &StoredReceipt,
) -> RetainedApplyReport {
    RetainedApplyReport {
        apply,
        receipt: receipt.public(),
    }
}

pub(crate) fn undo_receipts(
    root: &FsRoot,
    options: WorktreeOptions,
) -> Result<Vec<UndoReceipt>, WorktreeError> {
    let Some(session) = open_session(root)? else {
        return Ok(Vec::new());
    };
    let mut ids = session.control.undo_receipt_ids().map_err(session_io)?;
    ids.sort_unstable();
    let mut receipts = Vec::with_capacity(ids.len());
    for id in &ids {
        let id: UndoId = id.parse().map_err(|_| {
            WorktreeError::new(
                WorktreeErrorCode::UndoCorrupt,
                TransactionPhase::Validate,
                "stored undo receipt name is not a valid identifier",
            )
            .requiring_recovery()
        })?;
        receipts.push(inspect(&session.control, &id, options)?.public());
    }
    Ok(receipts)
}

pub(crate) fn undo_usage(
    root: &FsRoot,
    options: WorktreeOptions,
) -> Result<UndoStoreUsage, WorktreeError> {
    let Some(session) = open_session(root)? else {
        return Ok(UndoStoreUsage {
            receipts: 0,
            bytes: 0,
        });
    };
    store_usage(&session.control, options)
}

pub(crate) fn undo_rollback(
    root: &FsRoot,
    options: WorktreeOptions,
    id: &UndoId,
) -> Result<UndoRollbackReport, WorktreeError> {
    let Some(session) = open_session(root)? else {
        return Err(not_found());
    };
    let stored = inspect(&session.control, id, options)?;
    rollback(&session.control, root, &stored, options)
}

pub(crate) fn undo_discard(
    root: &FsRoot,
    options: WorktreeOptions,
    id: &UndoId,
) -> Result<usize, WorktreeError> {
    let Some(session) = open_session(root)? else {
        return Err(not_found());
    };
    let stored = inspect(&session.control, id, options)?;
    discard(&session.control, &stored, options)
}

fn open_session(root: &FsRoot) -> Result<Option<UndoSession>, WorktreeError> {
    let Some(control) = root.open_control(false).map_err(session_io)? else {
        return Ok(None);
    };
    let file = control.open_lock().map_err(session_io)?;
    match fs4::FileExt::try_lock(&file) {
        Ok(()) => {}
        Err(fs4::TryLockError::WouldBlock) => {
            return Err(WorktreeError::new(
                WorktreeErrorCode::RootBusy,
                TransactionPhase::Lock,
                "another worktree transaction holds the root lock",
            ));
        }
        Err(fs4::TryLockError::Error(error)) => return Err(session_io(error)),
    }
    let session = UndoSession {
        control,
        _lock: file,
    };
    crate::operation::pending::ensure_no_pending(&session.control)?;
    Ok(Some(session))
}

fn not_found() -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::UndoNotFound,
        TransactionPhase::Validate,
        "the exact undo receipt does not exist",
    )
}

fn session_io(error: std::io::Error) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::Io,
        TransactionPhase::Lock,
        "failed to open the retained undo store",
        error,
    )
}
