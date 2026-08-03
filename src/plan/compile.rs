use std::collections::BTreeMap;

use weavatrix_refactor_plan::{FileEdit, TextEdit, portable_path_key};

use crate::{CreateFile, WorktreeOperation, WorktreePlan, error::WorktreeError, hash::Sha256Hash};

mod endpoints;

use endpoints::{
    InputEndpoint, OutputEndpoint, build_transitions, insert_input, insert_output,
    validate_cross_roles,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InputRole {
    Modify,
    Delete,
    RenameSource { destination: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PlannedInput {
    Absent,
    Present {
        operation_index: usize,
        expected_sha256: Sha256Hash,
        role: InputRole,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PlannedOutput {
    Absent,
    Modify {
        operation_index: usize,
        file: FileEdit,
    },
    Create {
        operation_index: usize,
        file: CreateFile,
    },
    Rename {
        operation_index: usize,
        source: String,
        expected_source_sha256: Sha256Hash,
        edits: Vec<TextEdit>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PathTransition {
    pub(crate) path: String,
    pub(crate) before: PlannedInput,
    pub(crate) after: PlannedOutput,
}

pub(crate) fn compile_plan(plan: &WorktreePlan) -> Result<Vec<PathTransition>, WorktreeError> {
    let mut compiler = Compiler::new();
    for (index, operation) in plan.operations.iter().enumerate() {
        compiler.register(index, operation)?;
    }
    compiler.finish()
}

struct Compiler {
    paths: BTreeMap<String, String>,
    inputs: BTreeMap<String, InputEndpoint>,
    outputs: BTreeMap<String, OutputEndpoint>,
}

impl Compiler {
    fn new() -> Self {
        Self {
            paths: BTreeMap::new(),
            inputs: BTreeMap::new(),
            outputs: BTreeMap::new(),
        }
    }

    fn register(
        &mut self,
        index: usize,
        operation: &WorktreeOperation,
    ) -> Result<(), WorktreeError> {
        match operation {
            WorktreeOperation::Modify(file) => self.modify(index, file),
            WorktreeOperation::Create(file) => self.create(index, file),
            WorktreeOperation::Delete(file) => self.delete(index, file),
            WorktreeOperation::Rename(file) => self.rename(index, file),
        }
    }

    fn modify(&mut self, index: usize, file: &FileEdit) -> Result<(), WorktreeError> {
        let key = self.register_path(&file.path);
        insert_input(
            &mut self.inputs,
            &key,
            InputEndpoint::Modify {
                index,
                file: file.clone(),
            },
            index,
        )?;
        insert_output(
            &mut self.outputs,
            &key,
            OutputEndpoint::Modify {
                index,
                file: file.clone(),
            },
            index,
        )?;
        Ok(())
    }

    fn create(&mut self, index: usize, file: &CreateFile) -> Result<(), WorktreeError> {
        let key = self.register_path(&file.path);
        insert_output(
            &mut self.outputs,
            &key,
            OutputEndpoint::Create {
                index,
                file: file.clone(),
            },
            index,
        )
    }

    fn delete(&mut self, index: usize, file: &crate::DeleteFile) -> Result<(), WorktreeError> {
        let key = self.register_path(&file.path);
        insert_input(
            &mut self.inputs,
            &key,
            InputEndpoint::Delete {
                index,
                file: file.clone(),
            },
            index,
        )
    }

    fn rename(&mut self, index: usize, file: &crate::RenameFile) -> Result<(), WorktreeError> {
        let from = self.register_path(&file.from);
        let to = self.register_path(&file.to);
        insert_input(
            &mut self.inputs,
            &from,
            InputEndpoint::Rename {
                index,
                file: file.clone(),
            },
            index,
        )?;
        insert_output(
            &mut self.outputs,
            &to,
            OutputEndpoint::Rename {
                index,
                file: file.clone(),
            },
            index,
        )?;
        Ok(())
    }

    fn finish(self) -> Result<Vec<PathTransition>, WorktreeError> {
        validate_cross_roles(&self.inputs, &self.outputs, &self.paths)?;
        build_transitions(&self.paths, self.inputs, self.outputs)
    }

    fn register_path(&mut self, path: &str) -> String {
        let key = portable_path_key(path);
        self.paths
            .entry(key.clone())
            .or_insert_with(|| path.to_owned());
        key
    }
}
