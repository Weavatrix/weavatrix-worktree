//! Bounded, deterministic scheduling for independent preparation jobs.

use std::{
    any::Any,
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

pub(crate) const HARD_MAX_WORKERS: usize = 16;
type JobResult<R, E> = Option<Result<R, ScheduleError<E>>>;

/// Result of a job that did not produce its requested value.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ScheduleError<E> {
    Operation(E),
    Panicked(String),
    Cancelled,
}

/// Resolves an explicit worker request against the job count and hard ceiling.
#[must_use]
pub(crate) fn bounded_worker_count(requested: usize, job_count: usize) -> usize {
    if job_count == 0 {
        0
    } else {
        requested.clamp(1, HARD_MAX_WORKERS).min(job_count)
    }
}

/// Maps independent jobs on scoped threads and returns one result per input.
///
/// Results retain input order regardless of completion order. The first failed
/// or panicking job requests cancellation; already-admitted jobs finish and all
/// workers are joined, while jobs not admitted are represented as `Cancelled`.
pub(crate) fn map_ordered<T, R, E, F>(
    items: Vec<T>,
    requested_workers: usize,
    operation: F,
) -> Vec<Result<R, ScheduleError<E>>>
where
    T: Send,
    R: Send,
    E: Send,
    F: Fn(usize, T) -> Result<R, E> + Sync,
{
    let job_count = items.len();
    let worker_count = bounded_worker_count(requested_workers, job_count);
    if worker_count == 0 {
        return Vec::new();
    }

    let queue = Mutex::new(items.into_iter().enumerate().collect::<VecDeque<_>>());
    let results = Mutex::new((0..job_count).map(|_| None).collect::<Vec<_>>());
    let cancelled = AtomicBool::new(false);

    thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| run_worker(&queue, &results, &cancelled, &operation));
        }
    });

    into_inner(results)
        .into_iter()
        .map(|result| result.unwrap_or(Err(ScheduleError::Cancelled)))
        .collect()
}

fn run_worker<T, R, E, F>(
    queue: &Mutex<VecDeque<(usize, T)>>,
    results: &Mutex<Vec<JobResult<R, E>>>,
    cancelled: &AtomicBool,
    operation: &F,
) where
    T: Send,
    R: Send,
    E: Send,
    F: Fn(usize, T) -> Result<R, E> + Sync,
{
    loop {
        let job = {
            let mut queue = lock(queue);
            if cancelled.load(Ordering::Acquire) {
                None
            } else {
                queue.pop_front()
            }
        };
        let Some((index, item)) = job else {
            return;
        };

        let outcome = run_job(index, item, operation);
        let failed = outcome.is_err();
        lock(results)[index] = Some(outcome);
        if failed {
            cancelled.store(true, Ordering::Release);
        }
    }
}

fn run_job<T, R, E, F>(index: usize, item: T, operation: &F) -> Result<R, ScheduleError<E>>
where
    F: Fn(usize, T) -> Result<R, E>,
{
    match catch_unwind(AssertUnwindSafe(|| operation(index, item))) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(ScheduleError::Operation(error)),
        Err(payload) => Err(ScheduleError::Panicked(panic_message(payload.as_ref()))),
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn into_inner<T>(mutex: Mutex<T>) -> T {
    mutex.into_inner().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use crate::scheduler::{HARD_MAX_WORKERS, ScheduleError, bounded_worker_count, map_ordered};

    #[test]
    fn worker_counts_are_bounded() {
        assert_eq!(bounded_worker_count(100, 100), HARD_MAX_WORKERS);
        assert_eq!(bounded_worker_count(0, 3), 1);
        assert_eq!(bounded_worker_count(4, 2), 2);
        assert_eq!(bounded_worker_count(4, 0), 0);
    }

    #[test]
    fn results_keep_input_order() {
        let results = map_ordered(vec![3, 1, 2], 3, |index, value| {
            Ok::<_, ()>((index, value * 2))
        });
        assert_eq!(results, [Ok((0, 6)), Ok((1, 2)), Ok((2, 4))]);
    }

    #[test]
    fn one_worker_cancels_unstarted_jobs_after_an_error() {
        let results = map_ordered(vec![1, 2, 3], 1, |index, value| {
            if index == 0 { Err(value) } else { Ok(value) }
        });
        assert_eq!(
            results,
            [
                Err(ScheduleError::Operation(1)),
                Err(ScheduleError::Cancelled),
                Err(ScheduleError::Cancelled),
            ]
        );
    }

    #[test]
    fn panic_payload_is_captured_per_job() {
        let results = map_ordered(vec![()], 1, |_, ()| -> Result<(), ()> {
            panic!("worker failed")
        });
        assert_eq!(
            results,
            [Err(ScheduleError::Panicked("worker failed".to_owned()))]
        );
    }
}
