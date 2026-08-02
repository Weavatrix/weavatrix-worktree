use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::FsRoot,
    hash::Sha256Hash,
    journal::{FinishOutcome, JournalRecord, JournalWriter, read_journal},
    options::WorktreeOptions,
    report::{FileChange, RecoveryAction, RecoveryReport},
};

use super::{
    lock::acquire,
    recovery_model::{ParsedJournal, RecoveryFile, parse_journal},
    util::{fs_error, journal_error},
    verify::{verify_artifact, verify_target},
};

pub(crate) fn recover_transaction(
    root: &FsRoot,
    options: WorktreeOptions,
) -> Result<RecoveryReport, WorktreeError> {
    let locked = acquire(root)?;
    let Some(file) = locked.control.open_journal().map_err(recovery_io)? else {
        return Ok(RecoveryReport::new(
            None,
            RecoveryAction::NoPendingTransaction,
            Vec::new(),
            0,
        ));
    };
    let entries =
        read_journal(&file, options.limits.max_journal_bytes as u64).map_err(|error| {
            journal_error(TransactionPhase::Recover, "failed to replay journal", error)
        })?;
    if entries.is_empty() {
        locked.control.remove_journal().map_err(recovery_io)?;
        return Ok(RecoveryReport::new(
            None,
            RecoveryAction::DiscardedStaging,
            Vec::new(),
            0,
        ));
    }
    let parsed = parse_journal(root, &entries, options)?;
    let changes = parsed.files.iter().map(file_change).collect::<Vec<_>>();
    let transaction_id = parsed.transaction_id.clone();
    let mut writer =
        JournalWriter::resume(file, options.limits.max_journal_bytes as u64).map_err(|error| {
            journal_error(TransactionPhase::Recover, "failed to resume journal", error)
        })?;
    let action = recover_state(&parsed, options, &mut writer)?;
    let removed = cleanup_recovered(&parsed.files)?;
    locked.control.remove_journal().map_err(recovery_io)?;
    Ok(RecoveryReport::new(
        Some(transaction_id),
        action,
        changes,
        removed,
    ))
}

fn recover_state(
    parsed: &ParsedJournal,
    options: WorktreeOptions,
    writer: &mut JournalWriter,
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
        None if parsed.commit_intents.is_empty() => {
            append_finished(writer, FinishOutcome::Aborted)?;
            Ok(RecoveryAction::DiscardedStaging)
        }
        None => {
            rollback_unfinished(parsed, options, writer)?;
            append_finished(writer, FinishOutcome::RolledBack)?;
            Ok(RecoveryAction::RolledBack)
        }
    }
}

fn rollback_unfinished(
    parsed: &ParsedJournal,
    options: WorktreeOptions,
    writer: &mut JournalWriter,
) -> Result<(), WorktreeError> {
    for file in parsed.files.iter().rev() {
        if !parsed.commit_intents.contains(&file.index) {
            continue;
        }
        let max_bytes = options
            .limits
            .max_source_bytes_per_file
            .max(options.limits.max_output_bytes_per_file);
        let current = target_hash(file, max_bytes)?;
        if current == file.old_hash {
            continue;
        }
        if current != file.new_hash {
            return Err(recovery_required(
                file,
                "target matches neither journal hash",
            ));
        }
        restore_file(file, options, writer)?;
    }
    Ok(())
}

fn restore_file(
    file: &RecoveryFile,
    options: WorktreeOptions,
    writer: &mut JournalWriter,
) -> Result<(), WorktreeError> {
    verify_artifact(
        &file.access,
        &file.backup_name,
        file.old_hash,
        options.limits.max_source_bytes_per_file,
        file.index as usize,
        TransactionPhase::Recover,
    )?;
    writer
        .append(&JournalRecord::RollbackIntent { index: file.index })
        .map_err(|error| {
            journal_error(
                TransactionPhase::Recover,
                "failed to record recovery intent",
                error,
            )
        })?;
    file.access
        .rename_from(&file.backup_name)
        .and_then(|()| file.access.sync_parent())
        .map_err(|error| {
            fs_error(
                TransactionPhase::Recover,
                file.access.path(),
                file.index as usize,
                "failed to restore target during recovery",
                error,
            )
            .requiring_recovery()
        })?;
    writer
        .append(&JournalRecord::RolledBack { index: file.index })
        .map_err(|error| {
            journal_error(
                TransactionPhase::Recover,
                "failed to record recovery completion",
                error,
            )
        })?;
    Ok(())
}

fn verify_finished(
    parsed: &ParsedJournal,
    options: WorktreeOptions,
    committed: bool,
) -> Result<(), WorktreeError> {
    for file in &parsed.files {
        if !committed && !parsed.commit_intents.contains(&file.index) {
            continue;
        }
        let expected = if committed {
            file.new_hash
        } else {
            file.old_hash
        };
        verify_target(
            &file.access,
            expected,
            None,
            options
                .limits
                .max_source_bytes_per_file
                .max(options.limits.max_output_bytes_per_file),
            file.index as usize,
            TransactionPhase::Recover,
        )
        .map_err(|error| recovery_required(file, &error.to_string()))?;
    }
    Ok(())
}

fn cleanup_recovered(files: &[RecoveryFile]) -> Result<usize, WorktreeError> {
    let mut removed = 0;
    for file in files {
        for name in [&file.stage_name, &file.backup_name] {
            let was_removed = file.access.remove_artifact(name).map_err(|error| {
                fs_error(
                    TransactionPhase::Cleanup,
                    file.access.path(),
                    file.index as usize,
                    "failed to clean recovered artifact",
                    error,
                )
                .requiring_recovery()
            })?;
            removed += usize::from(was_removed);
        }
    }
    Ok(removed)
}

fn target_hash(file: &RecoveryFile, max_bytes: usize) -> Result<Sha256Hash, WorktreeError> {
    let snapshot = file.access.snapshot(max_bytes).map_err(|error| {
        fs_error(
            TransactionPhase::Recover,
            file.access.path(),
            file.index as usize,
            "failed to inspect recovery target",
            error,
        )
        .requiring_recovery()
    })?;
    Ok(Sha256Hash::compute(&snapshot.source))
}

fn file_change(file: &RecoveryFile) -> FileChange {
    FileChange::new(
        file.access.path().to_owned(),
        file.old_hash,
        file.new_hash,
        file.bytes_before,
        file.bytes_after,
        file.edit_count as usize,
    )
}

fn append_finished(
    writer: &mut JournalWriter,
    outcome: FinishOutcome,
) -> Result<(), WorktreeError> {
    writer
        .append(&JournalRecord::Finished { outcome })
        .map_err(|error| {
            journal_error(
                TransactionPhase::Recover,
                "failed to finish recovery journal",
                error,
            )
            .requiring_recovery()
        })?;
    Ok(())
}

fn recovery_required(file: &RecoveryFile, message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::RecoveryRequired,
        TransactionPhase::Recover,
        message,
    )
    .at_path(file.access.path().to_owned())
    .at_file(file.index as usize)
    .requiring_recovery()
}

fn recovery_io(error: std::io::Error) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::RecoveryRequired,
        TransactionPhase::Recover,
        "recovery filesystem I/O failed",
        error,
    )
    .requiring_recovery()
}
