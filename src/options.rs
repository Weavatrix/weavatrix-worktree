use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    limits::WorktreeLimits,
};

/// Automatic preparation never starts more than this many workers by default.
pub const DEFAULT_AUTO_WORKERS: usize = 4;

/// Runtime policy for opening and preparing a worktree transaction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorktreeOptions {
    pub limits: WorktreeLimits,
    /// Zero selects bounded automatic parallelism.
    pub parallelism: usize,
}

impl WorktreeOptions {
    #[must_use]
    pub const fn with_limits(mut self, limits: WorktreeLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Sets preparation workers. Zero restores bounded automatic selection.
    #[must_use]
    pub const fn with_parallelism(mut self, parallelism: usize) -> Self {
        self.parallelism = parallelism;
        self
    }

    pub fn validate(&self) -> Result<(), WorktreeError> {
        self.limits.validate()?;
        if self.parallelism > self.limits.max_workers {
            return Err(WorktreeError::new(
                WorktreeErrorCode::InvalidOptions,
                TransactionPhase::Validate,
                format!(
                    "parallelism {} exceeds max_workers {}",
                    self.parallelism, self.limits.max_workers
                ),
            ));
        }
        Ok(())
    }

    /// Resolves the bounded worker count for a known number of files.
    #[must_use]
    pub fn worker_count(&self, file_count: usize) -> usize {
        if file_count == 0 {
            return 0;
        }
        let requested = if self.parallelism == 0 {
            std::thread::available_parallelism()
                .map_or(1, core::num::NonZeroUsize::get)
                .min(DEFAULT_AUTO_WORKERS)
        } else {
            self.parallelism
        };
        requested
            .max(1)
            .min(self.limits.max_workers)
            .min(file_count)
    }
}

#[cfg(test)]
mod tests {
    use crate::options::{DEFAULT_AUTO_WORKERS, WorktreeOptions};

    #[test]
    fn automatic_and_explicit_parallelism_are_bounded() {
        let automatic = WorktreeOptions::default();
        assert!((1..=DEFAULT_AUTO_WORKERS).contains(&automatic.worker_count(10)));
        assert_eq!(automatic.worker_count(0), 0);

        let explicit = automatic.with_parallelism(8);
        assert_eq!(explicit.worker_count(5), 5);
        assert!(explicit.validate().is_ok());
    }
}
