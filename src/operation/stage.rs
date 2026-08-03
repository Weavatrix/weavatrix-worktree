use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{PresentEvidence, SlotEvidence},
    options::WorktreeOptions,
    scheduler::{ScheduleError, map_ordered},
};

use super::{
    journal::{Record, StateRecord, Writer},
    model::{ProjectedOutput, ProjectedPath, ProjectedPlan, StagedPath},
};

mod artifact;

use artifact::{write_artifact, write_output};

struct StageJob {
    path: ProjectedPath,
    stage_name: Option<String>,
    backup_name: Option<String>,
}

pub(super) fn stage_all(
    projected: ProjectedPlan,
    transaction_id: &str,
    options: WorktreeOptions,
    journal: &mut Writer,
) -> Result<(String, Vec<crate::report::OperationChange>, Vec<StagedPath>), WorktreeError> {
    append_operations(journal, &projected.operations)?;
    let mut jobs = Vec::with_capacity(projected.paths.len());
    for path in projected.paths {
        let stage_name = matches!(path.after, ProjectedOutput::Present(_))
            .then(|| artifact_name(transaction_id, path.stable_index, "stage"));
        let backup_name = matches!(path.before, super::model::ProjectedInput::Present { .. })
            .then(|| artifact_name(transaction_id, path.stable_index, "backup"));
        journal
            .append(&Record::PathIntent {
                index: path.stable_index,
                path: path.path.clone(),
                before: state_from_evidence(path.before.evidence(), true),
                after: state_from_output(&path.after),
                stage_name: stage_name.clone(),
                backup_name: backup_name.clone(),
            })
            .map_err(|error| journal_error("failed to record path staging intent", error))?;
        jobs.push(StageJob {
            path,
            stage_name,
            backup_name,
        });
    }

    let workers = options.worker_count(jobs.len());
    let results = map_ordered(jobs, workers, |_, job| stage_one(job, options));
    let staged = collect(results)?;
    for path in &staged {
        journal
            .append(&Record::PathStaged {
                index: path.stable_index,
                stage_identity: present(path.after).map(|value| value.identity),
                backup_identity: path.backup.map(|value| value.identity),
            })
            .map_err(|error| {
                journal_error("failed to record synchronized path artifacts", error)
                    .requiring_recovery()
            })?;
    }
    Ok((projected.operation, projected.operations, staged))
}

pub(super) fn cleanup(paths: &[StagedPath]) -> Result<usize, WorktreeError> {
    let mut removed = 0;
    for (index, path) in paths.iter().enumerate() {
        for artifact in [&path.stage_name, &path.backup_name].into_iter().flatten() {
            removed += usize::from(path.access.remove_artifact(artifact).map_err(|error| {
                WorktreeError::with_source(
                    WorktreeErrorCode::Io,
                    TransactionPhase::Cleanup,
                    "failed to remove an operation transaction artifact",
                    error,
                )
                .at_path(path.path.clone())
                .at_file(index)
                .requiring_recovery()
            })?);
        }
    }
    Ok(removed)
}

fn stage_one(job: StageJob, options: WorktreeOptions) -> Result<StagedPath, WorktreeError> {
    let staged = (|| {
        let before = job.path.before.evidence();
        let backup = match (&job.path.before, &job.backup_name) {
            (super::model::ProjectedInput::Present { source, evidence }, Some(name)) => {
                Some(write_artifact(
                    &job.path.access,
                    name,
                    source.as_bytes(),
                    evidence.permissions,
                    options.limits.max_source_bytes_per_file,
                    evidence.sha256,
                    &job.path.path,
                    job.path.stable_index as usize,
                )?)
            }
            (super::model::ProjectedInput::Absent, None) => None,
            _ => {
                return Err(invalid_stage(
                    &job.path.path,
                    "backup recipe is inconsistent",
                ));
            }
        };
        let stage = match (&job.path.after, &job.stage_name) {
            (ProjectedOutput::Present(present), Some(name)) => Some(write_output(
                &job.path.access,
                name,
                present,
                options,
                &job.path.path,
                job.path.stable_index as usize,
            )?),
            (ProjectedOutput::Absent, None) => None,
            _ => {
                return Err(invalid_stage(
                    &job.path.path,
                    "stage recipe is inconsistent",
                ));
            }
        };
        job.path.access.sync_parent().map_err(|error| {
            stage_io(
                &job.path.path,
                job.path.stable_index as usize,
                "failed to synchronize operation artifacts",
                error,
            )
        })?;
        Ok((before, backup, stage))
    })();
    let (before, backup, stage) = match staged {
        Ok(value) => value,
        Err(primary) => {
            for artifact in [&job.stage_name, &job.backup_name].into_iter().flatten() {
                if let Err(error) = job.path.access.remove_artifact(artifact) {
                    return Err(stage_io(
                        &job.path.path,
                        job.path.stable_index as usize,
                        "staging failed and artifact cleanup also failed",
                        error,
                    )
                    .requiring_recovery());
                }
            }
            return Err(primary);
        }
    };
    Ok(StagedPath {
        stable_index: job.path.stable_index,
        path: job.path.path,
        access: job.path.access,
        before,
        after: stage.map_or(SlotEvidence::Absent, SlotEvidence::Present),
        backup,
        stage_name: job.stage_name,
        backup_name: job.backup_name,
    })
}

fn collect(
    results: Vec<Result<StagedPath, ScheduleError<WorktreeError>>>,
) -> Result<Vec<StagedPath>, WorktreeError> {
    let mut paths = Vec::with_capacity(results.len());
    let mut primary = None;
    for (index, result) in results.into_iter().enumerate() {
        match result {
            Ok(value) => paths.push(value),
            Err(ScheduleError::Operation(error)) => {
                primary.get_or_insert(error);
            }
            Err(ScheduleError::Panicked(message)) => {
                primary.get_or_insert_with(|| {
                    WorktreeError::new(
                        WorktreeErrorCode::WorkerPanicked,
                        TransactionPhase::Stage,
                        message,
                    )
                    .at_file(index)
                });
            }
            Err(ScheduleError::Cancelled) => {}
        }
    }
    if let Some(error) = primary {
        let cleanup = cleanup(&paths);
        return cleanup.map_or_else(Err, |_| Err(error));
    }
    Ok(paths)
}

fn append_operations(
    journal: &mut Writer,
    operations: &[crate::report::OperationChange],
) -> Result<(), WorktreeError> {
    for (index, operation) in operations.iter().enumerate() {
        journal
            .append(&Record::Operation {
                index: u32::try_from(index)
                    .map_err(|_| invalid_stage("", "operation index is too large"))?,
                kind: operation.kind().as_str().to_owned(),
                source_path: operation.source_path().map(str::to_owned),
                destination_path: operation.destination_path().map(str::to_owned),
                old_sha256: operation.old_sha256().map(|value| value.to_string()),
                new_sha256: operation.new_sha256().map(|value| value.to_string()),
                bytes_before: operation.bytes_before(),
                bytes_after: operation.bytes_after(),
                edit_count: u32::try_from(operation.edits_applied())
                    .map_err(|_| invalid_stage("", "edit count is too large"))?,
            })
            .map_err(|error| journal_error("failed to record logical operation", error))?;
    }
    Ok(())
}

pub(super) fn state_from_evidence(value: SlotEvidence, identity: bool) -> StateRecord {
    match value {
        SlotEvidence::Absent => StateRecord::Absent,
        SlotEvidence::Present(value) => StateRecord::Present {
            sha256: value.sha256.to_string(),
            bytes: value.bytes,
            permissions: value.permissions,
            identity: identity.then_some(value.identity),
        },
    }
}

fn state_from_output(value: &ProjectedOutput) -> StateRecord {
    match value {
        ProjectedOutput::Absent => StateRecord::Absent,
        ProjectedOutput::Present(value) => StateRecord::Present {
            sha256: value.sha256.to_string(),
            bytes: value.bytes,
            permissions: value.permissions,
            identity: None,
        },
    }
}

fn present(value: SlotEvidence) -> Option<PresentEvidence> {
    match value {
        SlotEvidence::Absent => None,
        SlotEvidence::Present(value) => Some(value),
    }
}

fn artifact_name(transaction_id: &str, index: u32, suffix: &str) -> String {
    format!(".weavatrix-{transaction_id}-{index:04}.{suffix}")
}

fn stage_io(path: &str, index: usize, message: &str, source: std::io::Error) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::StageFailed,
        TransactionPhase::Stage,
        message,
        source,
    )
    .at_path(path.to_owned())
    .at_file(index)
}

fn journal_error(
    message: &str,
    source: impl std::error::Error + Send + Sync + 'static,
) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::JournalCorrupt,
        TransactionPhase::Stage,
        message,
        source,
    )
}

fn invalid_stage(path: &str, message: &str) -> WorktreeError {
    let error = WorktreeError::new(
        WorktreeErrorCode::StageFailed,
        TransactionPhase::Stage,
        message,
    );
    if path.is_empty() {
        error
    } else {
        error.at_path(path.to_owned())
    }
}
