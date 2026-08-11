use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{ControlDir, FsRoot},
};

pub(crate) struct TransactionLock {
    pub(crate) control: ControlDir,
    pub(crate) file: LockGuard,
}

/// Releases the advisory root lock explicitly before the file handle closes.
///
/// A duplicated or inherited handle can keep the same open file description
/// alive on Unix. `flock(LOCK_UN)` is shared by those duplicates, while merely
/// dropping one handle is not enough.
pub(crate) struct LockGuard(std::fs::File);

impl LockGuard {
    const fn new(file: std::fs::File) -> Self {
        Self(file)
    }

    #[cfg(test)]
    pub(crate) fn try_clone(&self) -> std::io::Result<std::fs::File> {
        self.0.try_clone()
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs4::FileExt::unlock(&self.0);
    }
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
        Ok(()) => Ok(TransactionLock {
            control,
            file: LockGuard::new(file),
        }),
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
