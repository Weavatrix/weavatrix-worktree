mod operation;
mod parse;
mod validate;

use std::collections::BTreeSet;

use crate::{
    filesystem::{FileIdentity, PortablePermissions, TargetAccess},
    hash::Sha256Hash,
    journal::FinishOutcome,
};

pub(super) use parse::parse_journal;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum StateSpec {
    Absent,
    Present(PresentSpec),
}

impl StateSpec {
    pub(super) const fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }

    pub(super) const fn present(&self) -> Option<&PresentSpec> {
        match self {
            Self::Present(present) => Some(present),
            Self::Absent => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PresentSpec {
    pub(super) sha256: Sha256Hash,
    pub(super) bytes: u64,
    pub(super) permissions: PortablePermissions,
    pub(super) identity: Option<FileIdentity>,
}

pub(super) struct RecoveryPath {
    pub(super) index: u32,
    pub(super) path: String,
    pub(super) access: TargetAccess,
    pub(super) before: StateSpec,
    pub(super) after: StateSpec,
    pub(super) stage_name: Option<String>,
    pub(super) backup_name: Option<String>,
    pub(super) stage_identity: Option<FileIdentity>,
    pub(super) backup_identity: Option<FileIdentity>,
    pub(super) staged: bool,
}

pub(super) struct ParsedJournal {
    pub(super) transaction_id: String,
    pub(super) paths: Vec<RecoveryPath>,
    pub(super) prepared: bool,
    pub(super) commit_intents: BTreeSet<u32>,
    pub(super) rollback_intents: BTreeSet<u32>,
    pub(super) rolled_back: BTreeSet<u32>,
    pub(super) finished: Option<FinishOutcome>,
}
