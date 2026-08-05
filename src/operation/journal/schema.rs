use super::{JournalError, invalid};

pub(super) const CURRENT: &str = "weavatrix.worktree-journal.v3";
pub(super) const LEGACY: &str = "weavatrix.worktree-journal.v2";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum JournalSchema {
    V2,
    V3,
}

impl JournalSchema {
    pub(super) const fn current() -> Self {
        Self::V3
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::V2 => LEGACY,
            Self::V3 => CURRENT,
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, JournalError> {
        match value {
            LEGACY => Ok(Self::V2),
            CURRENT => Ok(Self::V3),
            _ => Err(invalid(format!(
                "unknown operation journal schema {value:?}"
            ))),
        }
    }
}
