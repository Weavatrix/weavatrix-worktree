use std::collections::{BTreeMap, BTreeSet};

use weavatrix_refactor_plan::FileEdit;

use super::{InputRole, PathTransition, PlannedInput, PlannedOutput};
use crate::{
    CreateFile, DeleteFile, RenameFile,
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    hash::Sha256Hash,
};

#[derive(Clone)]
pub(super) enum InputEndpoint {
    Modify { index: usize, file: FileEdit },
    Delete { index: usize, file: DeleteFile },
    Rename { index: usize, file: RenameFile },
}

#[derive(Clone)]
pub(super) enum OutputEndpoint {
    Modify { index: usize, file: FileEdit },
    Create { index: usize, file: CreateFile },
    Rename { index: usize, file: RenameFile },
}

pub(super) fn insert_input(
    values: &mut BTreeMap<String, InputEndpoint>,
    key: &str,
    value: InputEndpoint,
    index: usize,
) -> Result<(), WorktreeError> {
    if values.insert(key.to_owned(), value).is_some() {
        return Err(invalid_invariant(
            "validated plan consumes one path more than once",
            index,
        ));
    }
    Ok(())
}

pub(super) fn insert_output(
    values: &mut BTreeMap<String, OutputEndpoint>,
    key: &str,
    value: OutputEndpoint,
    index: usize,
) -> Result<(), WorktreeError> {
    if values.insert(key.to_owned(), value).is_some() {
        return Err(invalid_invariant(
            "validated plan produces one path more than once",
            index,
        ));
    }
    Ok(())
}

pub(super) fn validate_cross_roles(
    inputs: &BTreeMap<String, InputEndpoint>,
    outputs: &BTreeMap<String, OutputEndpoint>,
    paths: &BTreeMap<String, String>,
) -> Result<(), WorktreeError> {
    for (key, input) in inputs {
        let Some(output) = outputs.get(key) else {
            continue;
        };
        let allowed = match (input, output) {
            (
                InputEndpoint::Modify { index: left, .. },
                OutputEndpoint::Modify { index: right, .. },
            ) => left == right,
            (InputEndpoint::Rename { .. }, OutputEndpoint::Rename { .. }) => true,
            _ => false,
        };
        if !allowed {
            let index = input_index(input).max(output_index(output));
            return Err(invalid_invariant(
                &format!("validated plan has incompatible roles for {}", paths[key]),
                index,
            ));
        }
    }
    Ok(())
}

pub(super) fn build_transitions(
    paths: &BTreeMap<String, String>,
    mut inputs: BTreeMap<String, InputEndpoint>,
    mut outputs: BTreeMap<String, OutputEndpoint>,
) -> Result<Vec<PathTransition>, WorktreeError> {
    paths
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|key| {
            let path = paths
                .get(&key)
                .cloned()
                .ok_or_else(|| invalid_invariant("validated plan lost a registered path", 0))?;
            let before = match inputs.remove(&key) {
                Some(input) => planned_input(input)?,
                None => PlannedInput::Absent,
            };
            let after = match outputs.remove(&key) {
                Some(output) => planned_output(output)?,
                None => PlannedOutput::Absent,
            };
            Ok(PathTransition {
                path,
                before,
                after,
            })
        })
        .collect()
}

fn planned_input(endpoint: InputEndpoint) -> Result<PlannedInput, WorktreeError> {
    let (operation_index, hash, role) = match endpoint {
        InputEndpoint::Modify { index, file } => (index, file.sha256, InputRole::Modify),
        InputEndpoint::Delete { index, file } => (index, file.expected_sha256, InputRole::Delete),
        InputEndpoint::Rename { index, file } => (
            index,
            file.expected_source_sha256,
            InputRole::RenameSource {
                destination: file.to,
            },
        ),
    };
    Ok(PlannedInput::Present {
        operation_index,
        expected_sha256: parse_validated_hash(&hash, operation_index)?,
        role,
    })
}

fn planned_output(endpoint: OutputEndpoint) -> Result<PlannedOutput, WorktreeError> {
    Ok(match endpoint {
        OutputEndpoint::Modify { index, file } => PlannedOutput::Modify {
            operation_index: index,
            file,
        },
        OutputEndpoint::Create { index, file } => PlannedOutput::Create {
            operation_index: index,
            file,
        },
        OutputEndpoint::Rename { index, file } => PlannedOutput::Rename {
            operation_index: index,
            source: file.from,
            expected_source_sha256: parse_validated_hash(&file.expected_source_sha256, index)?,
            edits: file.edits,
        },
    })
}

fn parse_validated_hash(value: &str, index: usize) -> Result<Sha256Hash, WorktreeError> {
    Sha256Hash::parse(value).map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::InvalidPlan,
            TransactionPhase::Validate,
            "validated refactor plan contains a malformed SHA-256",
            error,
        )
        .at_file(index)
    })
}

fn invalid_invariant(message: &str, index: usize) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::InvalidPlan,
        TransactionPhase::Validate,
        message,
    )
    .at_file(index)
}

fn input_index(endpoint: &InputEndpoint) -> usize {
    match endpoint {
        InputEndpoint::Modify { index, .. }
        | InputEndpoint::Delete { index, .. }
        | InputEndpoint::Rename { index, .. } => *index,
    }
}

fn output_index(endpoint: &OutputEndpoint) -> usize {
    match endpoint {
        OutputEndpoint::Modify { index, .. }
        | OutputEndpoint::Create { index, .. }
        | OutputEndpoint::Rename { index, .. } => *index,
    }
}
