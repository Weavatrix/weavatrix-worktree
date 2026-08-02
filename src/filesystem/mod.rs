mod control;
mod durability;
mod root;
mod target;

pub(crate) use control::ControlDir;
pub(crate) use durability::sync_directory;
pub(crate) use root::FsRoot;
pub(crate) use target::{FileIdentity, TargetAccess};

pub(crate) const STATE_DIR: &str = ".weavatrix/worktree";
