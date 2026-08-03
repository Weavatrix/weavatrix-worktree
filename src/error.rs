use core::fmt;

/// Stable transaction phase attached to every worktree failure.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TransactionPhase {
    Open,
    Validate,
    Lock,
    DryRun,
    Prepare,
    Stage,
    Commit,
    Rollback,
    Recover,
    Cleanup,
}

impl TransactionPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Validate => "VALIDATE",
            Self::Lock => "LOCK",
            Self::DryRun => "DRY_RUN",
            Self::Prepare => "PREPARE",
            Self::Stage => "STAGE",
            Self::Commit => "COMMIT",
            Self::Rollback => "ROLLBACK",
            Self::Recover => "RECOVER",
            Self::Cleanup => "CLEANUP",
        }
    }
}

impl fmt::Display for TransactionPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Stable machine-readable worktree failure categories.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum WorktreeErrorCode {
    InvalidRoot,
    InvalidOptions,
    RootBusy,
    RecoveryRequired,
    InvalidPlan,
    OperationConflict,
    PathExists,
    PathMissing,
    ReservedPath,
    PathEscape,
    SymlinkNotAllowed,
    CrossFilesystem,
    NotRegularFile,
    HardlinkNotAllowed,
    ReadOnlyFile,
    SourceTooLarge,
    TransactionTooLarge,
    NonUtf8Source,
    SourceHashMismatch,
    ConcurrentModification,
    EditRejected,
    StageFailed,
    DurabilityFailed,
    CommitFailed,
    RollbackFailed,
    JournalCorrupt,
    UndoNotFound,
    UndoConflict,
    UndoStoreFull,
    UndoCorrupt,
    UndoFailed,
    WorkerPanicked,
    Cancelled,
    Io,
}

impl WorktreeErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRoot => "INVALID_ROOT",
            Self::InvalidOptions => "INVALID_OPTIONS",
            Self::RootBusy => "ROOT_BUSY",
            Self::RecoveryRequired => "RECOVERY_REQUIRED",
            Self::InvalidPlan => "INVALID_PLAN",
            Self::OperationConflict => "OPERATION_CONFLICT",
            Self::PathExists => "PATH_EXISTS",
            Self::PathMissing => "PATH_MISSING",
            Self::ReservedPath => "RESERVED_PATH",
            Self::PathEscape => "PATH_ESCAPE",
            Self::SymlinkNotAllowed => "SYMLINK_NOT_ALLOWED",
            Self::CrossFilesystem => "CROSS_FILESYSTEM",
            Self::NotRegularFile => "NOT_REGULAR_FILE",
            Self::HardlinkNotAllowed => "HARDLINK_NOT_ALLOWED",
            Self::ReadOnlyFile => "READ_ONLY_FILE",
            Self::SourceTooLarge => "SOURCE_TOO_LARGE",
            Self::TransactionTooLarge => "TRANSACTION_TOO_LARGE",
            Self::NonUtf8Source => "NON_UTF8_SOURCE",
            Self::SourceHashMismatch => "SOURCE_HASH_MISMATCH",
            Self::ConcurrentModification => "CONCURRENT_MODIFICATION",
            Self::EditRejected => "EDIT_REJECTED",
            Self::StageFailed => "STAGE_FAILED",
            Self::DurabilityFailed => "DURABILITY_FAILED",
            Self::CommitFailed => "COMMIT_FAILED",
            Self::RollbackFailed => "ROLLBACK_FAILED",
            Self::JournalCorrupt => "JOURNAL_CORRUPT",
            Self::UndoNotFound => "UNDO_NOT_FOUND",
            Self::UndoConflict => "UNDO_CONFLICT",
            Self::UndoStoreFull => "UNDO_STORE_FULL",
            Self::UndoCorrupt => "UNDO_CORRUPT",
            Self::UndoFailed => "UNDO_FAILED",
            Self::WorkerPanicked => "WORKER_PANICKED",
            Self::Cancelled => "CANCELLED",
            Self::Io => "IO",
        }
    }
}

impl fmt::Display for WorktreeErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Structured worktree error with stable routing fields.
#[derive(Debug)]
pub struct WorktreeError {
    code: WorktreeErrorCode,
    phase: TransactionPhase,
    message: String,
    path: Option<String>,
    file_index: Option<usize>,
    transaction_id: Option<String>,
    recovery_required: bool,
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl WorktreeError {
    pub(crate) fn new(
        code: WorktreeErrorCode,
        phase: TransactionPhase,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            phase,
            message: message.into(),
            path: None,
            file_index: None,
            transaction_id: None,
            recovery_required: matches!(
                code,
                WorktreeErrorCode::RecoveryRequired | WorktreeErrorCode::RollbackFailed
            ),
            source: None,
        }
    }

    pub(crate) fn with_source<E>(
        code: WorktreeErrorCode,
        phase: TransactionPhase,
        message: impl Into<String>,
        source: E,
    ) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        let mut error = Self::new(code, phase, message);
        error.source = Some(Box::new(source));
        error
    }

    #[must_use]
    pub(crate) fn at_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    #[must_use]
    pub(crate) const fn at_file(mut self, file_index: usize) -> Self {
        self.file_index = Some(file_index);
        self
    }

    #[must_use]
    pub(crate) fn in_transaction(mut self, transaction_id: impl Into<String>) -> Self {
        self.transaction_id = Some(transaction_id.into());
        self
    }

    #[must_use]
    pub(crate) const fn requiring_recovery(mut self) -> Self {
        self.recovery_required = true;
        self
    }

    #[must_use]
    pub const fn code(&self) -> WorktreeErrorCode {
        self.code
    }

    #[must_use]
    pub const fn phase(&self) -> TransactionPhase {
        self.phase
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    #[must_use]
    pub const fn file_index(&self) -> Option<usize> {
        self.file_index
    }

    #[must_use]
    pub fn transaction_id(&self) -> Option<&str> {
        self.transaction_id.as_deref()
    }

    #[must_use]
    pub const fn recovery_required(&self) -> bool {
        self.recovery_required
    }
}

impl fmt::Display for WorktreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} during {}", self.code, self.phase)?;
        if let Some(path) = &self.path {
            write!(formatter, " at {path}")?;
        }
        if let Some(index) = self.file_index {
            write!(formatter, " [file {index}]")?;
        }
        write!(formatter, ": {}", self.message)
    }
}

impl std::error::Error for WorktreeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use crate::error::{TransactionPhase, WorktreeError, WorktreeErrorCode};

    #[test]
    fn stable_fields_survive_context_builders() {
        let error = WorktreeError::new(
            WorktreeErrorCode::CommitFailed,
            TransactionPhase::Commit,
            "replace failed",
        )
        .at_path("src/lib.rs")
        .at_file(2)
        .in_transaction("abc")
        .requiring_recovery();

        assert_eq!(error.code().as_str(), "COMMIT_FAILED");
        assert_eq!(error.path(), Some("src/lib.rs"));
        assert_eq!(error.file_index(), Some(2));
        assert_eq!(error.transaction_id(), Some("abc"));
        assert!(error.recovery_required());
    }
}
