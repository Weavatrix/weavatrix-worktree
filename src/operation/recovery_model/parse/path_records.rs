use crate::{
    error::{TransactionPhase, WorktreeError, WorktreeErrorCode},
    operation::journal::Record,
};
use weavatrix_refactor_plan::validate_plan_path;

use super::{Phase, Replay, corrupt, require_phase};
use crate::operation::recovery_model::{
    RecoveryPath,
    validate::{
        parse_state, validate_artifact_contract, validate_expected_path, validate_expected_paths,
        validate_staged_identity,
    },
};

impl Replay<'_> {
    pub(super) fn apply_path_record(&mut self, record: &Record) -> Result<(), WorktreeError> {
        match record {
            Record::PathIntent {
                index,
                path,
                before,
                after,
                stage_name,
                backup_name,
            } => self.path_intent(
                *index,
                path,
                before,
                after,
                stage_name.clone(),
                backup_name.clone(),
            ),
            Record::PathStaged {
                index,
                stage_identity,
                backup_identity,
            } => self.path_staged(*index, *stage_identity, *backup_identity),
            Record::Prepared {
                operation_count,
                path_count,
            } => self.prepared(*operation_count, *path_count),
            _ => Err(corrupt("non-path record reached path replay")),
        }
    }

    fn path_intent(
        &mut self,
        index: u32,
        path: &str,
        before: &crate::operation::journal::StateRecord,
        after: &crate::operation::journal::StateRecord,
        stage_name: Option<String>,
        backup_name: Option<String>,
    ) -> Result<(), WorktreeError> {
        if self.phase == Phase::Operations {
            if self.operation_count != self.expected_operation_count {
                return Err(corrupt("PathIntent appeared before every Operation"));
            }
            self.phase = Phase::Paths;
        }
        require_phase(self.phase, Phase::Paths, "PathIntent")?;
        if usize::try_from(index).ok() != Some(self.paths.len())
            || index >= self.expected_path_count
        {
            return Err(corrupt("path records must have contiguous indices"));
        }
        validate_plan_path(path, 4_096).map_err(|_| corrupt("journal contains an unsafe path"))?;
        let key = weavatrix_refactor_plan::portable_path_key(path);
        if !self.path_keys.insert(key.clone()) {
            return Err(corrupt("journal paths alias portably"));
        }
        if self.path_keys.iter().next_back() != Some(&key) {
            return Err(corrupt("journal paths are not in deterministic order"));
        }
        let before = parse_state(before, self.options, true)?;
        let after = parse_state(after, self.options, false)?;
        validate_expected_path(&self.expected_paths, path, &before, &after)?;
        validate_artifact_contract(
            &self.transaction_id,
            index,
            &before,
            &after,
            stage_name.as_deref(),
            backup_name.as_deref(),
        )?;
        let access = self.root.open_target(path).map_err(|error| {
            WorktreeError::with_source(
                WorktreeErrorCode::RecoveryRequired,
                TransactionPhase::Recover,
                "failed to reopen an operation journal path",
                error,
            )
            .at_path(path.to_owned())
            .at_file(usize::try_from(index).unwrap_or(usize::MAX))
            .in_transaction(self.transaction_id.clone())
            .requiring_recovery()
        })?;
        self.paths.push(RecoveryPath {
            index,
            path: path.to_owned(),
            access,
            before,
            after,
            stage_name,
            backup_name,
            stage_identity: None,
            backup_identity: None,
            staged: false,
        });
        Ok(())
    }

    fn path_staged(
        &mut self,
        index: u32,
        stage_identity: Option<crate::filesystem::FileIdentity>,
        backup_identity: Option<crate::filesystem::FileIdentity>,
    ) -> Result<(), WorktreeError> {
        if self.phase == Phase::Paths {
            if usize::try_from(self.expected_path_count).ok() != Some(self.paths.len()) {
                return Err(corrupt("PathStaged appeared before every PathIntent"));
            }
            self.phase = Phase::Staged;
        }
        require_phase(self.phase, Phase::Staged, "PathStaged")?;
        let Some(path) = self.paths.get_mut(self.staged_count) else {
            return Err(corrupt("PathStaged exceeds the declared path count"));
        };
        if path.index != index || path.staged {
            return Err(corrupt("PathStaged is duplicate or out of order"));
        }
        validate_staged_identity(path, stage_identity, backup_identity)?;
        path.stage_identity = stage_identity;
        path.backup_identity = backup_identity;
        path.staged = true;
        self.staged_count += 1;
        Ok(())
    }

    fn prepared(&mut self, operation_count: u32, path_count: u32) -> Result<(), WorktreeError> {
        require_phase(self.phase, Phase::Staged, "Prepared")?;
        if operation_count != self.expected_operation_count
            || path_count != self.expected_path_count
            || self.operation_count != self.expected_operation_count
            || usize::try_from(self.expected_path_count).ok() != Some(self.paths.len())
            || self.staged_count != self.paths.len()
        {
            return Err(corrupt(
                "Prepared does not cover the complete operation plan",
            ));
        }
        validate_expected_paths(&self.expected_paths, &self.paths)?;
        self.prepared = true;
        self.phase = Phase::Prepared;
        Ok(())
    }
}
