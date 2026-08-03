use std::io;

use cap_fs_ext::MetadataExt;

use crate::filesystem::FileIdentity;

pub(crate) fn validate_metadata(
    metadata: &cap_std::fs::Metadata,
    root_device: u64,
) -> io::Result<()> {
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

pub(crate) fn identity(metadata: &cap_std::fs::Metadata) -> FileIdentity {
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(windows)]
pub(crate) fn is_link_or_reparse(metadata: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
pub(crate) fn is_link_or_reparse(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub(crate) fn validate_artifact_name(name: &str) -> io::Result<()> {
    if name.is_empty() || name.contains(['/', '\\']) || !name.starts_with(".weavatrix-") {
        return Err(invalid("invalid transaction artifact name"));
    }
    Ok(())
}

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
