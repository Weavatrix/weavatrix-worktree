use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{FileIdentity, TargetAccess},
    hash::Sha256Hash,
};

use super::util::fs_error;

pub(crate) fn verify_target(
    access: &TargetAccess,
    expected_hash: Sha256Hash,
    expected_identity: Option<FileIdentity>,
    max_bytes: usize,
    index: usize,
    phase: TransactionPhase,
) -> Result<(), WorktreeError> {
    let snapshot = access.snapshot(max_bytes).map_err(|error| {
        fs_error(
            phase,
            access.path(),
            index,
            "failed to revalidate a transaction target",
            error,
        )
    })?;
    if expected_identity.is_some_and(|identity| identity != snapshot.identity) {
        return Err(concurrent(access, index, phase, "target identity changed"));
    }
    let actual = Sha256Hash::compute(&snapshot.source);
    if actual != expected_hash {
        return Err(concurrent(
            access,
            index,
            phase,
            &format!("expected target {expected_hash}, found {actual}"),
        ));
    }
    Ok(())
}

pub(crate) fn verify_artifact(
    access: &TargetAccess,
    name: &str,
    expected_hash: Sha256Hash,
    max_bytes: usize,
    index: usize,
    phase: TransactionPhase,
) -> Result<(), WorktreeError> {
    let bytes = access.read_artifact(name, max_bytes).map_err(|error| {
        let error = fs_error(
            phase,
            access.path(),
            index,
            "failed to read a transaction artifact",
            error,
        );
        if phase == TransactionPhase::Commit {
            error
        } else {
            error.requiring_recovery()
        }
    })?;
    if Sha256Hash::compute(&bytes) != expected_hash {
        let code = if phase == TransactionPhase::Commit {
            WorktreeErrorCode::CommitFailed
        } else {
            WorktreeErrorCode::RecoveryRequired
        };
        let error = WorktreeError::new(
            code,
            phase,
            "transaction artifact SHA-256 does not match its journal evidence",
        )
        .at_path(access.path().to_owned())
        .at_file(index);
        return Err(if phase == TransactionPhase::Commit {
            error
        } else {
            error.requiring_recovery()
        });
    }
    Ok(())
}

fn concurrent(
    access: &TargetAccess,
    index: usize,
    phase: TransactionPhase,
    message: &str,
) -> WorktreeError {
    WorktreeError::new(WorktreeErrorCode::ConcurrentModification, phase, message)
        .at_path(access.path().to_owned())
        .at_file(index)
}
