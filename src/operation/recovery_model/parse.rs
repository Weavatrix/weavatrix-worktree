mod path_records;
mod transaction_records;

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    filesystem::FsRoot,
    options::WorktreeOptions,
};

use super::{
    ParsedJournal, RecoveryPath,
    operation::ExpectedPath,
    validate::{Header, parse_header, validate_expected_paths},
};
use crate::operation::journal::{Entry, Record};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Phase {
    Operations,
    Paths,
    Staged,
    Prepared,
    Commit,
    Rollback,
    Finished,
}

pub(super) struct Replay<'root> {
    pub(super) root: &'root FsRoot,
    pub(super) options: WorktreeOptions,
    pub(super) transaction_id: String,
    pub(super) expected_operation_count: u32,
    pub(super) expected_path_count: u32,
    pub(super) phase: Phase,
    pub(super) operation_count: u32,
    pub(super) expected_paths: BTreeMap<String, ExpectedPath>,
    pub(super) inputs: BTreeSet<String>,
    pub(super) outputs: BTreeSet<String>,
    pub(super) paths: Vec<RecoveryPath>,
    pub(super) path_keys: BTreeSet<String>,
    pub(super) staged_count: usize,
    pub(super) prepared: bool,
    pub(super) commit_intents: BTreeSet<u32>,
    pub(super) committed: BTreeSet<u32>,
    pub(super) rollback_intents: BTreeSet<u32>,
    pub(super) rolled_back: BTreeSet<u32>,
    pub(super) finished: Option<crate::journal::FinishOutcome>,
    pub(super) last_commit_intent: Option<u32>,
    pub(super) last_rollback_intent: Option<u32>,
}

pub(crate) fn parse_journal(
    root: &FsRoot,
    entries: &[Entry],
    options: WorktreeOptions,
) -> Result<ParsedJournal, WorktreeError> {
    let header = parse_header(entries, options)?;
    let mut replay = Replay::new(root, options, header);
    for entry in &entries[1..] {
        replay.apply_record(&entry.record)?;
    }
    replay.finish()
}

impl<'root> Replay<'root> {
    fn new(root: &'root FsRoot, options: WorktreeOptions, header: Header<'_>) -> Self {
        Self {
            root,
            options,
            transaction_id: header.transaction_id.to_owned(),
            expected_operation_count: header.operation_count,
            expected_path_count: header.path_count,
            phase: Phase::Operations,
            operation_count: 0,
            expected_paths: BTreeMap::new(),
            inputs: BTreeSet::new(),
            outputs: BTreeSet::new(),
            paths: Vec::new(),
            path_keys: BTreeSet::new(),
            staged_count: 0,
            prepared: false,
            commit_intents: BTreeSet::new(),
            committed: BTreeSet::new(),
            rollback_intents: BTreeSet::new(),
            rolled_back: BTreeSet::new(),
            finished: None,
            last_commit_intent: None,
            last_rollback_intent: None,
        }
    }

    fn apply_record(&mut self, record: &Record) -> Result<(), WorktreeError> {
        if self.phase == Phase::Finished {
            return Err(corrupt("operation journal contains records after Finished"));
        }
        match record {
            Record::Operation { .. } => self.apply_operation(record),
            Record::PathIntent { .. } | Record::PathStaged { .. } | Record::Prepared { .. } => {
                self.apply_path_record(record)
            }
            Record::CommitIntent { .. }
            | Record::Committed { .. }
            | Record::RollbackIntent { .. }
            | Record::RolledBack { .. }
            | Record::Finished { .. } => self.apply_transaction_record(record),
            Record::Header { .. } => Err(corrupt("duplicate operation journal header")),
        }
    }

    fn finish(self) -> Result<ParsedJournal, WorktreeError> {
        if self.prepared {
            validate_expected_paths(&self.expected_paths, &self.paths)?;
        }
        Ok(ParsedJournal {
            transaction_id: self.transaction_id,
            paths: self.paths,
            prepared: self.prepared,
            commit_intents: self.commit_intents,
            rollback_intents: self.rollback_intents,
            rolled_back: self.rolled_back,
            finished: self.finished,
        })
    }
}

pub(super) fn require_phase(
    actual: Phase,
    expected: Phase,
    record: &str,
) -> Result<(), WorktreeError> {
    if actual == expected {
        Ok(())
    } else {
        Err(corrupt(&format!("{record} appeared in the wrong phase")))
    }
}

pub(super) fn corrupt(message: &str) -> WorktreeError {
    WorktreeError::new(
        WorktreeErrorCode::JournalCorrupt,
        TransactionPhase::Recover,
        message,
    )
    .requiring_recovery()
}
