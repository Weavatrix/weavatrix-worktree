use std::io::Write;

use crate::{
    edit::write_projected,
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{FileIdentity, TargetAccess},
    hash::Sha256Hash,
    journal::{JournalRecord, JournalWriter},
    options::WorktreeOptions,
    report::FileChange,
    scheduler::{ScheduleError, map_ordered},
};

use super::{
    plan::{ProjectedTarget, change_for},
    stage_cleanup::cleanup_artifacts,
    util::{fs_error, journal_error},
};

pub(crate) use super::stage_cleanup::cleanup_staged;

pub(crate) struct StagedFile {
    pub(crate) stable_index: u32,
    pub(crate) original_index: usize,
    pub(crate) access: TargetAccess,
    pub(crate) identity: FileIdentity,
    pub(crate) old_hash: Sha256Hash,
    pub(crate) new_hash: Sha256Hash,
    pub(crate) stage_name: String,
    pub(crate) backup_name: String,
    pub(crate) change: FileChange,
}

struct StageJob {
    file: ProjectedTarget,
    stable_index: u32,
    stage_name: String,
    backup_name: String,
}

pub(super) struct ArtifactRef {
    pub(super) access: TargetAccess,
    pub(super) stage_name: String,
    pub(super) backup_name: String,
}

pub(crate) fn stage_all(
    files: Vec<ProjectedTarget>,
    transaction_id: &str,
    options: WorktreeOptions,
    journal: &mut JournalWriter,
) -> Result<Vec<StagedFile>, WorktreeError> {
    let mut jobs = Vec::with_capacity(files.len());
    let mut artifacts = Vec::with_capacity(files.len());
    for (stable_index, file) in files.into_iter().enumerate() {
        let stable_index = u32::try_from(stable_index)
            .map_err(|_| too_large("file index does not fit the journal contract"))?;
        let stage_name = artifact_name(transaction_id, stable_index, "stage");
        let backup_name = artifact_name(transaction_id, stable_index, "backup");
        append_file_record(journal, &file, stable_index, &stage_name, &backup_name)?;
        artifacts.push(ArtifactRef {
            access: file.access.try_clone().map_err(|error| {
                fs_error(
                    TransactionPhase::Stage,
                    &file.file.path,
                    file.original_index,
                    "failed to clone a target capability",
                    error,
                )
            })?,
            stage_name: stage_name.clone(),
            backup_name: backup_name.clone(),
        });
        jobs.push(StageJob {
            file,
            stable_index,
            stage_name,
            backup_name,
        });
    }
    let results = map_ordered(jobs, options.worker_count(artifacts.len()), |_, job| {
        stage_one(job, options)
    });
    collect_staged(results, &artifacts)
}

fn stage_one(job: StageJob, options: WorktreeOptions) -> Result<StagedFile, WorktreeError> {
    let original_index = job.file.original_index;
    let result = write_artifacts(&job, options);
    if let Err(error) = result {
        let _ = job.file.access.remove_artifact(&job.stage_name);
        let _ = job.file.access.remove_artifact(&job.backup_name);
        return Err(error);
    }
    let change = change_for(&job.file);
    Ok(StagedFile {
        stable_index: job.stable_index,
        original_index,
        access: job.file.access,
        identity: job.file.identity,
        old_hash: job.file.projected.source_hash,
        new_hash: job.file.projected.output_hash,
        stage_name: job.stage_name,
        backup_name: job.backup_name,
        change,
    })
}

fn write_artifacts(job: &StageJob, options: WorktreeOptions) -> Result<(), WorktreeError> {
    let file = &job.file;
    let mut backup = file.access.create_new(&job.backup_name).map_err(|error| {
        fs_error(
            TransactionPhase::Stage,
            &file.file.path,
            file.original_index,
            "failed to create an exclusive backup",
            error,
        )
    })?;
    backup
        .write_all(file.projected.source.as_bytes())
        .and_then(|()| backup.flush())
        .and_then(|()| backup.set_permissions(file.permissions.clone()))
        .and_then(|()| backup.sync_all())
        .map_err(|error| stage_io(file, "failed to write and synchronize backup", error))?;
    drop(backup);

    let mut stage = file
        .access
        .create_new(&job.stage_name)
        .map_err(|error| stage_io(file, "failed to create an exclusive stage file", error))?;
    write_projected(&file.file, &file.projected, options.limits, &mut stage)?;
    stage
        .flush()
        .and_then(|()| stage.set_permissions(file.permissions.clone()))
        .and_then(|()| stage.sync_all())
        .map_err(|error| stage_io(file, "failed to synchronize staged output", error))?;
    drop(stage);
    file.access
        .sync_parent()
        .map_err(|error| stage_io(file, "failed to synchronize target parent", error))?;
    verify_artifacts(job, options)
}

fn verify_artifacts(job: &StageJob, options: WorktreeOptions) -> Result<(), WorktreeError> {
    let file = &job.file;
    let backup = file
        .access
        .read_artifact(&job.backup_name, options.limits.max_source_bytes_per_file)
        .map_err(|error| stage_io(file, "failed to verify backup", error))?;
    let stage = file
        .access
        .read_artifact(&job.stage_name, options.limits.max_output_bytes_per_file)
        .map_err(|error| stage_io(file, "failed to verify staged output", error))?;
    if Sha256Hash::compute(&backup) != file.projected.source_hash
        || Sha256Hash::compute(&stage) != file.projected.output_hash
    {
        return Err(WorktreeError::new(
            WorktreeErrorCode::StageFailed,
            TransactionPhase::Stage,
            "a synchronized artifact failed SHA-256 verification",
        )
        .at_path(file.file.path.clone())
        .at_file(file.original_index));
    }
    Ok(())
}

fn collect_staged(
    results: Vec<Result<StagedFile, ScheduleError<WorktreeError>>>,
    artifacts: &[ArtifactRef],
) -> Result<Vec<StagedFile>, WorktreeError> {
    let mut staged = Vec::with_capacity(results.len());
    let mut primary = None;
    for (index, result) in results.into_iter().enumerate() {
        match result {
            Ok(file) => staged.push(file),
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
        cleanup_artifacts(artifacts)?;
        return Err(error);
    }
    Ok(staged)
}

fn append_file_record(
    journal: &mut JournalWriter,
    file: &ProjectedTarget,
    index: u32,
    stage_name: &str,
    backup_name: &str,
) -> Result<(), WorktreeError> {
    journal
        .append(&JournalRecord::PreparedFile {
            index,
            path: file.file.path.clone(),
            old_sha256: file.projected.source_hash.to_string(),
            new_sha256: file.projected.output_hash.to_string(),
            bytes_before: file.projected.bytes_before as u64,
            bytes_after: file.projected.bytes_after as u64,
            edit_count: u32::try_from(file.projected.edit_count)
                .map_err(|_| too_large("edit count does not fit the journal contract"))?,
            stage_name: stage_name.to_owned(),
            backup_name: backup_name.to_owned(),
        })
        .map_err(|error| {
            journal_error(
                TransactionPhase::Stage,
                "failed to record file intent",
                error,
            )
        })?;
    Ok(())
}

fn artifact_name(transaction_id: &str, index: u32, suffix: &str) -> String {
    format!(".weavatrix-{transaction_id}-{index:04}.{suffix}")
}

fn stage_io(file: &ProjectedTarget, message: &str, source: std::io::Error) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::StageFailed,
        TransactionPhase::Stage,
        message,
        source,
    )
    .at_path(file.file.path.clone())
    .at_file(file.original_index)
}

fn too_large(message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::TransactionTooLarge,
        TransactionPhase::Stage,
        message,
    )
}
