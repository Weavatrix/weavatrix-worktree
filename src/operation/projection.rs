use std::sync::Arc;

use weavatrix_refactor_plan::ValidatedExecutorPlan;

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{PresentEvidence, SlotSnapshot},
    hash::Sha256Hash,
    options::WorktreeOptions,
    plan::{PlannedInput, compile_plan},
    report::WorktreeDryRunReport,
    scheduler::{ScheduleError, map_ordered},
};

use super::model::{ProjectedInput, ProjectedPlan};

mod assemble;
mod changes;

use assemble::assemble;

struct SnapshotJob {
    stable_index: u32,
    transition: crate::plan::PathTransition,
    access: crate::filesystem::TargetAccess,
}

struct SnapshotResult {
    stable_index: u32,
    transition: crate::plan::PathTransition,
    access: crate::filesystem::TargetAccess,
    before: ProjectedInput,
}

pub(super) fn project(
    root: &crate::filesystem::FsRoot,
    options: WorktreeOptions,
    validated: &ValidatedExecutorPlan<'_>,
) -> Result<ProjectedPlan, WorktreeError> {
    let plan = validated.plan();
    let transitions = compile_plan(plan)?;
    let mut jobs = Vec::with_capacity(transitions.len());
    for (stable_index, transition) in transitions.into_iter().enumerate() {
        let stable_index = u32::try_from(stable_index)
            .map_err(|_| too_large("path index does not fit the transaction contract"))?;
        let access = root.open_target(&transition.path).map_err(|error| {
            path_io(
                TransactionPhase::Validate,
                &transition.path,
                "failed to open a confined path slot",
                error,
            )
        })?;
        jobs.push(SnapshotJob {
            stable_index,
            transition,
            access,
        });
    }

    let workers = options.worker_count(jobs.len());
    let results = map_ordered(jobs, workers, |_, job| snapshot(job, options));
    let snapshots = collect(results)?;
    assemble(plan, snapshots, options)
}

pub(super) fn preview(projected: &ProjectedPlan) -> WorktreeDryRunReport {
    WorktreeDryRunReport::new(
        projected.operation.clone(),
        projected.operations.clone(),
        projected.paths.len(),
    )
}

fn snapshot(job: SnapshotJob, options: WorktreeOptions) -> Result<SnapshotResult, WorktreeError> {
    let observed = job
        .access
        .snapshot_slot(options.limits.max_source_bytes_per_file)
        .map_err(|error| {
            path_io(
                TransactionPhase::Prepare,
                &job.transition.path,
                "failed to snapshot an exact path slot",
                error,
            )
        })?;
    let before = match (&job.transition.before, observed) {
        (PlannedInput::Absent, SlotSnapshot::Absent) => ProjectedInput::Absent,
        (PlannedInput::Absent, SlotSnapshot::Present(_)) => {
            return Err(WorktreeError::new(
                WorktreeErrorCode::PathExists,
                TransactionPhase::Prepare,
                "path must be absent before this operation",
            )
            .at_path(job.transition.path));
        }
        (
            PlannedInput::Present {
                operation_index, ..
            },
            SlotSnapshot::Absent,
        ) => {
            return Err(WorktreeError::new(
                WorktreeErrorCode::PathMissing,
                TransactionPhase::Prepare,
                "path must exist before this operation",
            )
            .at_path(job.transition.path)
            .at_file(*operation_index));
        }
        (
            PlannedInput::Present {
                operation_index,
                expected_sha256,
                ..
            },
            SlotSnapshot::Present(snapshot),
        ) => {
            let actual = Sha256Hash::compute(&snapshot.source);
            if actual != *expected_sha256 {
                return Err(WorktreeError::new(
                    WorktreeErrorCode::SourceHashMismatch,
                    TransactionPhase::Prepare,
                    format!("expected {expected_sha256}, found {actual}"),
                )
                .at_path(job.transition.path)
                .at_file(*operation_index));
            }
            let source = String::from_utf8(snapshot.source).map_err(|error| {
                WorktreeError::with_source(
                    WorktreeErrorCode::NonUtf8Source,
                    TransactionPhase::Prepare,
                    "operation source is not valid UTF-8",
                    error,
                )
                .at_path(job.transition.path.clone())
                .at_file(*operation_index)
            })?;
            let bytes = source.len() as u64;
            ProjectedInput::Present {
                source: Arc::from(source),
                evidence: PresentEvidence {
                    sha256: actual,
                    bytes,
                    identity: snapshot.identity,
                    permissions: snapshot.portable_permissions,
                },
            }
        }
    };
    Ok(SnapshotResult {
        stable_index: job.stable_index,
        transition: job.transition,
        access: job.access,
        before,
    })
}

fn collect(
    results: Vec<Result<SnapshotResult, ScheduleError<WorktreeError>>>,
) -> Result<Vec<SnapshotResult>, WorktreeError> {
    results
        .into_iter()
        .enumerate()
        .map(|(index, result)| match result {
            Ok(value) => Ok(value),
            Err(ScheduleError::Operation(error)) => Err(error),
            Err(ScheduleError::Panicked(message)) => Err(WorktreeError::new(
                WorktreeErrorCode::WorkerPanicked,
                TransactionPhase::Prepare,
                message,
            )
            .at_file(index)),
            Err(ScheduleError::Cancelled) => Err(WorktreeError::new(
                WorktreeErrorCode::Cancelled,
                TransactionPhase::Prepare,
                "projection was cancelled after an earlier worker failure",
            )
            .at_file(index)),
        })
        .collect()
}

fn path_io(
    phase: TransactionPhase,
    path: &str,
    message: &str,
    source: std::io::Error,
) -> WorktreeError {
    let detail = source.to_string();
    let code = match source.kind() {
        std::io::ErrorKind::NotFound => WorktreeErrorCode::PathMissing,
        std::io::ErrorKind::AlreadyExists => WorktreeErrorCode::PathExists,
        _ if detail.contains("reserved") || detail.contains("state namespace") => {
            WorktreeErrorCode::ReservedPath
        }
        _ if detail.contains("symbolic link") || detail.contains("reparse") => {
            WorktreeErrorCode::SymlinkNotAllowed
        }
        _ if detail.contains("crosses") => WorktreeErrorCode::CrossFilesystem,
        _ if detail.contains("not a regular file") => WorktreeErrorCode::NotRegularFile,
        _ if detail.contains("hard-linked") => WorktreeErrorCode::HardlinkNotAllowed,
        _ if detail.contains("read-only") => WorktreeErrorCode::ReadOnlyFile,
        _ if detail.contains("byte limit") || detail.contains("exceeds") => {
            WorktreeErrorCode::SourceTooLarge
        }
        _ => WorktreeErrorCode::Io,
    };
    WorktreeError::with_source(code, phase, message, source).at_path(path.to_owned())
}

fn invalid_internal(message: &str, index: usize) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::InvalidPlan,
        TransactionPhase::Validate,
        message,
    )
    .at_file(index)
}

fn too_large(message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::TransactionTooLarge,
        TransactionPhase::Validate,
        message,
    )
}
