use std::{io, path::Path};

use cap_fs_ext::{DirExt, MetadataExt};
use cap_std::{ambient_authority, fs::Dir};

use super::{ControlDir, TargetAccess, sync_directory};

pub(crate) struct FsRoot {
    dir: Dir,
    device: u64,
}

impl FsRoot {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_dir() || is_ambient_link_or_reparse(&metadata) {
            return Err(invalid(
                "worktree root must be a real directory, not a link",
            ));
        }
        let canonical = std::fs::canonicalize(path)?;
        let dir = Dir::open_ambient_dir(canonical, ambient_authority())?;
        let handle_metadata = dir.dir_metadata()?;
        if !handle_metadata.is_dir() || is_cap_link_or_reparse(&handle_metadata) {
            return Err(invalid("worktree root changed while it was opened"));
        }
        let device = handle_metadata.dev();
        Ok(Self { dir, device })
    }

    pub(crate) fn open_target(&self, path: &str) -> io::Result<TargetAccess> {
        TargetAccess::open(&self.dir, self.device, path)
    }

    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            dir: self.dir.try_clone()?,
            device: self.device,
        })
    }

    /// Re-resolves a target parent from the root and compares its identity.
    pub(crate) fn revalidate_parent(&self, target: &TargetAccess) -> io::Result<()> {
        target.verify_parent_handle()?;
        let reopened = self.open_target(target.path())?;
        reopened.verify_parent_handle()?;
        if reopened.parent_identity() != target.parent_identity() {
            return Err(invalid(
                "target parent path now resolves to another directory",
            ));
        }
        Ok(())
    }

    pub(crate) fn open_control(&self, create: bool) -> io::Result<Option<ControlDir>> {
        let weavatrix = open_or_create_child(&self.dir, ".weavatrix", create)?;
        let Some(weavatrix) = weavatrix else {
            return Ok(None);
        };
        let worktree = open_or_create_child(&weavatrix, "worktree", create)?;
        Ok(worktree.map(ControlDir::new))
    }
}

fn open_or_create_child(parent: &Dir, name: &str, create: bool) -> io::Result<Option<Dir>> {
    let child = match parent.open_dir_nofollow(name) {
        Ok(dir) => Some(dir),
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
            match parent.create_dir(name) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            Some(parent.open_dir_nofollow(name)?)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if create && child.is_some() {
        // Persist each namespace edge (`root/.weavatrix` and then
        // `.weavatrix/worktree`) before a transaction may depend on it.
        sync_directory(parent)?;
    }
    Ok(child)
}

#[cfg(windows)]
fn is_cap_link_or_reparse(metadata: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_cap_link_or_reparse(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn is_ambient_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_ambient_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
