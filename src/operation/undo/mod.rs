mod api;
mod receipt;
mod recovery;
mod rollback;
mod types;

#[cfg(test)]
mod tests;

pub(in crate::operation) use api::retained_report;
pub(crate) use api::{undo_discard, undo_receipts, undo_rollback, undo_usage};
pub(super) use receipt::{
    StoredReceipt, discard, inspect, prepare_retention, read_exact, relocate_backups, store_usage,
};
pub(in crate::operation) use recovery::recover_undo;
pub(super) use rollback::rollback;
pub use types::{
    ParseUndoIdError, RetainedApplyReport, UndoId, UndoReceipt, UndoRetention, UndoRollbackReport,
    UndoStoreUsage, WorktreeSnapshotFingerprint,
};
