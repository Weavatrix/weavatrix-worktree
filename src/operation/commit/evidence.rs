use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{PresentEvidence, SlotEvidence},
};

use super::{MutationState, PreparedWorktreeTransaction, StagedPath};

pub(super) fn verify_all(transaction: &PreparedWorktreeTransaction) -> Result<(), WorktreeError> {
    for (index, path) in transaction.paths.iter().enumerate() {
        transaction
            .root
            .revalidate_parent(&path.access)
            .map_err(|error| {
                path_error(
                    TransactionPhase::Commit,
                    path,
                    index,
                    "path parent changed after preparation",
                    error,
                )
            })?;
        verify_slot(path, path.before, transaction, index, "source path changed")?;
        verify_artifact(
            path,
            path.stage_name.as_deref(),
            present(path.after),
            transaction.options.limits.max_output_bytes_per_file,
            index,
            "staged output changed",
        )?;
        verify_artifact(
            path,
            path.backup_name.as_deref(),
            path.backup,
            transaction.options.limits.max_source_bytes_per_file,
            index,
            "backup changed",
        )?;
    }
    Ok(())
}

pub(super) fn classify(
    path: &StagedPath,
    transaction: &PreparedWorktreeTransaction,
) -> MutationState {
    if let Some(stage) = &path.stage_name {
        match path.access.same_file_as_artifact(stage) {
            Ok(true) => return MutationState::LinkedInstall,
            Ok(false) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return MutationState::Ambiguous,
        }
    }
    if path
        .access
        .verify_slot(
            path.after,
            transaction.options.limits.max_output_bytes_per_file,
        )
        .is_ok()
    {
        MutationState::Changed
    } else if path
        .access
        .verify_slot(
            path.before,
            transaction.options.limits.max_source_bytes_per_file,
        )
        .is_ok()
    {
        MutationState::Unchanged
    } else {
        MutationState::Ambiguous
    }
}

pub(super) fn verify_slot(
    path: &StagedPath,
    expected: SlotEvidence,
    transaction: &PreparedWorktreeTransaction,
    index: usize,
    message: &str,
) -> Result<(), WorktreeError> {
    path.access
        .verify_slot(
            expected,
            transaction.options.limits.max_source_bytes_per_file,
        )
        .map(drop)
        .map_err(|error| path_error(TransactionPhase::Commit, path, index, message, error))
}

fn verify_artifact(
    path: &StagedPath,
    name: Option<&str>,
    expected: Option<PresentEvidence>,
    max_bytes: usize,
    index: usize,
    message: &str,
) -> Result<(), WorktreeError> {
    match (name, expected) {
        (None, None) => Ok(()),
        (Some(name), Some(expected)) => {
            let actual = path
                .access
                .artifact_evidence(name, max_bytes)
                .map_err(|error| {
                    path_error(TransactionPhase::Commit, path, index, message, error)
                })?;
            if actual == expected {
                Ok(())
            } else {
                Err(WorktreeError::new(
                    WorktreeErrorCode::ConcurrentModification,
                    TransactionPhase::Commit,
                    message,
                )
                .at_path(path.path.clone())
                .at_file(index))
            }
        }
        _ => Err(WorktreeError::new(
            WorktreeErrorCode::JournalCorrupt,
            TransactionPhase::Commit,
            "artifact recipe and evidence disagree",
        )
        .at_path(path.path.clone())
        .at_file(index)),
    }
}

pub(super) fn present(value: SlotEvidence) -> Option<PresentEvidence> {
    match value {
        SlotEvidence::Absent => None,
        SlotEvidence::Present(value) => Some(value),
    }
}

pub(super) fn path_error(
    phase: TransactionPhase,
    path: &StagedPath,
    index: usize,
    message: &str,
    source: std::io::Error,
) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::ConcurrentModification,
        phase,
        message,
        source,
    )
    .at_path(path.path.clone())
    .at_file(index)
}
