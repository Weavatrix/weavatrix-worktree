use std::{collections::BTreeMap, sync::Arc};

use weavatrix_refactor_plan::{ApplyLimits, TextEdit, prepare_edits_with_limits};

use crate::{
    WorktreePlan,
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{PortablePermissions, PresentEvidence},
    hash::{Sha256Hash, Sha256Hasher},
    options::WorktreeOptions,
    plan::PlannedOutput,
};

use super::{SnapshotResult, invalid_internal, too_large};
use crate::operation::model::{
    OutputRecipe, ProjectedInput, ProjectedOutput, ProjectedPath, ProjectedPlan, ProjectedPresent,
};

use super::changes::changes;

pub(super) fn assemble(
    plan: &WorktreePlan,
    snapshots: Vec<SnapshotResult>,
    options: WorktreeOptions,
) -> Result<ProjectedPlan, WorktreeError> {
    let mut sources = BTreeMap::<String, (Arc<str>, PresentEvidence)>::new();
    let mut source_bytes = 0_usize;
    for snapshot in &snapshots {
        if let ProjectedInput::Present { source, evidence } = &snapshot.before {
            source_bytes = source_bytes
                .checked_add(source.len())
                .ok_or_else(|| too_large("total source byte count overflow"))?;
            if source_bytes > options.limits.max_total_source_bytes {
                return Err(too_large(
                    "operation plan exceeds the total source byte limit",
                ));
            }
            sources.insert(
                snapshot.transition.path.clone(),
                (source.clone(), *evidence),
            );
        }
    }

    let mut outputs = BTreeMap::<String, (Sha256Hash, u64, usize)>::new();
    let mut output_bytes = 0_usize;
    let mut paths = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let after = project_after(
            &snapshot.transition.after,
            &snapshot.before,
            &sources,
            options,
        )?;
        if let ProjectedOutput::Present(present) = &after {
            let bytes = usize::try_from(present.bytes)
                .map_err(|_| too_large("output size does not fit this platform"))?;
            output_bytes = output_bytes
                .checked_add(bytes)
                .ok_or_else(|| too_large("total output byte count overflow"))?;
            if output_bytes > options.limits.max_total_output_bytes {
                return Err(too_large(
                    "operation plan exceeds the total output byte limit",
                ));
            }
            outputs.insert(
                snapshot.transition.path.clone(),
                (present.sha256, present.bytes, present.edit_count),
            );
        }
        paths.push(ProjectedPath {
            stable_index: snapshot.stable_index,
            path: snapshot.transition.path,
            access: snapshot.access,
            before: snapshot.before,
            after,
        });
    }
    let artifact_bytes = source_bytes
        .checked_add(output_bytes)
        .ok_or_else(|| too_large("total artifact byte count overflow"))?;
    if artifact_bytes > options.limits.max_total_artifact_bytes {
        return Err(too_large(
            "operation plan exceeds the total artifact byte limit",
        ));
    }
    Ok(ProjectedPlan {
        operation: plan.operation.clone(),
        operations: changes(plan, &sources, &outputs)?,
        paths,
    })
}

fn project_after(
    planned: &PlannedOutput,
    own_before: &ProjectedInput,
    sources: &BTreeMap<String, (Arc<str>, PresentEvidence)>,
    options: WorktreeOptions,
) -> Result<ProjectedOutput, WorktreeError> {
    match planned {
        PlannedOutput::Absent => Ok(ProjectedOutput::Absent),
        PlannedOutput::Create {
            operation_index,
            file,
        } => present(
            Arc::from(file.contents.clone()),
            &[],
            create_permissions(file.permissions),
            *operation_index,
            &file.path,
            options,
        ),
        PlannedOutput::Modify {
            operation_index,
            file,
        } => {
            let Some(source) = own_before.source() else {
                return Err(invalid_internal(
                    "modify output has no source",
                    *operation_index,
                ));
            };
            present(
                source.clone(),
                &file.edits,
                own_permissions(own_before, *operation_index)?,
                *operation_index,
                &file.path,
                options,
            )
        }
        PlannedOutput::Rename {
            operation_index,
            source,
            edits,
            ..
        } => {
            let Some((contents, evidence)) = sources.get(source) else {
                return Err(invalid_internal(
                    "rename output has no source",
                    *operation_index,
                ));
            };
            present(
                contents.clone(),
                edits,
                evidence.permissions,
                *operation_index,
                source,
                options,
            )
        }
    }
}

fn present(
    source: Arc<str>,
    edits: &[TextEdit],
    permissions: PortablePermissions,
    operation_index: usize,
    path: &str,
    options: WorktreeOptions,
) -> Result<ProjectedOutput, WorktreeError> {
    if source.len() > options.limits.max_output_bytes_per_file {
        return Err(WorktreeError::new(
            WorktreeErrorCode::TransactionTooLarge,
            TransactionPhase::Prepare,
            "operation output exceeds the per-file byte limit",
        )
        .at_path(path.to_owned())
        .at_file(operation_index));
    }
    if edits.is_empty() {
        return Ok(ProjectedOutput::Present(ProjectedPresent {
            sha256: Sha256Hash::compute(source.as_bytes()),
            bytes: source.len() as u64,
            permissions,
            edit_count: 0,
            recipe: OutputRecipe::Exact(source),
        }));
    }
    let prepared = prepare_edits_with_limits(
        &source,
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
            TransactionPhase::Prepare,
            "weavatrix-edit rejected operation edits",
            error,
        )
        .at_path(path.to_owned())
        .at_file(operation_index)
    })?;
    let mut hasher = Sha256Hasher::new();
    for chunk in prepared.chunks() {
        hasher.update(chunk.as_bytes());
    }
    let bytes = prepared.bytes_after() as u64;
    let edit_count = prepared.len();
    drop(prepared);
    Ok(ProjectedOutput::Present(ProjectedPresent {
        recipe: OutputRecipe::Edited {
            source,
            edits: edits.to_vec(),
        },
        sha256: hasher.finish(),
        bytes,
        permissions,
        edit_count,
    }))
}

fn own_permissions(
    input: &ProjectedInput,
    operation_index: usize,
) -> Result<PortablePermissions, WorktreeError> {
    match input {
        ProjectedInput::Present { evidence, .. } => Ok(evidence.permissions),
        ProjectedInput::Absent => Err(invalid_internal(
            "present output has no permission source",
            operation_index,
        )),
    }
}

fn create_permissions(value: crate::CreatePermissions) -> PortablePermissions {
    PortablePermissions {
        readonly: value.readonly(),
        unix_mode: cfg!(unix).then(|| value.unix_mode()),
    }
}
