use std::io;

use crate::{error::WorktreeError, filesystem::SlotEvidence, options::WorktreeOptions};

use super::evidence::{
    corrupt, current_state, foreign_state, matches_present_evidence, max_bytes, path_io,
    path_recovery_io, path_recovery_required,
};
use crate::operation::recovery_model::{ParsedJournal, RecoveryPath, StateSpec};

pub(super) fn linked_create_is_exact(
    path: &RecoveryPath,
    options: WorktreeOptions,
    parsed: &ParsedJournal,
) -> Result<bool, WorktreeError> {
    if !matches!(path.before, StateSpec::Absent) {
        return Ok(false);
    }
    let Some(stage) = path.stage_name.as_deref() else {
        return Ok(false);
    };
    match path.access.same_file_as_artifact(stage) {
        Ok(true) => {
            let evidence = path
                .access
                .linked_artifact_evidence(stage, max_bytes(options))
                .map_err(|error| {
                    path_io(
                        parsed,
                        path,
                        "failed to verify linked operation install",
                        error,
                    )
                })?;
            let Some(spec) = path.after.present() else {
                return Err(corrupt(
                    parsed,
                    path,
                    "linked install has no present after-state",
                ));
            };
            if !matches_present_evidence(spec, evidence, path.stage_identity) {
                return Err(foreign_state(
                    parsed,
                    path,
                    "linked operation install does not match exact staged evidence",
                ));
            }
            Ok(true)
        }
        Ok(false) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_recovery_io(
            path,
            "failed to inspect linked operation install",
            error,
        )),
    }
}

pub(super) fn linked_backup_restore_is_exact(
    path: &RecoveryPath,
    options: WorktreeOptions,
    parsed: &ParsedJournal,
) -> Result<bool, WorktreeError> {
    let (StateSpec::Present(spec), StateSpec::Absent) = (&path.before, &path.after) else {
        return Ok(false);
    };
    let Some(backup) = path.backup_name.as_deref() else {
        return Ok(false);
    };
    match path.access.same_file_as_artifact(backup) {
        Ok(true) => {
            let evidence = path
                .access
                .linked_artifact_evidence(backup, max_bytes(options))
                .map_err(|error| {
                    path_io(
                        parsed,
                        path,
                        "failed to verify linked operation backup restore",
                        error,
                    )
                })?;
            if !matches_present_evidence(spec, evidence, path.backup_identity) {
                return Err(foreign_state(
                    parsed,
                    path,
                    "linked operation backup restore does not match exact before evidence",
                ));
            }
            Ok(true)
        }
        Ok(false) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_recovery_io(
            path,
            "failed to inspect linked operation backup restore",
            error,
        )),
    }
}

pub(super) fn rollback_linked_create(
    path: &RecoveryPath,
    options: WorktreeOptions,
) -> Result<(), WorktreeError> {
    let stage = path.stage_name.as_deref().ok_or_else(|| {
        path_recovery_required(path, "linked operation install has no staged artifact name")
    })?;
    path.access
        .rollback_linked_install(stage)
        .map_err(|error| {
            path_recovery_io(path, "failed to reverse linked operation install", error)
        })?;
    if current_state(path, options)? != SlotEvidence::Absent {
        return Err(path_recovery_required(
            path,
            "linked operation install rollback did not restore absence",
        ));
    }
    Ok(())
}

pub(super) fn finish_linked_backup_restore(
    path: &RecoveryPath,
    options: WorktreeOptions,
) -> Result<(), WorktreeError> {
    let backup = path.backup_name.as_deref().ok_or_else(|| {
        path_recovery_required(path, "linked operation backup restore has no artifact name")
    })?;
    let identity = path.backup_identity.ok_or_else(|| {
        path_recovery_required(
            path,
            "linked operation backup restore has no durable identity",
        )
    })?;
    path.access
        .finish_linked_install(backup, identity)
        .map_err(|error| {
            path_recovery_io(
                path,
                "failed to finish linked operation backup restore",
                error,
            )
        })?;
    let StateSpec::Present(before) = &path.before else {
        return Err(path_recovery_required(
            path,
            "linked operation backup restore has no present before-state",
        ));
    };
    if !super::evidence::matches_present(before, current_state(path, options)?, Some(identity)) {
        return Err(path_recovery_required(
            path,
            "linked operation backup restore failed exact verification",
        ));
    }
    Ok(())
}
