use crate::{
    error::WorktreeError, filesystem::SlotEvidence, journal::FinishOutcome,
    options::WorktreeOptions, report::RecoveryAction,
};

use super::{
    append_finished, append_record,
    evidence::{
        corrupt, current_state, foreign_state, matches_after, matches_before, matches_present,
        path_io,
    },
    finished::verify_finished,
    linked::{
        finish_linked_backup_restore, linked_backup_restore_is_exact, linked_create_is_exact,
        rollback_linked_create,
    },
};
use crate::operation::{
    journal::{Record, Writer},
    recovery_model::{ParsedJournal, RecoveryPath, StateSpec},
};

pub(super) fn recover_state(
    parsed: &ParsedJournal,
    options: WorktreeOptions,
    writer: &mut Writer,
) -> Result<RecoveryAction, WorktreeError> {
    match parsed.finished {
        Some(FinishOutcome::Committed) => {
            verify_finished(parsed, options, true)?;
            Ok(RecoveryAction::FinishedCommitCleanup)
        }
        Some(FinishOutcome::RolledBack | FinishOutcome::Aborted) => {
            verify_finished(parsed, options, false)?;
            Ok(RecoveryAction::DiscardedStaging)
        }
        None if !parsed.prepared || parsed.commit_intents.is_empty() => {
            discard_uncommitted(parsed, options)?;
            append_finished(writer, parsed, FinishOutcome::Aborted)?;
            Ok(RecoveryAction::DiscardedStaging)
        }
        None => {
            rollback_changed(parsed, options, writer)?;
            append_finished(writer, parsed, FinishOutcome::RolledBack)?;
            Ok(RecoveryAction::RolledBack)
        }
    }
}

fn discard_uncommitted(
    parsed: &ParsedJournal,
    options: WorktreeOptions,
) -> Result<(), WorktreeError> {
    for path in parsed.paths.iter().rev() {
        if linked_create_is_exact(path, options, parsed)? {
            return Err(foreign_state(
                parsed,
                path,
                "linked install exists before any synchronized commit intent",
            ));
        }
        if !matches_before(path, current_state(path, options)?) {
            return Err(foreign_state(
                parsed,
                path,
                "uncommitted operation path no longer matches its exact before state",
            ));
        }
    }
    Ok(())
}

fn finish_linked_backup_path(
    parsed: &ParsedJournal,
    path: &RecoveryPath,
    options: WorktreeOptions,
    writer: &mut Writer,
) -> Result<(), WorktreeError> {
    if parsed.rolled_back.contains(&path.index) {
        return Err(foreign_state(
            parsed,
            path,
            "a journaled rolled-back path is still a linked backup restore",
        ));
    }
    if !parsed.rollback_intents.contains(&path.index) {
        return Err(foreign_state(
            parsed,
            path,
            "linked backup restore lacks a synchronized operation rollback intent",
        ));
    }
    finish_linked_backup_restore(path, options)?;
    append_record(writer, parsed, &Record::RolledBack { index: path.index })
}

fn rollback_changed(
    parsed: &ParsedJournal,
    options: WorktreeOptions,
    writer: &mut Writer,
) -> Result<(), WorktreeError> {
    for path in parsed.paths.iter().rev() {
        if linked_backup_restore_is_exact(path, options, parsed)? {
            finish_linked_backup_path(parsed, path, options, writer)?;
            continue;
        }
        if linked_create_is_exact(path, options, parsed)? {
            rollback_linked_path(parsed, path, options, writer)?;
            continue;
        }
        let actual = current_state(path, options)?;
        if parsed.rolled_back.contains(&path.index) {
            if matches_before(path, actual) {
                continue;
            }
            return Err(foreign_state(
                parsed,
                path,
                "a journaled rolled-back path no longer matches its before state",
            ));
        }
        if matches_before(path, actual) {
            attest_unchanged_intent(parsed, path, writer)?;
            continue;
        }
        if !parsed.commit_intents.contains(&path.index) {
            return Err(foreign_state(
                parsed,
                path,
                "path changed without a synchronized operation commit intent",
            ));
        }
        if !matches_after(path, actual) {
            return Err(foreign_state(
                parsed,
                path,
                "operation path matches neither journaled before nor after evidence",
            ));
        }
        append_rollback_intent(parsed, path, writer)?;
        restore_before(path, actual, options, parsed)?;
        append_record(writer, parsed, &Record::RolledBack { index: path.index })?;
    }
    Ok(())
}

fn rollback_linked_path(
    parsed: &ParsedJournal,
    path: &RecoveryPath,
    options: WorktreeOptions,
    writer: &mut Writer,
) -> Result<(), WorktreeError> {
    if parsed.rolled_back.contains(&path.index) {
        return Err(foreign_state(
            parsed,
            path,
            "a journaled rolled-back path is still a linked install",
        ));
    }
    if !parsed.commit_intents.contains(&path.index) {
        return Err(foreign_state(
            parsed,
            path,
            "linked install lacks a synchronized operation commit intent",
        ));
    }
    append_rollback_intent(parsed, path, writer)?;
    rollback_linked_create(path, options)?;
    append_record(writer, parsed, &Record::RolledBack { index: path.index })
}

fn attest_unchanged_intent(
    parsed: &ParsedJournal,
    path: &RecoveryPath,
    writer: &mut Writer,
) -> Result<(), WorktreeError> {
    if parsed.commit_intents.contains(&path.index) {
        path.access.sync_parent().map_err(|error| {
            path_io(
                parsed,
                path,
                "before-state directory did not synchronize during recovery",
                error,
            )
        })?;
        append_rollback_intent(parsed, path, writer)?;
        append_record(writer, parsed, &Record::RolledBack { index: path.index })?;
    }
    Ok(())
}

fn append_rollback_intent(
    parsed: &ParsedJournal,
    path: &RecoveryPath,
    writer: &mut Writer,
) -> Result<(), WorktreeError> {
    if parsed.rollback_intents.contains(&path.index) {
        Ok(())
    } else {
        append_record(
            writer,
            parsed,
            &Record::RollbackIntent { index: path.index },
        )
    }
}

fn restore_before(
    path: &RecoveryPath,
    actual: SlotEvidence,
    options: WorktreeOptions,
    parsed: &ParsedJournal,
) -> Result<(), WorktreeError> {
    match (&path.before, &path.after) {
        (StateSpec::Absent, StateSpec::Present(_)) => {
            let SlotEvidence::Present(present) = actual else {
                return Err(foreign_state(
                    parsed,
                    path,
                    "create target vanished during rollback",
                ));
            };
            path.access
                .remove_exact(present, super::evidence::max_bytes(options))
                .map_err(|error| path_io(parsed, path, "failed to remove created path", error))?;
            path.access.sync_parent().map_err(|error| {
                path_io(
                    parsed,
                    path,
                    "created-path rollback did not synchronize",
                    error,
                )
            })?;
        }
        (StateSpec::Present(before), StateSpec::Absent | StateSpec::Present(_)) => {
            let backup = path
                .backup_name
                .as_deref()
                .ok_or_else(|| corrupt(parsed, path, "present before-state lacks backup name"))?;
            super::evidence::verify_artifact(
                path,
                backup,
                before,
                path.backup_identity,
                options,
                parsed,
            )?;
            match actual {
                SlotEvidence::Absent => {
                    path.access.install_absent_from(backup).map_err(|error| {
                        path_io(parsed, path, "failed to reinstall operation backup", error)
                    })?;
                }
                SlotEvidence::Present(_) => path.access.replace_from(backup).map_err(|error| {
                    path_io(parsed, path, "failed to restore operation backup", error)
                })?,
            }
            path.access.sync_parent().map_err(|error| {
                path_io(
                    parsed,
                    path,
                    "restored operation backup did not synchronize",
                    error,
                )
            })?;
            if !matches_present(before, current_state(path, options)?, path.backup_identity) {
                return Err(foreign_state(
                    parsed,
                    path,
                    "restored operation backup failed exact evidence verification",
                ));
            }
        }
        (StateSpec::Absent, StateSpec::Absent) => {
            return Err(corrupt(parsed, path, "invalid absent-to-absent transition"));
        }
    }
    Ok(())
}
