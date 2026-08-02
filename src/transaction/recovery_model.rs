use std::collections::{HashMap, HashSet};

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::{FsRoot, TargetAccess},
    hash::Sha256Hash,
    journal::{FinishOutcome, JournalEntry, JournalRecord},
    options::WorktreeOptions,
};

use super::util::fs_error;

pub(super) struct RecoveryFile {
    pub(super) index: u32,
    pub(super) access: TargetAccess,
    pub(super) old_hash: Sha256Hash,
    pub(super) new_hash: Sha256Hash,
    pub(super) bytes_before: u64,
    pub(super) bytes_after: u64,
    pub(super) edit_count: u32,
    pub(super) stage_name: String,
    pub(super) backup_name: String,
}

pub(super) struct ParsedJournal {
    pub(super) transaction_id: String,
    pub(super) files: Vec<RecoveryFile>,
    pub(super) commit_intents: HashSet<u32>,
    pub(super) finished: Option<FinishOutcome>,
}

pub(super) fn parse_journal(
    root: &FsRoot,
    entries: &[JournalEntry],
    options: WorktreeOptions,
) -> Result<ParsedJournal, WorktreeError> {
    let JournalRecord::Header {
        transaction_id,
        file_count,
        ..
    } = &entries[0].record
    else {
        return Err(corrupt("journal does not begin with a header"));
    };
    if *file_count as usize > options.limits.max_files {
        return Err(corrupt("journal file count exceeds configured limits"));
    }
    let mut records = HashMap::new();
    let mut intents = HashSet::new();
    let mut committed = HashSet::new();
    let mut rollback_intents = HashSet::new();
    let mut finished = None;
    let mut prepared = false;
    for entry in &entries[1..] {
        if finished.is_some() {
            return Err(corrupt("journal contains records after Finished"));
        }
        match &entry.record {
            JournalRecord::PreparedFile { index, .. } => {
                if prepared
                    || *index >= *file_count
                    || records.insert(*index, &entry.record).is_some()
                {
                    return Err(corrupt("invalid or duplicate PreparedFile record"));
                }
            }
            JournalRecord::Prepared { file_count: count } => {
                if prepared || count != file_count || records.len() != *file_count as usize {
                    return Err(corrupt("Prepared does not cover every journal file"));
                }
                prepared = true;
            }
            JournalRecord::CommitIntent { index } => {
                if !prepared || !records.contains_key(index) || !intents.insert(*index) {
                    return Err(corrupt("invalid or duplicate CommitIntent"));
                }
            }
            JournalRecord::Committed { index } => {
                if !intents.contains(index) || !committed.insert(*index) {
                    return Err(corrupt("Committed lacks a unique matching intent"));
                }
            }
            JournalRecord::RollbackIntent { index } => {
                if !intents.contains(index) || !rollback_intents.insert(*index) {
                    return Err(corrupt("invalid or duplicate RollbackIntent"));
                }
            }
            JournalRecord::RolledBack { index } => {
                if !rollback_intents.contains(index) {
                    return Err(corrupt("RolledBack lacks a matching intent"));
                }
            }
            JournalRecord::Finished { outcome } => finished = Some(outcome.clone()),
            JournalRecord::Header { .. } => return Err(corrupt("duplicate journal header")),
        }
    }
    let mut files = records
        .into_values()
        .map(|record| recovery_file(root, record, options))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort_by_key(|file| file.index);
    Ok(ParsedJournal {
        transaction_id: transaction_id.clone(),
        files,
        commit_intents: intents,
        finished,
    })
}

fn recovery_file(
    root: &FsRoot,
    record: &JournalRecord,
    options: WorktreeOptions,
) -> Result<RecoveryFile, WorktreeError> {
    let JournalRecord::PreparedFile {
        index,
        path,
        old_sha256,
        new_sha256,
        bytes_before,
        bytes_after,
        edit_count,
        stage_name,
        backup_name,
    } = record
    else {
        return Err(corrupt("expected PreparedFile"));
    };
    if *bytes_before > options.limits.max_source_bytes_per_file as u64
        || *bytes_after > options.limits.max_output_bytes_per_file as u64
        || *edit_count as usize > options.limits.max_edits_per_file
    {
        return Err(corrupt("journal file evidence exceeds configured limits"));
    }
    let access = root.open_target(path).map_err(|error| {
        fs_error(
            TransactionPhase::Recover,
            path,
            *index as usize,
            "failed to reopen journal target",
            error,
        )
        .requiring_recovery()
    })?;
    Ok(RecoveryFile {
        index: *index,
        access,
        old_hash: Sha256Hash::parse(old_sha256).map_err(|_| corrupt("invalid old SHA-256"))?,
        new_hash: Sha256Hash::parse(new_sha256).map_err(|_| corrupt("invalid new SHA-256"))?,
        bytes_before: *bytes_before,
        bytes_after: *bytes_after,
        edit_count: *edit_count,
        stage_name: stage_name.clone(),
        backup_name: backup_name.clone(),
    })
}

fn corrupt(message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::JournalCorrupt,
        TransactionPhase::Recover,
        message,
    )
    .requiring_recovery()
}
