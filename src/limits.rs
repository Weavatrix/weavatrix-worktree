use crate::error::{TransactionPhase, WorktreeError, WorktreeErrorCode};

const MIB: usize = 1024 * 1024;

/// Absolute worker ceiling retained even when callers raise transaction limits.
pub const ABSOLUTE_MAX_WORKERS: usize = 16;

/// Absolute journal ceiling retained even when callers customize limits.
pub const ABSOLUTE_MAX_JOURNAL_BYTES: usize = 16 * MIB;

/// Hard resource ceilings for one worktree transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorktreeLimits {
    pub max_files: usize,
    pub max_edits_per_file: usize,
    pub max_source_bytes_per_file: usize,
    pub max_output_bytes_per_file: usize,
    pub max_total_source_bytes: usize,
    pub max_total_output_bytes: usize,
    pub max_total_artifact_bytes: usize,
    pub max_journal_bytes: usize,
    pub max_workers: usize,
}

impl Default for WorktreeLimits {
    fn default() -> Self {
        Self {
            max_files: 64,
            max_edits_per_file: 2_000,
            max_source_bytes_per_file: 16 * MIB,
            max_output_bytes_per_file: 64 * MIB,
            max_total_source_bytes: 128 * MIB,
            max_total_output_bytes: 256 * MIB,
            max_total_artifact_bytes: 384 * MIB,
            max_journal_bytes: MIB,
            max_workers: 16,
        }
    }
}

impl WorktreeLimits {
    /// Validates that every limit is nonzero and internally consistent.
    pub fn validate(&self) -> Result<(), WorktreeError> {
        for (name, value) in [
            ("max_files", self.max_files),
            ("max_edits_per_file", self.max_edits_per_file),
            ("max_source_bytes_per_file", self.max_source_bytes_per_file),
            ("max_output_bytes_per_file", self.max_output_bytes_per_file),
            ("max_total_source_bytes", self.max_total_source_bytes),
            ("max_total_output_bytes", self.max_total_output_bytes),
            ("max_total_artifact_bytes", self.max_total_artifact_bytes),
            ("max_journal_bytes", self.max_journal_bytes),
            ("max_workers", self.max_workers),
        ] {
            if value == 0 {
                return Err(invalid(format!("{name} must be nonzero")));
            }
        }

        if self.max_workers > ABSOLUTE_MAX_WORKERS {
            return Err(invalid(format!(
                "max_workers may not exceed {ABSOLUTE_MAX_WORKERS}"
            )));
        }
        if self.max_journal_bytes > ABSOLUTE_MAX_JOURNAL_BYTES {
            return Err(invalid(format!(
                "max_journal_bytes may not exceed {ABSOLUTE_MAX_JOURNAL_BYTES}"
            )));
        }
        if self.max_total_source_bytes < self.max_source_bytes_per_file {
            return Err(invalid(
                "max_total_source_bytes must cover one maximum-size source",
            ));
        }
        if self.max_total_output_bytes < self.max_output_bytes_per_file {
            return Err(invalid(
                "max_total_output_bytes must cover one maximum-size output",
            ));
        }

        let required_artifacts = self
            .max_total_source_bytes
            .checked_add(self.max_total_output_bytes)
            .ok_or_else(|| invalid("source and output transaction limits overflow"))?;
        if self.max_total_artifact_bytes < required_artifacts {
            return Err(invalid(
                "max_total_artifact_bytes must cover total source plus output bytes",
            ));
        }
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::InvalidOptions,
        TransactionPhase::Validate,
        message,
    )
}

#[cfg(test)]
mod tests {
    use crate::limits::{ABSOLUTE_MAX_WORKERS, WorktreeLimits};

    #[test]
    fn defaults_match_the_transaction_contract() {
        let limits = WorktreeLimits::default();
        assert_eq!(limits.max_files, 64);
        assert_eq!(limits.max_workers, ABSOLUTE_MAX_WORKERS);
        assert_eq!(limits.max_total_artifact_bytes, 384 * 1024 * 1024);
        assert!(limits.validate().is_ok());
    }

    #[test]
    fn rejects_inconsistent_and_absolute_limits() {
        let mut limits = WorktreeLimits::default();
        limits.max_total_source_bytes = limits.max_source_bytes_per_file - 1;
        assert!(limits.validate().is_err());

        let limits = WorktreeLimits {
            max_workers: ABSOLUTE_MAX_WORKERS + 1,
            ..WorktreeLimits::default()
        };
        assert!(limits.validate().is_err());
    }
}
