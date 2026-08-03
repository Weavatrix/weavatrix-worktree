use std::{collections::BTreeMap, sync::Arc};

use crate::{
    WorktreeOperation, WorktreePlan,
    error::WorktreeError,
    filesystem::PresentEvidence,
    hash::Sha256Hash,
    report::{OperationChange, OperationKind},
};

use super::invalid_internal;

pub(super) fn changes(
    plan: &WorktreePlan,
    sources: &BTreeMap<String, (Arc<str>, PresentEvidence)>,
    outputs: &BTreeMap<String, (Sha256Hash, u64, usize)>,
) -> Result<Vec<OperationChange>, WorktreeError> {
    plan.operations
        .iter()
        .enumerate()
        .map(|(index, operation)| match operation {
            WorktreeOperation::Modify(file) => {
                let (_, before) = source(sources, &file.path, index)?;
                let (hash, bytes, edits) = output(outputs, &file.path, index)?;
                Ok(OperationChange::new(
                    OperationKind::Modify,
                    Some(file.path.clone()),
                    None,
                    Some(before.sha256),
                    Some(hash),
                    before.bytes,
                    bytes,
                    edits,
                ))
            }
            WorktreeOperation::Create(file) => {
                let (hash, bytes, edits) = output(outputs, &file.path, index)?;
                Ok(OperationChange::new(
                    OperationKind::Create,
                    None,
                    Some(file.path.clone()),
                    None,
                    Some(hash),
                    0,
                    bytes,
                    edits,
                ))
            }
            WorktreeOperation::Delete(file) => {
                let (_, before) = source(sources, &file.path, index)?;
                Ok(OperationChange::new(
                    OperationKind::Delete,
                    Some(file.path.clone()),
                    None,
                    Some(before.sha256),
                    None,
                    before.bytes,
                    0,
                    0,
                ))
            }
            WorktreeOperation::Rename(file) => {
                let (_, before) = source(sources, &file.from, index)?;
                let (hash, bytes, edits) = output(outputs, &file.to, index)?;
                Ok(OperationChange::new(
                    OperationKind::Rename,
                    Some(file.from.clone()),
                    Some(file.to.clone()),
                    Some(before.sha256),
                    Some(hash),
                    before.bytes,
                    bytes,
                    edits,
                ))
            }
        })
        .collect()
}

fn source<'a>(
    values: &'a BTreeMap<String, (Arc<str>, PresentEvidence)>,
    path: &str,
    index: usize,
) -> Result<&'a (Arc<str>, PresentEvidence), WorktreeError> {
    values
        .get(path)
        .ok_or_else(|| invalid_internal("logical operation has no projected source", index))
}

fn output(
    values: &BTreeMap<String, (Sha256Hash, u64, usize)>,
    path: &str,
    index: usize,
) -> Result<(Sha256Hash, u64, usize), WorktreeError> {
    values
        .get(path)
        .copied()
        .ok_or_else(|| invalid_internal("logical operation has no projected output", index))
}
