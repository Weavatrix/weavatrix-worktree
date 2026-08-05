use core::fmt;

use crate::hash::Sha256Hash;

/// Logical filesystem operation represented by a worktree plan.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationKind {
    Modify,
    Create,
    Delete,
    Rename,
}

impl OperationKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Modify => "MODIFY",
            Self::Create => "CREATE",
            Self::Delete => "DELETE",
            Self::Rename => "RENAME",
        }
    }
}

impl fmt::Display for OperationKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Deterministic before/after evidence for one logical filesystem operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationChange {
    kind: OperationKind,
    source_path: Option<String>,
    destination_path: Option<String>,
    old_sha256: Option<Sha256Hash>,
    new_sha256: Option<Sha256Hash>,
    bytes_before: u64,
    bytes_after: u64,
    edits_applied: usize,
}

impl OperationChange {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        kind: OperationKind,
        source_path: Option<String>,
        destination_path: Option<String>,
        old_sha256: Option<Sha256Hash>,
        new_sha256: Option<Sha256Hash>,
        bytes_before: u64,
        bytes_after: u64,
        edits_applied: usize,
    ) -> Self {
        Self {
            kind,
            source_path,
            destination_path,
            old_sha256,
            new_sha256,
            bytes_before,
            bytes_after,
            edits_applied,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> OperationKind {
        self.kind
    }

    #[must_use]
    pub fn source_path(&self) -> Option<&str> {
        self.source_path.as_deref()
    }

    #[must_use]
    pub fn destination_path(&self) -> Option<&str> {
        self.destination_path.as_deref()
    }

    #[must_use]
    pub const fn old_sha256(&self) -> Option<Sha256Hash> {
        self.old_sha256
    }

    #[must_use]
    pub const fn new_sha256(&self) -> Option<Sha256Hash> {
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
}

/// Side-effect-free projection of a resource-operation plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeDryRunReport {
    operation: String,
    files: Vec<OperationChange>,
    touched_paths: usize,
}

impl WorktreeDryRunReport {
    pub(crate) fn new(
        operation: impl Into<String>,
        files: Vec<OperationChange>,
        touched_paths: usize,
    ) -> Self {
        Self {
            operation: operation.into(),
            files,
            touched_paths,
        }
    }

    #[must_use]
    pub fn operation(&self) -> &str {
        &self.operation
    }

    #[must_use]
    pub fn files(&self) -> &[OperationChange] {
        &self.files
    }

    #[must_use]
    pub const fn touched_paths(&self) -> usize {
        self.touched_paths
    }

    #[must_use]
    pub fn total_edits(&self) -> usize {
        self.files
            .iter()
            .fold(0, |total, file| total.saturating_add(file.edits_applied()))
    }
}

/// Successful durable commit of a resource-operation plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeApplyReport {
    transaction_id: String,
    operation: String,
    files: Vec<OperationChange>,
    touched_paths: usize,
}

impl WorktreeApplyReport {
    pub(crate) fn new(
        transaction_id: impl Into<String>,
        operation: impl Into<String>,
        files: Vec<OperationChange>,
        touched_paths: usize,
    ) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            operation: operation.into(),
            files,
            touched_paths,
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
    pub fn files(&self) -> &[OperationChange] {
        &self.files
    }

    #[must_use]
    pub const fn touched_paths(&self) -> usize {
        self.touched_paths
    }
}
