use std::io;

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{FileIdentity, PresentEvidence, SlotEvidence},
    options::WorktreeOptions,
};

use crate::operation::recovery_model::{ParsedJournal, PresentSpec, RecoveryPath, StateSpec};

pub(super) fn cleanup_artifacts(
    parsed: &ParsedJournal,
    options: WorktreeOptions,
    keep_backups: bool,
) -> Result<usize, WorktreeError> {
    let mut removed = 0_usize;
    for path in &parsed.paths {
        if let (Some(name), Some(spec)) = (path.stage_name.as_deref(), path.after.present()) {
            removed += cleanup_artifact(
                path,
                name,
                spec,
                path.stage_identity,
                options,
                parsed,
                "failed to remove operation stage artifact",
            )?;
        }
        if keep_backups {
            // A retained commit keeps its backups as consumable undo evidence.
            continue;
        }
        if let (Some(name), Some(spec)) = (path.backup_name.as_deref(), path.before.present()) {
            removed += cleanup_artifact(
                path,
                name,
                spec,
                path.backup_identity,
                options,
                parsed,
                "failed to remove operation backup artifact",
            )?;
        }
    }
    Ok(removed)
}

#[allow(clippy::too_many_arguments)]
fn cleanup_artifact(
    path: &RecoveryPath,
    name: &str,
    spec: &PresentSpec,
    identity: Option<FileIdentity>,
    options: WorktreeOptions,
    parsed: &ParsedJournal,
    message: &str,
) -> Result<usize, WorktreeError> {
    if path.staged {
        return remove_artifact_if_exact(path, name, spec, identity, options, parsed, message);
    }

    // A crash may interrupt artifact writes before PathStaged is durable. Such
    // files cannot match complete content evidence by construction, but their
    // names are already bound to this transaction by the validated PathIntent.
    path.access
        .remove_artifact(name)
        .map(usize::from)
        .map_err(|error| path_io(parsed, path, message, error))
}

#[allow(clippy::too_many_arguments)]
fn remove_artifact_if_exact(
    path: &RecoveryPath,
    name: &str,
    spec: &PresentSpec,
    identity: Option<FileIdentity>,
    options: WorktreeOptions,
    parsed: &ParsedJournal,
    message: &str,
) -> Result<usize, WorktreeError> {
    if artifact_exists_exact(path, name, spec, identity, options, parsed)? {
        path.access
            .remove_artifact(name)
            .map(usize::from)
            .map_err(|error| path_io(parsed, path, message, error))
    } else {
        Ok(0)
    }
}

fn artifact_exists_exact(
    path: &RecoveryPath,
    name: &str,
    spec: &PresentSpec,
    identity: Option<FileIdentity>,
    options: WorktreeOptions,
    parsed: &ParsedJournal,
) -> Result<bool, WorktreeError> {
    match path.access.artifact_evidence(name, max_bytes(options)) {
        Ok(actual) if matches_present_evidence(spec, actual, identity) => Ok(true),
        Ok(_) => Err(foreign_state(
            parsed,
            path,
            "transaction artifact does not match exact journal evidence",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_io(
            parsed,
            path,
            "failed to inspect operation transaction artifact",
            error,
        )),
    }
}

pub(super) fn verify_artifact(
    path: &RecoveryPath,
    name: &str,
    spec: &PresentSpec,
    identity: Option<FileIdentity>,
    options: WorktreeOptions,
    parsed: &ParsedJournal,
) -> Result<(), WorktreeError> {
    if artifact_exists_exact(path, name, spec, identity, options, parsed)? {
        Ok(())
    } else {
        Err(foreign_state(
            parsed,
            path,
            "required operation rollback artifact is missing",
        ))
    }
}

pub(super) fn current_state(
    path: &RecoveryPath,
    options: WorktreeOptions,
) -> Result<SlotEvidence, WorktreeError> {
    path.access
        .slot_evidence(max_bytes(options))
        .map_err(|error| {
            path_recovery_io(
                path,
                "failed to inspect exact operation target state",
                error,
            )
        })
}

pub(super) fn matches_before(path: &RecoveryPath, actual: SlotEvidence) -> bool {
    match &path.before {
        StateSpec::Absent => actual == SlotEvidence::Absent,
        StateSpec::Present(spec) => {
            matches_present(spec, actual, spec.identity)
                || matches_present(spec, actual, path.backup_identity)
        }
    }
}

pub(super) fn matches_after(path: &RecoveryPath, actual: SlotEvidence) -> bool {
    match &path.after {
        StateSpec::Absent => actual == SlotEvidence::Absent,
        StateSpec::Present(spec) => {
            matches_present(spec, actual, path.stage_identity.or(spec.identity))
        }
    }
}

pub(super) fn matches_present(
    expected: &PresentSpec,
    actual: SlotEvidence,
    identity: Option<FileIdentity>,
) -> bool {
    matches!(
        actual,
        SlotEvidence::Present(actual)
            if matches_present_evidence(expected, actual, identity)
    )
}

pub(super) fn matches_present_evidence(
    expected: &PresentSpec,
    actual: PresentEvidence,
    identity: Option<FileIdentity>,
) -> bool {
    expected.sha256 == actual.sha256
        && expected.bytes == actual.bytes
        && expected.permissions == actual.permissions
        && identity.is_none_or(|identity| identity == actual.identity)
}

pub(super) fn max_bytes(options: WorktreeOptions) -> usize {
    options
        .limits
        .max_source_bytes_per_file
        .max(options.limits.max_output_bytes_per_file)
}

pub(super) fn foreign_state(
    parsed: &ParsedJournal,
    path: &RecoveryPath,
    message: &str,
) -> WorktreeError {
    path_recovery_required(path, message).in_transaction(parsed.transaction_id.clone())
}

pub(super) fn corrupt(parsed: &ParsedJournal, path: &RecoveryPath, message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::JournalCorrupt,
        TransactionPhase::Recover,
        message,
    )
    .at_path(path.path.clone())
    .at_file(path_index(path))
    .in_transaction(parsed.transaction_id.clone())
    .requiring_recovery()
}

pub(super) fn path_io(
    parsed: &ParsedJournal,
    path: &RecoveryPath,
    message: &str,
    error: io::Error,
) -> WorktreeError {
    path_recovery_io(path, message, error).in_transaction(parsed.transaction_id.clone())
}

pub(super) fn path_recovery_io(
    path: &RecoveryPath,
    message: &str,
    error: io::Error,
) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::RecoveryRequired,
        TransactionPhase::Recover,
        message,
        error,
    )
    .at_path(path.path.clone())
    .at_file(path_index(path))
    .requiring_recovery()
}

pub(super) fn path_recovery_required(path: &RecoveryPath, message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::RecoveryRequired,
        TransactionPhase::Recover,
        message,
    )
    .at_path(path.path.clone())
    .at_file(path_index(path))
    .requiring_recovery()
}

fn path_index(path: &RecoveryPath) -> usize {
    usize::try_from(path.index).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests;
