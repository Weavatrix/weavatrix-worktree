use std::path::Path;

use weavatrix_edit::EditPlan;

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::FsRoot,
    options::WorktreeOptions,
    report::{ApplyReport, DryRunReport, RecoveryReport},
    transaction::{
        PreparedTransaction, dry_run_report, prepare_transaction, project_plan, recover_transaction,
    },
};

/// Capability-rooted facade for deterministic multi-file edit transactions.
pub struct Worktree {
    root: FsRoot,
    options: WorktreeOptions,
}

impl Worktree {
    /// Opens a worktree with bounded default limits and automatic parallelism.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, WorktreeError> {
        Self::open_with(root, WorktreeOptions::default())
    }

    /// Opens a worktree with explicit limits and preparation parallelism.
    pub fn open_with(
        root: impl AsRef<Path>,
        options: WorktreeOptions,
    ) -> Result<Self, WorktreeError> {
        options.validate()?;
        let root = FsRoot::open(root.as_ref()).map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::InvalidRoot,
                TransactionPhase::Open,
                "failed to open a real worktree root capability",
                error,
            )
        })?;
        Ok(Self { root, options })
    }

    #[must_use]
    pub const fn options(&self) -> WorktreeOptions {
        self.options
    }

    /// Validates, reads, hashes, and projects a plan without creating state.
    pub fn dry_run(&self, plan: &EditPlan) -> Result<DryRunReport, WorktreeError> {
        let projected = project_plan(&self.root, self.options, plan)?;
        Ok(dry_run_report(plan, &projected))
    }

    /// Locks, validates, backs up, stages, and journals the complete plan.
    pub fn prepare(&self, plan: &EditPlan) -> Result<PreparedTransaction, WorktreeError> {
        prepare_transaction(&self.root, self.options, plan)
    }

    /// Prepares and commits a complete plan.
    pub fn apply(&self, plan: &EditPlan) -> Result<ApplyReport, WorktreeError> {
        self.prepare(plan)?.commit()
    }

    /// Replays and safely resolves an interrupted transaction journal.
    pub fn recover(&self) -> Result<RecoveryReport, WorktreeError> {
        recover_transaction(&self.root, self.options)
    }
}
