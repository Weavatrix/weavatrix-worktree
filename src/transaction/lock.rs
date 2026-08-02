use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{ControlDir, FsRoot},
};

pub(crate) struct TransactionLock {
    pub(crate) control: ControlDir,
    pub(crate) file: std::fs::File,
}

pub(crate) fn acquire(root: &FsRoot) -> Result<TransactionLock, WorktreeError> {
    let control = root
        .open_control(true)
        .map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::Io,
                TransactionPhase::Lock,
                "failed to open the worktree control directory",
                error,
            )
        })?
        .ok_or_else(|| {
            WorktreeError::new(
                WorktreeErrorCode::Io,
                TransactionPhase::Lock,
                "worktree control directory was unavailable after creation",
            )
        })?;
    let file = control.open_lock().map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::Io,
            TransactionPhase::Lock,
            "failed to open the worktree lock",
            error,
        )
    })?;
    match fs4::FileExt::try_lock(&file) {
        Ok(()) => Ok(TransactionLock { control, file }),
        Err(fs4::TryLockError::WouldBlock) => Err(WorktreeError::new(
            WorktreeErrorCode::RootBusy,
            TransactionPhase::Lock,
            "another worktree transaction holds the root lock",
        )),
        Err(fs4::TryLockError::Error(error)) => Err(WorktreeError::with_source(
            WorktreeErrorCode::Io,
            TransactionPhase::Lock,
            "failed to acquire the worktree lock",
            error,
        )),
    }
}
