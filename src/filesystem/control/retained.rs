use std::io::{self, Read};

use cap_fs_ext::{FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
use cap_std::fs::OpenOptions;

use crate::Sha256Hash;
use crate::filesystem::{
    FileIdentity, PortablePermissions, PresentEvidence, SlotProbe, TargetAccess,
    target::{identity, invalid, is_link_or_reparse, validate_artifact_name},
};

use super::ControlDir;

impl ControlDir {
    /// Moves one exact retained backup out of a target directory and into the
    /// capability-scoped state directory. The operation is idempotent so a
    /// committed journal can finish a partially completed relocation.
    pub(crate) fn retain_backup_from(
        &self,
        target: &TargetAccess,
        name: &str,
        expected: PresentEvidence,
        max_bytes: usize,
    ) -> io::Result<()> {
        if target.root_device != self.device {
            return Err(invalid("retained backup crosses the worktree filesystem"));
        }
        let adjacent = optional_evidence(target.artifact_evidence(name, max_bytes))?;
        let retained = optional_evidence(self.backup_evidence(name, max_bytes))?;
        match (adjacent, retained) {
            (Some(actual), None) if actual == expected => {
                target.parent.rename(name, &self.dir, name)?;
                target.sync_parent()?;
                self.sync()?;
                if self.backup_evidence(name, max_bytes)? != expected {
                    return Err(invalid("relocated backup changed during retention"));
                }
                Ok(())
            }
            (None, Some(actual)) if actual == expected => Ok(()),
            (Some(_), Some(_)) => Err(invalid(
                "retained backup exists both beside the target and in state",
            )),
            (None, None) => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "retained backup is missing",
            )),
            _ => Err(invalid("retained backup does not match exact evidence")),
        }
    }

    pub(crate) fn backup_evidence(
        &self,
        name: &str,
        max_bytes: usize,
    ) -> io::Result<PresentEvidence> {
        self.backup_evidence_with_links(name, max_bytes, 1)
    }

    pub(crate) fn remove_backup(&self, name: &str) -> io::Result<bool> {
        validate_artifact_name(name)?;
        match self.safe_backup_identity(name, 1) {
            Ok(_) => {
                self.dir.remove_file(name)?;
                self.sync()?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Restores a retained backup over an existing exact target.
    pub(crate) fn replace_target_from_backup(
        &self,
        target: &TargetAccess,
        name: &str,
    ) -> io::Result<()> {
        let expected = self.safe_backup_identity(name, 1)?;
        if target.root_device != self.device {
            return Err(invalid("retained restore crosses the worktree filesystem"));
        }
        self.dir.rename(name, &target.parent, &target.name)?;
        target.sync_parent()?;
        self.sync()?;
        let actual = target.probe()?;
        if actual.identity != expected {
            return Err(invalid(
                "restored backup identity changed during replacement",
            ));
        }
        Ok(())
    }

    /// Starts the no-clobber restore of a retained backup into an absent slot.
    pub(crate) fn link_absent_target_from_backup(
        &self,
        target: &TargetAccess,
        name: &str,
    ) -> io::Result<FileIdentity> {
        match target.probe_slot()? {
            SlotProbe::Absent => {}
            SlotProbe::Present(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "target must remain absent",
                ));
            }
        }
        let expected = self.safe_backup_identity(name, 1)?;
        self.dir.hard_link(name, &target.parent, &target.name)?;
        target.sync_parent()?;
        if !self.same_file_as_target(target, name)? {
            return Err(invalid(
                "restored destination does not alias its state backup",
            ));
        }
        Ok(expected)
    }

    pub(crate) fn install_absent_target_from_backup(
        &self,
        target: &TargetAccess,
        name: &str,
    ) -> io::Result<()> {
        let expected = self.link_absent_target_from_backup(target, name)?;
        self.finish_linked_restore(target, name, expected)
    }

    /// Completes the two-link crash intermediate of an absent-slot restore.
    pub(crate) fn finish_linked_restore(
        &self,
        target: &TargetAccess,
        name: &str,
        expected: FileIdentity,
    ) -> io::Result<()> {
        if !self.same_file_as_target(target, name)? {
            return Err(invalid("target is not the retained two-link restore"));
        }
        self.dir.remove_file(name)?;
        self.sync()?;
        let actual = target.probe()?;
        if actual.identity != expected {
            return Err(invalid(
                "restored target identity changed during finalization",
            ));
        }
        Ok(())
    }

    pub(crate) fn same_file_as_target(
        &self,
        target: &TargetAccess,
        name: &str,
    ) -> io::Result<bool> {
        validate_artifact_name(name)?;
        let backup = self.dir.symlink_metadata(name)?;
        let target_metadata = target.parent.symlink_metadata(&target.name)?;
        if is_link_or_reparse(&backup) || is_link_or_reparse(&target_metadata) {
            return Err(invalid(
                "linked retained restore contains a link or reparse point",
            ));
        }
        if !backup.is_file()
            || !target_metadata.is_file()
            || backup.dev() != self.device
            || target_metadata.dev() != self.device
        {
            return Err(invalid(
                "linked retained restore is not a safe regular file",
            ));
        }
        Ok(identity(&backup) == identity(&target_metadata)
            && backup.nlink() == 2
            && target_metadata.nlink() == 2)
    }

    pub(crate) fn linked_backup_evidence(
        &self,
        target: &TargetAccess,
        name: &str,
        max_bytes: usize,
    ) -> io::Result<PresentEvidence> {
        if !self.same_file_as_target(target, name)? {
            return Err(invalid("target is not the retained two-link restore"));
        }
        self.backup_evidence_with_links(name, max_bytes, 2)
    }

    fn backup_evidence_with_links(
        &self,
        name: &str,
        max_bytes: usize,
        expected_links: u64,
    ) -> io::Result<PresentEvidence> {
        validate_artifact_name(name)?;
        let mut options = OpenOptions::new();
        options.read(true);
        options.follow(FollowSymlinks::No);
        let mut file = self.dir.open_with(name, &options)?;
        let metadata = file.metadata()?;
        let expected_identity = self.safe_backup_metadata(&metadata, expected_links)?;
        let limit = u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .checked_add(1)
            .ok_or_else(|| invalid("retained backup byte limit overflow"))?;
        let mut bytes = Vec::with_capacity(
            usize::try_from(metadata.len())
                .unwrap_or(max_bytes)
                .min(max_bytes),
        );
        file.by_ref().take(limit).read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(invalid("retained backup exceeds its byte limit"));
        }
        let final_metadata = file.metadata()?;
        if self.safe_backup_metadata(&final_metadata, expected_links)? != expected_identity {
            return Err(invalid("retained backup changed while it was inspected"));
        }
        Ok(PresentEvidence {
            sha256: Sha256Hash::compute(&bytes),
            bytes: bytes.len() as u64,
            identity: expected_identity,
            permissions: PortablePermissions::from_permissions(&metadata.permissions()),
        })
    }

    fn safe_backup_identity(&self, name: &str, expected_links: u64) -> io::Result<FileIdentity> {
        validate_artifact_name(name)?;
        let metadata = self.dir.symlink_metadata(name)?;
        self.safe_backup_metadata(&metadata, expected_links)
    }

    fn safe_backup_metadata(
        &self,
        metadata: &cap_std::fs::Metadata,
        expected_links: u64,
    ) -> io::Result<FileIdentity> {
        if is_link_or_reparse(metadata)
            || !metadata.is_file()
            || metadata.dev() != self.device
            || metadata.nlink() != expected_links
        {
            return Err(invalid("retained backup identity is not safe"));
        }
        Ok(identity(metadata))
    }
}

fn optional_evidence(result: io::Result<PresentEvidence>) -> io::Result<Option<PresentEvidence>> {
    match result {
        Ok(evidence) => Ok(Some(evidence)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
