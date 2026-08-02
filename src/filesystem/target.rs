use std::io::{self, Read};

use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, OpenOptions, Permissions};

use super::{STATE_DIR, sync_directory};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FileIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

pub(crate) struct TargetSnapshot {
    pub(crate) source: Vec<u8>,
    pub(crate) identity: FileIdentity,
    pub(crate) permissions: Permissions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TargetProbe {
    pub(crate) bytes: u64,
    pub(crate) identity: FileIdentity,
}

pub(crate) struct TargetAccess {
    parent: Dir,
    name: String,
    path: String,
    root_device: u64,
}

impl TargetAccess {
    pub(crate) fn open(root: &Dir, root_device: u64, path: &str) -> io::Result<Self> {
        weavatrix_edit::validate_plan_path(path, 4_096)
            .map_err(|error| invalid(error.to_string()))?;
        if path == STATE_DIR || path.starts_with(&format!("{STATE_DIR}/")) {
            return Err(invalid("path is reserved for worktree transaction state"));
        }
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
            }
        }
        Ok(Self {
            parent,
            name: name.ok_or_else(|| invalid("target path has no file name"))?,
            path: path.to_owned(),
            root_device,
        })
    }

    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            parent: self.parent.try_clone()?,
            name: self.name.clone(),
            path: self.path.clone(),
            root_device: self.root_device,
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
        Ok(TargetSnapshot {
            source,
            identity: identity(&metadata),
            permissions: metadata.permissions(),
        })
    }

    pub(crate) fn probe(&self) -> io::Result<TargetProbe> {
        let file = self.open_file()?;
        let metadata = file.metadata()?;
        validate_metadata(&metadata, self.root_device)?;
        Ok(TargetProbe {
            bytes: metadata.len(),
            identity: identity(&metadata),
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

fn validate_metadata(metadata: &cap_std::fs::Metadata, root_device: u64) -> io::Result<()> {
    if !metadata.is_file() {
        return Err(invalid("target is not a regular file"));
    }
    if metadata.dev() != root_device {
        return Err(invalid("target crosses the worktree filesystem"));
    }
    if metadata.nlink() != 1 {
        return Err(invalid("hard-linked targets are rejected"));
    }
    if metadata.permissions().readonly() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "read-only targets are rejected",
        ));
    }
    Ok(())
}

fn identity(metadata: &cap_std::fs::Metadata) -> FileIdentity {
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn validate_artifact_name(name: &str) -> io::Result<()> {
    if name.is_empty() || name.contains(['/', '\\']) || !name.starts_with(".weavatrix-") {
        return Err(invalid("invalid transaction artifact name"));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
