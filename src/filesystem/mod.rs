mod control;
mod durability;
mod evidence;
mod root;
mod slot;
mod target;

pub(crate) use control::ControlDir;
pub(crate) use durability::sync_directory;
pub(crate) use evidence::{
    FileIdentity, ParentIdentity, PortablePermissions, PresentEvidence, SlotEvidence,
};
pub(crate) use root::FsRoot;
pub(crate) use target::{SlotProbe, SlotSnapshot, TargetAccess};

pub(crate) const RESERVED_ROOTS: [&str; 2] = [".git", ".weavatrix"];
