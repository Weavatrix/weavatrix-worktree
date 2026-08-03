use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::ControlDir,
};

/// Rejects new work while any edit, operation, or undo journal is pending.
pub(super) fn ensure_no_pending(control: &ControlDir) -> Result<(), WorktreeError> {
    if control.open_journal().map_err(pending_error)?.is_some()
        || control
            .open_operation_journal()
            .map_err(pending_error)?
            .is_some()
        || control
            .open_undo_journal()
            .map_err(pending_error)?
            .is_some()
    {
        return Err(WorktreeError::new(
            WorktreeErrorCode::RecoveryRequired,
            TransactionPhase::Lock,
            "a previous transaction journal must be recovered first",
        )
        .requiring_recovery());
    }
    Ok(())
}

fn pending_error(error: std::io::Error) -> WorktreeError {
    let code = if error.kind() == std::io::ErrorKind::InvalidData {
        WorktreeErrorCode::JournalCorrupt
    } else {
        WorktreeErrorCode::Io
    };
    let mapped = WorktreeError::with_source(
        code,
        TransactionPhase::Lock,
        "failed to inspect pending transaction state",
        error,
    );
    if code == WorktreeErrorCode::JournalCorrupt {
        mapped.requiring_recovery()
    } else {
        mapped
    }
}
