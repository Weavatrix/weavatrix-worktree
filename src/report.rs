use core::fmt;

use crate::hash::Sha256Hash;

/// Deterministic before/after evidence for one repository-relative file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileChange {
    path: String,
    old_sha256: Sha256Hash,
    new_sha256: Sha256Hash,
    bytes_before: u64,
    bytes_after: u64,
    edits_applied: usize,
}

impl FileChange {
    pub(crate) fn new(
        path: impl Into<String>,
        old_sha256: Sha256Hash,
        new_sha256: Sha256Hash,
        bytes_before: u64,
        bytes_after: u64,
        edits_applied: usize,
    ) -> Self {
        Self {
            path: path.into(),
            old_sha256,
            new_sha256,
            bytes_before,
            bytes_after,
            edits_applied,
        }
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub const fn old_sha256(&self) -> Sha256Hash {
        self.old_sha256
    }

    #[must_use]
    pub const fn new_sha256(&self) -> Sha256Hash {
        self.new_sha256
    }

    #[must_use]
    pub const fn bytes_before(&self) -> u64 {
        self.bytes_before
    }

    #[must_use]
    pub const fn bytes_after(&self) -> u64 {
        self.bytes_after
    }

    #[must_use]
    pub const fn edits_applied(&self) -> usize {
        self.edits_applied
    }

    #[must_use]
    pub fn changed(&self) -> bool {
        self.old_sha256 != self.new_sha256
    }
}

/// Read-only result of validating and projecting an edit plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DryRunReport {
    operation: String,
    files: Vec<FileChange>,
}

impl DryRunReport {
    pub(crate) fn new(operation: impl Into<String>, files: Vec<FileChange>) -> Self {
        Self {
            operation: operation.into(),
            files,
        }
    }

    #[must_use]
    pub fn operation(&self) -> &str {
        &self.operation
    }

    #[must_use]
    pub fn files(&self) -> &[FileChange] {
        &self.files
    }

    #[must_use]
    pub fn total_edits(&self) -> usize {
        self.files
            .iter()
            .fold(0, |total, file| total.saturating_add(file.edits_applied))
    }

    #[must_use]
    pub fn total_bytes_before(&self) -> u64 {
        total_bytes(&self.files, FileChange::bytes_before)
    }

    #[must_use]
    pub fn total_bytes_after(&self) -> u64 {
        total_bytes(&self.files, FileChange::bytes_after)
    }
}

/// Successful durable commit report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyReport {
    transaction_id: String,
    operation: String,
    files: Vec<FileChange>,
}

impl ApplyReport {
    pub(crate) fn new(
        transaction_id: impl Into<String>,
        operation: impl Into<String>,
        files: Vec<FileChange>,
    ) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            operation: operation.into(),
            files,
        }
    }

    #[must_use]
    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    #[must_use]
    pub fn operation(&self) -> &str {
        &self.operation
    }

    #[must_use]
    pub fn files(&self) -> &[FileChange] {
        &self.files
    }

    #[must_use]
    pub fn total_edits(&self) -> usize {
        self.files
            .iter()
            .fold(0, |total, file| total.saturating_add(file.edits_applied))
    }
}

/// Successful explicit cancellation of a prepared transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbortReport {
    transaction_id: String,
    prepared_files: usize,
    artifacts_removed: usize,
}

impl AbortReport {
    pub(crate) fn new(
        transaction_id: impl Into<String>,
        prepared_files: usize,
        artifacts_removed: usize,
    ) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            prepared_files,
            artifacts_removed,
        }
    }

    #[must_use]
    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    #[must_use]
    pub const fn prepared_files(&self) -> usize {
        self.prepared_files
    }

    #[must_use]
    pub const fn artifacts_removed(&self) -> usize {
        self.artifacts_removed
    }
}

/// Completed action selected by deterministic journal recovery.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RecoveryAction {
    NoPendingTransaction,
    DiscardedStaging,
    RolledBack,
    FinishedCommitCleanup,
}

impl RecoveryAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoPendingTransaction => "NO_PENDING_TRANSACTION",
            Self::DiscardedStaging => "DISCARDED_STAGING",
            Self::RolledBack => "ROLLED_BACK",
            Self::FinishedCommitCleanup => "FINISHED_COMMIT_CLEANUP",
        }
    }
}

impl fmt::Display for RecoveryAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Successful recovery or recovery inspection result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryReport {
    transaction_id: Option<String>,
    action: RecoveryAction,
    files: Vec<FileChange>,
    artifacts_removed: usize,
}

impl RecoveryReport {
    pub(crate) fn new(
        transaction_id: Option<String>,
        action: RecoveryAction,
        files: Vec<FileChange>,
        artifacts_removed: usize,
    ) -> Self {
        Self {
            transaction_id,
            action,
            files,
            artifacts_removed,
        }
    }

    #[must_use]
    pub fn transaction_id(&self) -> Option<&str> {
        self.transaction_id.as_deref()
    }

    #[must_use]
    pub const fn action(&self) -> RecoveryAction {
        self.action
    }

    #[must_use]
    pub fn files(&self) -> &[FileChange] {
        &self.files
    }

    #[must_use]
    pub const fn artifacts_removed(&self) -> usize {
        self.artifacts_removed
    }
}

fn total_bytes(files: &[FileChange], projection: fn(&FileChange) -> u64) -> u64 {
    files
        .iter()
        .fold(0, |total, file| total.saturating_add(projection(file)))
}

#[cfg(test)]
mod tests {
    use crate::hash::Sha256Hash;
    use crate::report::{DryRunReport, FileChange, RecoveryAction};

    #[test]
    fn totals_and_wire_action_are_deterministic() {
        let old = Sha256Hash::compute(b"old");
        let new = Sha256Hash::compute(b"new");
        let report = DryRunReport::new(
            "rename",
            vec![FileChange::new("src/lib.rs", old, new, 3, 3, 2)],
        );
        assert_eq!(report.total_edits(), 2);
        assert_eq!(report.total_bytes_before(), 3);
        assert_eq!(RecoveryAction::RolledBack.as_str(), "ROLLED_BACK");
        assert!(report.files()[0].changed());
    }
}
