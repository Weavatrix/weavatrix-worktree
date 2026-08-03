use std::collections::BTreeSet;

use crate::{
    Sha256Hash,
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    journal::FinishOutcome,
    options::WorktreeOptions,
};

use crate::operation::journal::{Entry, Record};
use crate::operation::undo::receipt::StoredReceipt;
use crate::operation::undo::types::UndoId;

pub(super) struct UndoReplay {
    pub(super) rollback_id: String,
    pub(super) undo_id: UndoId,
    pub(super) checksum: Sha256Hash,
    pub(super) path_count: usize,
    pub(super) intents: BTreeSet<u32>,
    pub(super) rolled_back: BTreeSet<u32>,
    pub(super) finished: bool,
}

impl UndoReplay {
    /// Binds the journal to the exact receipt version it was created against.
    pub(super) fn bind(&self, receipt: &StoredReceipt) -> Result<(), WorktreeError> {
        if self.checksum != receipt.checksum || self.path_count != receipt.paths().len() {
            return Err(corrupt("undo journal does not match its stored receipt")
                .in_transaction(self.rollback_id.clone()));
        }
        Ok(())
    }
}

pub(super) fn parse(
    entries: &[Entry],
    options: WorktreeOptions,
) -> Result<UndoReplay, WorktreeError> {
    let mut replay = parse_header(entries, options)?;
    for entry in &entries[1..] {
        apply_record(&mut replay, &entry.record)?;
    }
    Ok(replay)
}

fn parse_header(entries: &[Entry], options: WorktreeOptions) -> Result<UndoReplay, WorktreeError> {
    let Some(Entry {
        record:
            Record::Header {
                transaction_id,
                contract_hash,
                operation,
                operation_count,
                path_count,
            },
        ..
    }) = entries.first()
    else {
        return Err(corrupt("undo journal does not begin with a header"));
    };
    let undo_id = operation
        .parse::<UndoId>()
        .map_err(|_| corrupt("undo journal header names an invalid receipt"))?;
    let checksum = Sha256Hash::parse(contract_hash)
        .map_err(|_| corrupt("undo journal header checksum is invalid"))?;
    let path_count = usize::try_from(*path_count).unwrap_or(usize::MAX);
    if transaction_id.parse::<UndoId>().is_err()
        || *operation_count != 0
        || path_count == 0
        || path_count > options.limits.max_files
    {
        return Err(corrupt("invalid or over-limit undo journal header"));
    }
    Ok(UndoReplay {
        rollback_id: transaction_id.clone(),
        undo_id,
        checksum,
        path_count,
        intents: BTreeSet::new(),
        rolled_back: BTreeSet::new(),
        finished: false,
    })
}

fn apply_record(replay: &mut UndoReplay, record: &Record) -> Result<(), WorktreeError> {
    if replay.finished {
        return Err(corrupt("undo journal contains records after Finished"));
    }
    match record {
        Record::RollbackIntent { index } => {
            require_index(replay, *index)?;
            if !replay.intents.insert(*index) {
                return Err(corrupt("duplicate undo rollback intent"));
            }
        }
        Record::RolledBack { index } => {
            require_index(replay, *index)?;
            if !replay.intents.contains(index) || !replay.rolled_back.insert(*index) {
                return Err(corrupt("undo completion without a matching intent"));
            }
        }
        Record::Finished {
            outcome: FinishOutcome::RolledBack,
        } => {
            if replay.rolled_back.len() != replay.path_count || replay.intents != replay.rolled_back
            {
                return Err(corrupt("undo journal finished before every path"));
            }
            replay.finished = true;
        }
        _ => return Err(corrupt("undo journal contains a foreign record")),
    }
    Ok(())
}

fn require_index(replay: &UndoReplay, index: u32) -> Result<(), WorktreeError> {
    if usize::try_from(index).map_or(true, |index| index >= replay.path_count) {
        Err(corrupt("undo record references an unknown path index"))
    } else {
        Ok(())
    }
}

fn corrupt(message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::JournalCorrupt,
        TransactionPhase::Recover,
        message,
    )
    .requiring_recovery()
}
