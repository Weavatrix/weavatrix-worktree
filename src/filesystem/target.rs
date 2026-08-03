use std::io::{self, Read};

use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, OpenOptions, Permissions};

use super::{FileIdentity, ParentIdentity, PortablePermissions, RESERVED_ROOTS, sync_directory};

mod metadata;

pub(super) use metadata::{
    identity, invalid, is_link_or_reparse, validate_artifact_name, validate_metadata,
};

pub(crate) struct TargetSnapshot {
    pub(crate) source: Vec<u8>,
    pub(crate) identity: FileIdentity,
    pub(crate) permissions: Permissions,
    pub(crate) portable_permissions: PortablePermissions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TargetProbe {
    pub(crate) bytes: u64,
    pub(crate) identity: FileIdentity,
    pub(crate) permissions: PortablePermissions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SlotProbe {
    Absent,
    Present(TargetProbe),
}

pub(crate) enum SlotSnapshot {
    Absent,
    Present(TargetSnapshot),
}

pub(crate) struct TargetAccess {
    pub(super) parent: Dir,
    pub(super) name: String,
    pub(super) path: String,
    pub(super) root_device: u64,
    pub(super) parent_identity: ParentIdentity,
}

impl TargetAccess {
    pub(crate) fn open(root: &Dir, root_device: u64, path: &str) -> io::Result<Self> {
        weavatrix_refactor_plan::validate_plan_path(path, 4_096)
            .map_err(|error| invalid(error.to_string()))?;
        let portable_path = weavatrix_refactor_plan::portable_path_key(path);
        let first = portable_path
            .split('/')
            .next()
            .expect("validated path has a first segment");
        if RESERVED_ROOTS.contains(&first) {
            return Err(invalid("path is reserved for worktree transaction state"));
        }
        let reserved_identities = reserved_root_identities(root)?;
        let mut segments = path.split('/').peekable();
        let mut parent = root.try_clone()?;
        let mut name = None;
        while let Some(segment) = segments.next() {
            if segments.peek().is_none() {
                name = Some(segment.to_owned());
            } else {
                let metadata = parent.symlink_metadata(segment)?;
                if is_link_or_reparse(&metadata) {
                    return Err(invalid("target parent is a symbolic link or reparse point"));
                }
                parent = match parent.open_dir_nofollow(segment) {
                    Ok(dir) => dir,
                    Err(open_error) => {
                        if parent
                            .symlink_metadata(segment)
                            .is_ok_and(|metadata| is_link_or_reparse(&metadata))
                        {
                            return Err(invalid(
                                "target parent is a symbolic link or reparse point",
                            ));
                        }
                        return Err(open_error);
                    }
                };
                let metadata = parent.dir_metadata()?;
                if is_link_or_reparse(&metadata) {
                    return Err(invalid("target parent is a symbolic link or reparse point"));
                }
                if metadata.dev() != root_device {
                    return Err(invalid("target parent crosses the worktree filesystem"));
                }
                reject_reserved_identity(&metadata, &reserved_identities)?;
            }
        }
        let parent_metadata = parent.dir_metadata()?;
        if is_link_or_reparse(&parent_metadata) || parent_metadata.dev() != root_device {
            return Err(invalid("target parent identity is not safe"));
        }
        let name = name.ok_or_else(|| invalid("target path has no file name"))?;
        match parent.symlink_metadata(&name) {
            Ok(metadata) => reject_reserved_identity(&metadata, &reserved_identities)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(Self {
            parent,
            name,
            path: path.to_owned(),
            root_device,
            parent_identity: ParentIdentity {
                device: parent_metadata.dev(),
                inode: parent_metadata.ino(),
            },
        })
    }

    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            parent: self.parent.try_clone()?,
            name: self.name.clone(),
            path: self.path.clone(),
            root_device: self.root_device,
            parent_identity: self.parent_identity,
        })
    }

    pub(crate) fn snapshot(&self, max_bytes: usize) -> io::Result<TargetSnapshot> {
        let mut file = self.open_file()?;
        let metadata = file.metadata()?;
        validate_metadata(&metadata, self.root_device)?;
        let mut source = Vec::with_capacity(
            usize::try_from(metadata.len())
                .unwrap_or(max_bytes)
                .min(max_bytes),
        );
        let limit = u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .checked_add(1)
            .ok_or_else(|| invalid("source byte limit overflow"))?;
        file.by_ref().take(limit).read_to_end(&mut source)?;
        if source.len() > max_bytes {
            return Err(invalid("source exceeds the per-file byte limit"));
        }
        let permissions = metadata.permissions();
        Ok(TargetSnapshot {
            source,
            identity: identity(&metadata),
            portable_permissions: PortablePermissions::from_permissions(&permissions),
            permissions,
        })
    }

    pub(crate) fn probe(&self) -> io::Result<TargetProbe> {
        let file = self.open_file()?;
        let metadata = file.metadata()?;
        validate_metadata(&metadata, self.root_device)?;
        Ok(TargetProbe {
            bytes: metadata.len(),
            identity: identity(&metadata),
            permissions: PortablePermissions::from_permissions(&metadata.permissions()),
        })
    }

    pub(crate) fn create_new(&self, name: &str) -> io::Result<File> {
        validate_artifact_name(name)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        options.follow(FollowSymlinks::No);
        self.parent.open_with(name, &options)
    }

    pub(crate) fn open_artifact(&self, name: &str) -> io::Result<File> {
        validate_artifact_name(name)?;
        let mut options = OpenOptions::new();
        options.read(true);
        options.follow(FollowSymlinks::No);
        self.parent.open_with(name, &options)
    }

    pub(crate) fn read_artifact(&self, name: &str, max_bytes: usize) -> io::Result<Vec<u8>> {
        let mut file = self.open_artifact(name)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.dev() != self.root_device || metadata.nlink() != 1 {
            return Err(invalid("transaction artifact identity is not safe"));
        }
        let limit = u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .checked_add(1)
            .ok_or_else(|| invalid("artifact byte limit overflow"))?;
        let mut bytes = Vec::with_capacity(
            usize::try_from(metadata.len())
                .unwrap_or(max_bytes)
                .min(max_bytes),
        );
        file.by_ref().take(limit).read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(invalid("transaction artifact exceeds its byte limit"));
        }
        Ok(bytes)
    }

    pub(crate) fn rename_from(&self, artifact: &str) -> io::Result<()> {
        validate_artifact_name(artifact)?;
        self.parent.rename(artifact, &self.parent, &self.name)
    }

    pub(crate) fn remove_artifact(&self, name: &str) -> io::Result<bool> {
        validate_artifact_name(name)?;
        let metadata = match self.parent.symlink_metadata(name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if is_link_or_reparse(&metadata)
            || !metadata.is_file()
            || metadata.dev() != self.root_device
            || metadata.nlink() != 1
        {
            return Err(invalid("refusing to remove an unsafe transaction artifact"));
        }
        self.parent.remove_file(name)?;
        self.sync_parent()?;
        Ok(true)
    }

    pub(crate) fn sync_parent(&self) -> io::Result<()> {
        sync_directory(&self.parent)
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) const fn parent_identity(&self) -> ParentIdentity {
        self.parent_identity
    }

    fn open_file(&self) -> io::Result<File> {
        let metadata = self.parent.symlink_metadata(&self.name)?;
        if is_link_or_reparse(&metadata) {
            return Err(invalid("target is a symbolic link or reparse point"));
        }
        validate_metadata(&metadata, self.root_device)?;
        let expected_identity = identity(&metadata);
        let mut options = OpenOptions::new();
        options.read(true);
        options.follow(FollowSymlinks::No);
        let file = self.parent.open_with(&self.name, &options)?;
        let opened_metadata = file.metadata()?;
        validate_metadata(&opened_metadata, self.root_device)?;
        if identity(&opened_metadata) != expected_identity {
            return Err(invalid("target identity changed while it was opened"));
        }
        Ok(file)
    }
}

fn reserved_root_identities(root: &Dir) -> io::Result<[Option<FileIdentity>; 2]> {
    let mut identities = [None, None];
    for (index, name) in RESERVED_ROOTS.into_iter().enumerate() {
        identities[index] = match root.symlink_metadata(name) {
            Ok(metadata) => Some(identity(&metadata)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
    }
    Ok(identities)
}

fn reject_reserved_identity(
    metadata: &cap_std::fs::Metadata,
    reserved: &[Option<FileIdentity>; 2],
) -> io::Result<()> {
    if reserved.contains(&Some(identity(metadata))) {
        Err(invalid("path resolves through a reserved worktree root"))
    } else {
        Ok(())
    }
}
