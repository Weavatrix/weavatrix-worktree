#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod error;
mod hash;
mod metadata;
mod plan;

mod edit;
mod filesystem;
mod journal;
mod limits;
mod operation;
mod options;
mod report;
mod scheduler;
mod transaction;
mod worktree;

pub use error::{TransactionPhase, WorktreeError, WorktreeErrorCode};
pub use hash::{ParseSha256Error, Sha256Hash};
pub use limits::WorktreeLimits;
pub use operation::{
    ParseUndoIdError, PreparedWorktreeTransaction, RetainedApplyReport, UndoId, UndoReceipt,
    UndoRetention, UndoRollbackReport, UndoStoreUsage, WorktreeSnapshotFingerprint,
};
pub use options::WorktreeOptions;
pub use report::{
    AbortReport, ApplyReport, DryRunReport, FileChange, OperationChange, OperationKind,
    RecoveryAction, RecoveryReport, WorktreeApplyReport, WorktreeDryRunReport,
};
pub use transaction::PreparedTransaction;
pub use weavatrix_refactor_plan::{
    CreateFile, CreatePermissions, DeleteFile, REFACTOR_PLAN_SCHEMA, RefactorOperation,
    RefactorPlan, RefactorPlanLimits, RenameFile,
};
pub use weavatrix_refactor_plan::{EditPlan, FileEdit};
pub use weavatrix_refactor_plan::{
    REFACTOR_PLAN_SCHEMA as WORKTREE_PLAN_SCHEMA, RefactorOperation as WorktreeOperation,
    RefactorPlan as WorktreePlan, RefactorPlanLimits as WorktreePlanLimits,
};
pub use worktree::Worktree;

/// Crate version compiled into this library.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
