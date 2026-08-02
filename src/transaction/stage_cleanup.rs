use crate::{
    error::{TransactionPhase, WorktreeError},
    filesystem::TargetAccess,
};

use super::{
    stage::{ArtifactRef, StagedFile},
    util::fs_error,
};

pub(crate) fn cleanup_staged(files: &[StagedFile]) -> Result<usize, WorktreeError> {
    let mut removed = 0;
    for file in files {
        removed += remove_pair(
            &file.access,
            &file.stage_name,
            &file.backup_name,
            file.original_index,
        )?;
    }
    Ok(removed)
}

pub(super) fn cleanup_artifacts(artifacts: &[ArtifactRef]) -> Result<usize, WorktreeError> {
    let mut removed = 0;
    for (index, artifact) in artifacts.iter().enumerate() {
        removed += remove_pair(
            &artifact.access,
            &artifact.stage_name,
            &artifact.backup_name,
            index,
        )?;
    }
    Ok(removed)
}

fn remove_pair(
    access: &TargetAccess,
    stage: &str,
    backup: &str,
    index: usize,
) -> Result<usize, WorktreeError> {
    let mut removed = 0;
    removed += usize::from(access.remove_artifact(stage).map_err(|error| {
        fs_error(
            TransactionPhase::Cleanup,
            access.path(),
            index,
            "failed to remove a stage artifact",
            error,
        )
        .requiring_recovery()
    })?);
    removed += usize::from(access.remove_artifact(backup).map_err(|error| {
        fs_error(
            TransactionPhase::Cleanup,
            access.path(),
            index,
            "failed to remove a backup artifact",
            error,
        )
        .requiring_recovery()
    })?);
    Ok(removed)
}
