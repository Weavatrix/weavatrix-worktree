use std::sync::Arc;

use weavatrix_refactor_plan::TextEdit;

use crate::{
    filesystem::{PortablePermissions, PresentEvidence, SlotEvidence, TargetAccess},
    report::OperationChange,
};

pub(super) enum ProjectedInput {
    Absent,
    Present {
        source: Arc<str>,
        evidence: PresentEvidence,
    },
}

impl ProjectedInput {
    pub(super) const fn evidence(&self) -> SlotEvidence {
        match self {
            Self::Absent => SlotEvidence::Absent,
            Self::Present { evidence, .. } => SlotEvidence::Present(*evidence),
        }
    }

    pub(super) fn source(&self) -> Option<&Arc<str>> {
        match self {
            Self::Absent => None,
            Self::Present { source, .. } => Some(source),
        }
    }
}

pub(super) enum OutputRecipe {
    Exact(Arc<str>),
    Edited {
        source: Arc<str>,
        edits: Vec<TextEdit>,
    },
}

pub(super) struct ProjectedPresent {
    pub(super) recipe: OutputRecipe,
    pub(super) sha256: crate::Sha256Hash,
    pub(super) bytes: u64,
    pub(super) permissions: PortablePermissions,
    pub(super) edit_count: usize,
}

pub(super) enum ProjectedOutput {
    Absent,
    Present(ProjectedPresent),
}

pub(super) struct ProjectedPath {
    pub(super) stable_index: u32,
    pub(super) path: String,
    pub(super) access: TargetAccess,
    pub(super) before: ProjectedInput,
    pub(super) after: ProjectedOutput,
}

pub(super) struct ProjectedPlan {
    pub(super) operation: String,
    pub(super) operations: Vec<OperationChange>,
    pub(super) paths: Vec<ProjectedPath>,
}

pub(super) struct StagedPath {
    pub(super) stable_index: u32,
    pub(super) path: String,
    pub(super) access: TargetAccess,
    pub(super) before: SlotEvidence,
    pub(super) after: SlotEvidence,
    pub(super) backup: Option<PresentEvidence>,
    pub(super) stage_name: Option<String>,
    pub(super) backup_name: Option<String>,
}
