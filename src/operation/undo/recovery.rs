mod converge;
mod replay;

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{ControlDir, FsRoot, TargetAccess},
    journal::FinishOutcome,
    options::WorktreeOptions,
    report::{RecoveryAction, RecoveryReport},
};

use super::receipt::{StoredReceipt, read_exact};
use crate::operation::journal::{Record, Writer, read};
use replay::UndoReplay;

/// Deterministically resolves an interrupted undo rollback journal.
///
/// A journal without any durable rollback intent aborts the undo and keeps the
/// receipt; a journal with at least one intent is idempotently completed until
/// every path matches its exact before state and the receipt is consumed.
pub(in crate::operation) fn recover_undo(
    control: &ControlDir,
    root: &FsRoot,
    file: std::fs::File,
    options: WorktreeOptions,
) -> Result<RecoveryReport, WorktreeError> {
    let entries = read(&file, options.limits.max_journal_bytes as u64).map_err(|error| {
        WorktreeError::with_source(
            WorktreeErrorCode::JournalCorrupt,
            TransactionPhase::Recover,
            "failed to replay the undo journal",
            error,
        )
        .requiring_recovery()
    })?;
    if entries.is_empty() {
        remove_journal(control, None)?;
        return Ok(RecoveryReport::new(
            None,
            RecoveryAction::DiscardedStaging,
            Vec::new(),
            0,
        ));
    }
    let replay = replay::parse(&entries, options)?;
    let stored = read_exact(control, replay.undo_id.as_str(), options)
        .map_err(|error| error.in_transaction(replay.rollback_id.clone()))?;
    let Some(receipt) = stored else {
        return finish_without_receipt(control, &replay);
    };
    replay.bind(&receipt)?;
    let accesses = open_targets(root, &receipt, &replay)?;
    if replay.finished {
        converge::verify_finished(control, &receipt, &accesses, &replay, options)?;
        consume(control, &replay, receipt.id())?;
        return Ok(report(&replay, RecoveryAction::RolledBack, 0));
    }
    if replay.intents.is_empty() {
        remove_journal(control, Some(&replay.rollback_id))?;
        return Ok(report(&replay, RecoveryAction::DiscardedStaging, 0));
    }
    let mut writer =
        Writer::resume(file, options.limits.max_journal_bytes as u64).map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::JournalCorrupt,
                TransactionPhase::Recover,
                "failed to resume the undo journal",
                error,
            )
            .in_transaction(replay.rollback_id.clone())
            .requiring_recovery()
        })?;
    let removed = converge::complete(control, &receipt, &accesses, &replay, &mut writer, options)?;
    append_finished(&mut writer, &replay)?;
    consume(control, &replay, receipt.id())?;
    Ok(report(&replay, RecoveryAction::RolledBack, removed))
}

fn finish_without_receipt(
    control: &ControlDir,
    replay: &UndoReplay,
) -> Result<RecoveryReport, WorktreeError> {
    if replay.finished {
        // The receipt was already consumed after the durable Finished record;
        // only the journal file remained to be removed.
        remove_journal(control, Some(&replay.rollback_id))?;
        return Ok(report(replay, RecoveryAction::RolledBack, 0));
    }
    Err(WorktreeError::new(
        WorktreeErrorCode::UndoCorrupt,
        TransactionPhase::Recover,
        "undo journal has no receipt to restore from",
    )
    .in_transaction(replay.rollback_id.clone())
    .requiring_recovery())
}

fn open_targets(
    root: &FsRoot,
    receipt: &StoredReceipt,
    replay: &UndoReplay,
) -> Result<Vec<TargetAccess>, WorktreeError> {
    let mut accesses = Vec::with_capacity(receipt.paths().len());
    for path in receipt.paths() {
        accesses.push(root.open_target(&path.path).map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::RecoveryRequired,
                TransactionPhase::Recover,
                "failed to reopen a retained path during undo recovery",
                error,
            )
            .at_path(path.path.clone())
            .at_file(path.index as usize)
            .in_transaction(replay.rollback_id.clone())
            .requiring_recovery()
        })?);
    }
    Ok(accesses)
}

fn append_finished(writer: &mut Writer, replay: &UndoReplay) -> Result<(), WorktreeError> {
    writer
        .append(&Record::Finished {
            outcome: FinishOutcome::RolledBack,
        })
        .map(drop)
        .map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::RecoveryRequired,
                TransactionPhase::Recover,
                "failed to record completed undo recovery",
                error,
            )
            .in_transaction(replay.rollback_id.clone())
            .requiring_recovery()
        })
}

fn consume(control: &ControlDir, replay: &UndoReplay, id: &str) -> Result<(), WorktreeError> {
    control
        .remove_undo_receipt(id)
        .map_err(|error| consume_error(replay, error))?;
    remove_journal(control, Some(&replay.rollback_id))
}

fn remove_journal(control: &ControlDir, rollback_id: Option<&str>) -> Result<(), WorktreeError> {
    control.remove_undo_journal().map_err(|error| {
        let mapped = WorktreeError::with_source(
            WorktreeErrorCode::RecoveryRequired,
            TransactionPhase::Recover,
            "failed to remove the resolved undo journal",
            error,
        )
        .requiring_recovery();
        match rollback_id {
            Some(id) => mapped.in_transaction(id.to_owned()),
            None => mapped,
        }
    })
}

fn consume_error(replay: &UndoReplay, error: std::io::Error) -> WorktreeError {
    WorktreeError::with_source(
        WorktreeErrorCode::RecoveryRequired,
        TransactionPhase::Recover,
        "failed to consume the restored undo receipt",
        error,
    )
    .in_transaction(replay.rollback_id.clone())
    .requiring_recovery()
}

fn report(replay: &UndoReplay, action: RecoveryAction, removed: usize) -> RecoveryReport {
    RecoveryReport::new(
        Some(replay.rollback_id.clone()),
        action,
        Vec::new(),
        removed,
    )
}
