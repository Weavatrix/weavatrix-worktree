use std::collections::HashMap;

use cap_std::fs::Permissions;
use weavatrix_refactor_plan::{EditPlan, FileEdit, PlanLimits};

use crate::{
    edit::{ProjectedFile, project_file},
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{FileIdentity, FsRoot, TargetAccess},
    options::WorktreeOptions,
    report::{DryRunReport, FileChange},
    scheduler::{ScheduleError, map_ordered},
};

use super::util::{checked_usize, fs_error};

mod metadata;

use metadata::validate_metadata;

pub(crate) struct ProjectedTarget {
    pub(crate) original_index: usize,
    pub(crate) file: FileEdit,
    pub(crate) access: TargetAccess,
    pub(crate) identity: FileIdentity,
    pub(crate) permissions: Permissions,
    pub(crate) projected: ProjectedFile,
}

struct PlannedTarget {
    original_index: usize,
    file: FileEdit,
    access: TargetAccess,
    identity: FileIdentity,
}

pub(crate) fn project_plan(
    root: &FsRoot,
    options: WorktreeOptions,
    plan: &EditPlan,
) -> Result<Vec<ProjectedTarget>, WorktreeError> {
    validate_plan(plan, options)?;
    let planned = preflight(root, options, plan)?;
    let workers = options.worker_count(planned.len());
    let results = map_ordered(planned, workers, |_, planned| {
        project_target(planned, options)
    });
    let projected = collect_results(results)?;
    validate_aggregate_output(&projected, options)?;
    Ok(projected)
}

pub(crate) fn dry_run_report(plan: &EditPlan, files: &[ProjectedTarget]) -> DryRunReport {
    DryRunReport::new(
        plan.operation.clone(),
        files.iter().map(change_for).collect(),
    )
}

pub(crate) fn change_for(file: &ProjectedTarget) -> FileChange {
    FileChange::new(
        file.file.path.clone(),
        file.projected.source_hash,
        file.projected.output_hash,
        file.projected.bytes_before as u64,
        file.projected.bytes_after as u64,
        file.projected.edit_count,
    )
}

fn validate_plan(plan: &EditPlan, options: WorktreeOptions) -> Result<(), WorktreeError> {
    let limits = options.limits;
    validate_metadata(plan, limits)?;
    let max_total_edits = limits
        .max_files
        .checked_mul(limits.max_edits_per_file)
        .ok_or_else(|| too_large("total edit limit overflow"))?;
    plan.validate_with(PlanLimits {
        max_files: limits.max_files,
        max_edits_per_file: limits.max_edits_per_file,
        max_total_edits,
        max_path_bytes: 4_096,
        max_total_text_bytes: limits.max_total_artifact_bytes,
    })
    .map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::InvalidPlan,
            TransactionPhase::Validate,
            "weavatrix-edit rejected the multi-file plan",
            error,
        )
    })?;
    Ok(())
}

fn preflight(
    root: &FsRoot,
    options: WorktreeOptions,
    plan: &EditPlan,
) -> Result<Vec<PlannedTarget>, WorktreeError> {
    let mut files = plan.files.iter().cloned().enumerate().collect::<Vec<_>>();
    files.sort_by(|left, right| {
        weavatrix_refactor_plan::portable_path_key(&left.1.path)
            .cmp(&weavatrix_refactor_plan::portable_path_key(&right.1.path))
            .then_with(|| left.1.path.cmp(&right.1.path))
    });
    let mut identities = HashMap::with_capacity(files.len());
    let mut total_source = 0_usize;
    let mut planned = Vec::with_capacity(files.len());
    for (original_index, file) in files {
        let access = root.open_target(&file.path).map_err(|error| {
            fs_error(
                TransactionPhase::Validate,
                &file.path,
                original_index,
                "failed to open a confined target",
                error,
            )
        })?;
        let probe = access.probe().map_err(|error| {
            fs_error(
                TransactionPhase::Validate,
                &file.path,
                original_index,
                "failed to inspect target metadata",
                error,
            )
        })?;
        let bytes = checked_usize(probe.bytes, "source size does not fit this platform")?;
        if bytes > options.limits.max_source_bytes_per_file {
            return Err(too_large("source exceeds the per-file byte limit")
                .at_path(file.path)
                .at_file(original_index));
        }
        total_source = total_source
            .checked_add(bytes)
            .ok_or_else(|| too_large("total source size overflow"))?;
        if total_source > options.limits.max_total_source_bytes {
            return Err(too_large("plan exceeds the total source byte limit"));
        }
        if let Some(first) = identities.insert(probe.identity, original_index) {
            return Err(WorktreeError::new(
                WorktreeErrorCode::InvalidPlan,
                TransactionPhase::Validate,
                format!("target aliases plan file {first} by filesystem identity"),
            )
            .at_path(file.path)
            .at_file(original_index));
        }
        planned.push(PlannedTarget {
            original_index,
            file,
            access,
            identity: probe.identity,
        });
    }
    Ok(planned)
}

fn project_target(
    planned: PlannedTarget,
    options: WorktreeOptions,
) -> Result<ProjectedTarget, WorktreeError> {
    let snapshot = planned
        .access
        .snapshot(options.limits.max_source_bytes_per_file)
        .map_err(|error| {
            fs_error(
                TransactionPhase::Prepare,
                &planned.file.path,
                planned.original_index,
                "failed to read target safely",
                error,
            )
        })?;
    if snapshot.identity != planned.identity {
        return Err(concurrent(&planned));
    }
    let source = String::from_utf8(snapshot.source).map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::NonUtf8Source,
            TransactionPhase::Prepare,
            "target is not valid UTF-8",
            error,
        )
        .at_path(planned.file.path.clone())
        .at_file(planned.original_index)
    })?;
    let projected = project_file(&planned.file, source, options.limits)
        .map_err(|error| error.at_file(planned.original_index))?;
    Ok(ProjectedTarget {
        original_index: planned.original_index,
        file: planned.file,
        access: planned.access,
        identity: planned.identity,
        permissions: snapshot.permissions,
        projected,
    })
}

fn collect_results(
    results: Vec<Result<ProjectedTarget, ScheduleError<WorktreeError>>>,
) -> Result<Vec<ProjectedTarget>, WorktreeError> {
    let mut values = Vec::with_capacity(results.len());
    for (index, result) in results.into_iter().enumerate() {
        match result {
            Ok(value) => values.push(value),
            Err(ScheduleError::Operation(error)) => return Err(error),
            Err(ScheduleError::Panicked(message)) => {
                return Err(WorktreeError::new(
                    WorktreeErrorCode::WorkerPanicked,
                    TransactionPhase::Prepare,
                    message,
                )
                .at_file(index));
            }
            Err(ScheduleError::Cancelled) => {
                return Err(WorktreeError::new(
                    WorktreeErrorCode::Cancelled,
                    TransactionPhase::Prepare,
                    "preparation was cancelled after an earlier worker failure",
                )
                .at_file(index));
            }
        }
    }
    Ok(values)
}

fn validate_aggregate_output(
    files: &[ProjectedTarget],
    options: WorktreeOptions,
) -> Result<(), WorktreeError> {
    let mut source = 0_usize;
    let mut output = 0_usize;
    for file in files {
        source = source
            .checked_add(file.projected.bytes_before)
            .ok_or_else(|| too_large("total source size overflow"))?;
        output = output
            .checked_add(file.projected.bytes_after)
            .ok_or_else(|| too_large("total output size overflow"))?;
    }
    if output > options.limits.max_total_output_bytes {
        return Err(too_large("plan exceeds the total output byte limit"));
    }
    let artifacts = source
        .checked_add(output)
        .ok_or_else(|| too_large("total artifact size overflow"))?;
    if artifacts > options.limits.max_total_artifact_bytes {
        return Err(too_large("plan exceeds the total artifact byte limit"));
    }
    Ok(())
}

fn concurrent(planned: &PlannedTarget) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::ConcurrentModification,
        TransactionPhase::Prepare,
        "target identity changed after preflight",
    )
    .at_path(planned.file.path.clone())
    .at_file(planned.original_index)
}

fn too_large(message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::TransactionTooLarge,
        TransactionPhase::Validate,
        message,
    )
}
