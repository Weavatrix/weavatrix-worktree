use crate::{
    error::WorktreeError,
    operation::{
        journal::Record,
        recovery_model::{operation::validate_operation, validate::validate_finished},
    },
};

use super::{Phase, Replay, corrupt, require_phase};
use crate::operation::recovery_model::validate::validate_path_index;

impl Replay<'_> {
    pub(super) fn apply_operation(&mut self, record: &Record) -> Result<(), WorktreeError> {
        let Record::Operation {
            index,
            kind,
            source_path,
            destination_path,
            old_sha256,
            new_sha256,
            bytes_before,
            bytes_after,
            edit_count,
        } = record
        else {
            return Err(corrupt("non-operation record reached operation replay"));
        };
        require_phase(self.phase, Phase::Operations, "Operation")?;
        if *index != self.operation_count || *index >= self.expected_operation_count {
            return Err(corrupt("operation records must have contiguous indices"));
        }
        validate_operation(
            kind,
            source_path.as_deref(),
            destination_path.as_deref(),
            old_sha256.as_deref(),
            new_sha256.as_deref(),
            *bytes_before,
            *bytes_after,
            *edit_count,
            self.options,
            &mut self.expected_paths,
            &mut self.inputs,
            &mut self.outputs,
        )?;
        self.operation_count += 1;
        Ok(())
    }

    pub(super) fn apply_transaction_record(
        &mut self,
        record: &Record,
    ) -> Result<(), WorktreeError> {
        match record {
            Record::CommitIntent { index } => self.commit_intent(*index),
            Record::Committed { index } => self.committed(*index),
            Record::RollbackIntent { index } => self.rollback_intent(*index),
            Record::RolledBack { index } => self.rolled_back(*index),
            Record::Finished { outcome } => self.finished(outcome.clone()),
            _ => Err(corrupt("non-transaction record reached transaction replay")),
        }
    }

    fn commit_intent(&mut self, index: u32) -> Result<(), WorktreeError> {
        if !matches!(self.phase, Phase::Prepared | Phase::Commit) {
            return Err(corrupt("CommitIntent appeared outside the commit phase"));
        }
        validate_path_index(index, &self.paths)?;
        if self
            .last_commit_intent
            .is_some_and(|previous| !self.committed.contains(&previous))
            || !self.commit_intents.insert(index)
            || self
                .last_commit_intent
                .is_some_and(|previous| index <= previous)
        {
            return Err(corrupt("CommitIntent is duplicate or out of order"));
        }
        self.last_commit_intent = Some(index);
        self.phase = Phase::Commit;
        Ok(())
    }

    fn committed(&mut self, index: u32) -> Result<(), WorktreeError> {
        require_phase(self.phase, Phase::Commit, "Committed")?;
        if !self.commit_intents.contains(&index)
            || !self.committed.insert(index)
            || self.last_commit_intent != Some(index)
        {
            return Err(corrupt("Committed lacks its unique latest intent"));
        }
        Ok(())
    }

    fn rollback_intent(&mut self, index: u32) -> Result<(), WorktreeError> {
        if !matches!(self.phase, Phase::Commit | Phase::Rollback)
            || !self.commit_intents.contains(&index)
            || self
                .last_rollback_intent
                .is_some_and(|previous| !self.rolled_back.contains(&previous))
            || !self.rollback_intents.insert(index)
            || self
                .last_rollback_intent
                .is_some_and(|previous| index >= previous)
        {
            return Err(corrupt("RollbackIntent is duplicate or out of order"));
        }
        self.last_rollback_intent = Some(index);
        self.phase = Phase::Rollback;
        Ok(())
    }

    fn rolled_back(&mut self, index: u32) -> Result<(), WorktreeError> {
        require_phase(self.phase, Phase::Rollback, "RolledBack")?;
        if !self.rollback_intents.contains(&index)
            || !self.rolled_back.insert(index)
            || self.last_rollback_intent != Some(index)
        {
            return Err(corrupt("RolledBack lacks its unique latest intent"));
        }
        Ok(())
    }

    fn finished(&mut self, outcome: crate::journal::FinishOutcome) -> Result<(), WorktreeError> {
        validate_finished(
            &outcome,
            self.prepared,
            self.paths.len(),
            &self.commit_intents,
            &self.committed,
        )?;
        self.finished = Some(outcome);
        self.phase = Phase::Finished;
        Ok(())
    }
}
