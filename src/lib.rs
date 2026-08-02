#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod edit;
mod error;
mod filesystem;
mod hash;
mod journal;
mod limits;
mod options;
mod report;
mod scheduler;
mod transaction;
mod worktree;

pub use error::{TransactionPhase, WorktreeError, WorktreeErrorCode};
pub use hash::{ParseSha256Error, Sha256Hash};
pub use limits::WorktreeLimits;
pub use options::WorktreeOptions;
pub use report::{
    AbortReport, ApplyReport, DryRunReport, FileChange, RecoveryAction, RecoveryReport,
};
pub use transaction::PreparedTransaction;
pub use weavatrix_edit::{EditPlan, FileEdit};
pub use worktree::Worktree;

/// Crate version compiled into this library.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
