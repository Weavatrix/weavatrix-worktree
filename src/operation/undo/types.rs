use core::{fmt, str::FromStr};

use crate::{Sha256Hash, WorktreeApplyReport};

/// Exact opaque identifier of one retained worktree commit.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UndoId(String);

impl UndoId {
    pub(super) fn from_transaction(value: String) -> Self {
        debug_assert!(is_valid_id(&value));
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for UndoId {
    type Err = ParseUndoIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        is_valid_id(value)
            .then(|| Self(value.to_owned()))
            .ok_or(ParseUndoIdError)
    }
}

impl fmt::Display for UndoId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for UndoId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("UndoId").field(&self.0).finish()
    }
}

/// A malformed undo identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseUndoIdError;

impl fmt::Display for ParseUndoIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("undo id must contain 32 lowercase hexadecimal characters")
    }
}

impl std::error::Error for ParseUndoIdError {}

fn is_valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Caller-selected ceiling for one retained commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UndoRetention {
    max_receipts: usize,
    max_bytes: usize,
}

impl UndoRetention {
    #[must_use]
    pub const fn new(max_receipts: usize, max_bytes: usize) -> Self {
        Self {
            max_receipts,
            max_bytes,
        }
    }

    #[must_use]
    pub const fn max_receipts(self) -> usize {
        self.max_receipts
    }

    #[must_use]
    pub const fn max_bytes(self) -> usize {
        self.max_bytes
    }
}

impl Default for UndoRetention {
    fn default() -> Self {
        Self::new(32, 384 * 1024 * 1024)
    }
}

/// Stable digest of ordered path names and semantic slot evidence.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorktreeSnapshotFingerprint(Sha256Hash);

impl WorktreeSnapshotFingerprint {
    pub(super) const fn new(value: Sha256Hash) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn sha256(self) -> Sha256Hash {
        self.0
    }
}

impl fmt::Display for WorktreeSnapshotFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Public, artifact-free metadata for one exact retained commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UndoReceipt {
    pub(super) id: UndoId,
    pub(super) plan_fingerprint: Sha256Hash,
    pub(super) before: WorktreeSnapshotFingerprint,
    pub(super) after: WorktreeSnapshotFingerprint,
    pub(super) touched_paths: usize,
    pub(super) retained_bytes: u64,
}

impl UndoReceipt {
    #[must_use]
    pub fn id(&self) -> &UndoId {
        &self.id
    }
    #[must_use]
    pub const fn plan_fingerprint(&self) -> Sha256Hash {
        self.plan_fingerprint
    }
    #[must_use]
    pub const fn before_fingerprint(&self) -> WorktreeSnapshotFingerprint {
        self.before
    }
    #[must_use]
    pub const fn after_fingerprint(&self) -> WorktreeSnapshotFingerprint {
        self.after
    }
    #[must_use]
    pub const fn touched_paths(&self) -> usize {
        self.touched_paths
    }
    #[must_use]
    pub const fn retained_bytes(&self) -> u64 {
        self.retained_bytes
    }
}

/// Successful commit plus its exact rollback binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedApplyReport {
    pub(super) apply: WorktreeApplyReport,
    pub(super) receipt: UndoReceipt,
}

impl RetainedApplyReport {
    #[must_use]
    pub const fn apply(&self) -> &WorktreeApplyReport {
        &self.apply
    }
    #[must_use]
    pub const fn receipt(&self) -> &UndoReceipt {
        &self.receipt
    }
    #[must_use]
    pub fn undo_id(&self) -> &UndoId {
        self.receipt.id()
    }
}

/// Current bounded retained-store usage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UndoStoreUsage {
    pub(super) receipts: usize,
    pub(super) bytes: u64,
}

impl UndoStoreUsage {
    #[must_use]
    pub const fn receipts(self) -> usize {
        self.receipts
    }
    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.bytes
    }
}

/// Successful exact rollback of one retained commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UndoRollbackReport {
    pub(super) undo_id: UndoId,
    pub(super) rollback_transaction_id: String,
    pub(super) restored_paths: usize,
    pub(super) artifacts_removed: usize,
}

impl UndoRollbackReport {
    #[must_use]
    pub fn undo_id(&self) -> &UndoId {
        &self.undo_id
    }
    #[must_use]
    pub fn rollback_transaction_id(&self) -> &str {
        &self.rollback_transaction_id
    }
    #[must_use]
    pub const fn restored_paths(&self) -> usize {
        self.restored_paths
    }
    #[must_use]
    pub const fn artifacts_removed(&self) -> usize {
        self.artifacts_removed
    }
}
