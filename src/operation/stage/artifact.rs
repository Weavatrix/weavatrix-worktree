use std::io::Write;

use weavatrix_refactor_plan::{ApplyLimits, prepare_edits_with_limits};

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{PresentEvidence, TargetAccess},
    options::WorktreeOptions,
};

use super::super::model::{OutputRecipe, ProjectedPresent};
use super::{invalid_stage, stage_io};

pub(super) fn write_output(
    access: &TargetAccess,
    name: &str,
    present: &ProjectedPresent,
    options: WorktreeOptions,
    path: &str,
    index: usize,
) -> Result<PresentEvidence, WorktreeError> {
    let mut file = access
        .create_new(name)
        .map_err(|error| stage_io(path, index, "failed to create exclusive stage", error))?;
    match &present.recipe {
        OutputRecipe::Exact(source) => file
            .write_all(source.as_bytes())
            .map_err(|error| stage_io(path, index, "failed to write exact staged output", error))?,
        OutputRecipe::Edited { source, edits } => {
            let prepared = prepare_edits_with_limits(
                source,
                edits,
                ApplyLimits {
                    max_source_bytes: options.limits.max_source_bytes_per_file,
                    max_edits: options.limits.max_edits_per_file,
                    max_output_bytes: options.limits.max_output_bytes_per_file,
                },
            )
            .map_err(|error| {
                WorktreeError::with_source(
                    WorktreeErrorCode::EditRejected,
                    TransactionPhase::Stage,
                    "staged edit recipe no longer validates",
                    error,
                )
                .at_path(path.to_owned())
                .at_file(index)
            })?;
            prepared.write_to(&mut file).map_err(|error| {
                stage_io(path, index, "failed to stream exact staged edits", error)
            })?;
        }
    }
    finish_artifact(
        access,
        name,
        file,
        present.permissions,
        options.limits.max_output_bytes_per_file,
        present.sha256,
        path,
        index,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn write_artifact(
    access: &TargetAccess,
    name: &str,
    bytes: &[u8],
    permissions: crate::filesystem::PortablePermissions,
    max_bytes: usize,
    expected: crate::Sha256Hash,
    path: &str,
    index: usize,
) -> Result<PresentEvidence, WorktreeError> {
    let mut file = access
        .create_new(name)
        .map_err(|error| stage_io(path, index, "failed to create exclusive backup", error))?;
    file.write_all(bytes)
        .map_err(|error| stage_io(path, index, "failed to write exact backup", error))?;
    finish_artifact(
        access,
        name,
        file,
        permissions,
        max_bytes,
        expected,
        path,
        index,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_artifact(
    access: &TargetAccess,
    name: &str,
    mut file: cap_std::fs::File,
    permissions: crate::filesystem::PortablePermissions,
    max_bytes: usize,
    expected: crate::Sha256Hash,
    path: &str,
    index: usize,
) -> Result<PresentEvidence, WorktreeError> {
    file.flush()
        .and_then(|()| {
            let mut value = file.metadata()?.permissions();
            permissions.apply_to(&mut value);
            file.set_permissions(value)
        })
        .and_then(|()| file.sync_all())
        .map_err(|error| stage_io(path, index, "failed to synchronize artifact", error))?;
    drop(file);
    let evidence = access
        .artifact_evidence(name, max_bytes)
        .map_err(|error| stage_io(path, index, "failed to verify synchronized artifact", error))?;
    if evidence.sha256 != expected || evidence.permissions != permissions {
        return Err(
            invalid_stage(path, "artifact evidence does not match projection").at_file(index),
        );
    }
    Ok(evidence)
}
