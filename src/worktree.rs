use std::path::Path;

use weavatrix_refactor_plan::EditPlan;

use crate::{
    WorktreePlan,
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::FsRoot,
    operation::{
        PreparedWorktreeTransaction, RetainedApplyReport, UndoId, UndoReceipt, UndoRetention,
        UndoRollbackReport, UndoStoreUsage, dry_run_operation_plan, prepare_operation_plan,
        recover_operation_transaction, undo_discard, undo_receipts, undo_rollback, undo_usage,
    },
    options::WorktreeOptions,
    report::{
        ApplyReport, DryRunReport, RecoveryReport, WorktreeApplyReport, WorktreeDryRunReport,
    },
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

    /// Validates and projects create/delete/modify/rename operations without writing state.
    pub fn dry_run_plan(&self, plan: &WorktreePlan) -> Result<WorktreeDryRunReport, WorktreeError> {
        dry_run_operation_plan(&self.root, self.options, plan)
    }

    /// Validates, locks, stages, and journals every submitted refactor operation.
    pub fn prepare_plan(
        &self,
        plan: &WorktreePlan,
    ) -> Result<PreparedWorktreeTransaction, WorktreeError> {
        prepare_operation_plan(&self.root, self.options, plan)
    }

    /// Prepares and durably commits every submitted filesystem operation.
    pub fn apply_plan(&self, plan: &WorktreePlan) -> Result<WorktreeApplyReport, WorktreeError> {
        self.prepare_plan(plan)?.commit()
    }

    /// Commits a plan while retaining exact rollback evidence and a receipt.
    pub fn apply_plan_retained(
        &self,
        plan: &WorktreePlan,
        retention: UndoRetention,
    ) -> Result<RetainedApplyReport, WorktreeError> {
        self.prepare_plan(plan)?.commit_retained(retention)
    }

    /// Lists every retained undo receipt in deterministic identifier order.
    pub fn undo_receipts(&self) -> Result<Vec<UndoReceipt>, WorktreeError> {
        undo_receipts(&self.root, self.options)
    }

    /// Reports the bounded usage of the retained undo store.
    pub fn undo_usage(&self) -> Result<UndoStoreUsage, WorktreeError> {
        undo_usage(&self.root, self.options)
    }

    /// Exactly restores the state captured by one retained commit.
    pub fn rollback_undo(&self, id: &UndoId) -> Result<UndoRollbackReport, WorktreeError> {
        undo_rollback(&self.root, self.options, id)
    }

    /// Verifies and removes one retained receipt without changing any target.
    pub fn discard_undo(&self, id: &UndoId) -> Result<usize, WorktreeError> {
        undo_discard(&self.root, self.options, id)
    }

    /// Replays and safely resolves an interrupted transaction journal.
    pub fn recover(&self) -> Result<RecoveryReport, WorktreeError> {
        if let Some(report) = recover_operation_transaction(&self.root, self.options)? {
            return Ok(report);
        }
        recover_transaction(&self.root, self.options)
    }
}
