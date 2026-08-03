mod commit;
mod lock;
mod plan;
mod prepare;
mod recovery;
mod recovery_model;
#[cfg(test)]
mod recovery_tests;
mod rollback;
mod stage;
mod stage_cleanup;
mod util;
mod verify;

use crate::{
    filesystem::ControlDir, journal::JournalWriter, options::WorktreeOptions, report::DryRunReport,
};

use stage::StagedFile;

/// A durably staged transaction holding the exclusive worktree lock.
#[must_use = "commit, abort, or later recover the prepared transaction"]
pub struct PreparedTransaction {
    transaction_id: String,
    operation: String,
    preview: DryRunReport,
    files: Vec<StagedFile>,
    options: WorktreeOptions,
    journal: JournalWriter,
    control: ControlDir,
    _lock: std::fs::File,
}

impl core::fmt::Debug for PreparedTransaction {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PreparedTransaction")
            .field("transaction_id", &self.transaction_id)
            .field("operation", &self.operation)
            .field("prepared_files", &self.files.len())
            .finish_non_exhaustive()
    }
}

impl PreparedTransaction {
    #[must_use]
    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    #[must_use]
    pub const fn preview(&self) -> &DryRunReport {
        &self.preview
    }
}

pub(crate) use lock::acquire;
pub(crate) use plan::{dry_run_report, project_plan};
pub(crate) use prepare::prepare_transaction;
pub(crate) use recovery::recover_transaction;
