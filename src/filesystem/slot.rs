use std::io::{self, Read};

use cap_fs_ext::MetadataExt;

use super::{
    FileIdentity, ParentIdentity, PortablePermissions, PresentEvidence, SlotEvidence, SlotProbe,
    SlotSnapshot, TargetAccess,
    target::{identity, invalid, is_link_or_reparse, validate_artifact_name, validate_metadata},
};
use crate::hash::Sha256Hash;

impl TargetAccess {
    pub(crate) fn probe_slot(&self) -> io::Result<SlotProbe> {
        let metadata = match self.parent.symlink_metadata(&self.name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(SlotProbe::Absent),
            Err(error) => return Err(error),
        };
        if is_link_or_reparse(&metadata) {
            return Err(invalid("target is a symbolic link or reparse point"));
        }
        validate_metadata(&metadata, self.root_device)?;
        Ok(SlotProbe::Present(super::target::TargetProbe {
            bytes: metadata.len(),
            identity: identity(&metadata),
            permissions: PortablePermissions::from_permissions(&metadata.permissions()),
        }))
    }

    pub(crate) fn snapshot_slot(&self, max_bytes: usize) -> io::Result<SlotSnapshot> {
        match self.probe_slot()? {
            SlotProbe::Absent => Ok(SlotSnapshot::Absent),
            SlotProbe::Present(_) => self.snapshot(max_bytes).map(SlotSnapshot::Present),
        }
    }

    pub(crate) fn verify_slot(
        &self,
        expected: SlotEvidence,
        max_bytes: usize,
    ) -> io::Result<SlotEvidence> {
        let actual = self.slot_evidence(max_bytes)?;
        if actual == expected {
            Ok(actual)
        } else {
            Err(invalid("target does not match its exact expected state"))
        }
    }

    pub(crate) fn slot_evidence(&self, max_bytes: usize) -> io::Result<SlotEvidence> {
        Ok(match self.snapshot_slot(max_bytes)? {
            SlotSnapshot::Absent => SlotEvidence::Absent,
            SlotSnapshot::Present(snapshot) => SlotEvidence::Present(PresentEvidence {
                sha256: Sha256Hash::compute(&snapshot.source),
                bytes: snapshot.source.len() as u64,
                identity: snapshot.identity,
                permissions: snapshot.portable_permissions,
            }),
        })
    }

    /// Installs a staged file only if the destination is still absent.
    pub(crate) fn install_absent_from(&self, artifact: &str) -> io::Result<()> {
        let artifact_identity = self.link_absent_from(artifact)?;
        self.finish_linked_install(artifact, artifact_identity)
    }

    /// Performs and synchronizes only the no-clobber link step.
    pub(crate) fn link_absent_from(&self, artifact: &str) -> io::Result<FileIdentity> {
        validate_artifact_name(artifact)?;
        require_absent(self)?;
        let artifact_identity = self.safe_artifact_identity(artifact, 1)?;
        self.parent.hard_link(artifact, &self.parent, &self.name)?;
        self.sync_parent()?;
        if !self.same_file_as_artifact(artifact)? {
            return Err(invalid(
                "installed destination does not alias its staged artifact",
            ));
        }
        Ok(artifact_identity)
    }

    /// Completes a no-clobber install found at its two-link crash point.
    pub(crate) fn finish_linked_install(
        &self,
        artifact: &str,
        expected_identity: FileIdentity,
    ) -> io::Result<()> {
        if !self.same_file_as_artifact(artifact)? {
            return Err(invalid("target is not the staged two-link install"));
        }
        self.parent.remove_file(artifact)?;
        self.sync_parent()?;
        let probe = self.probe()?;
        if probe.identity != expected_identity {
            return Err(invalid(
                "installed destination identity changed during finalization",
            ));
        }
        Ok(())
    }

    /// Reverses a no-clobber install found at its two-link crash point.
    pub(crate) fn rollback_linked_install(&self, artifact: &str) -> io::Result<()> {
        if !self.same_file_as_artifact(artifact)? {
            return Err(invalid("target is not the staged two-link install"));
        }
        self.parent.remove_file(&self.name)?;
        self.sync_parent()?;
        require_absent(self)?;
        self.safe_artifact_identity(artifact, 1).map(drop)
    }

    /// Reads exact evidence from the two-link no-clobber crash intermediate.
    pub(crate) fn linked_artifact_evidence(
        &self,
        artifact: &str,
        max_bytes: usize,
    ) -> io::Result<PresentEvidence> {
        if !self.same_file_as_artifact(artifact)? {
            return Err(invalid("target is not the staged two-link install"));
        }
        let mut file = self.open_artifact(artifact)?;
        let metadata = file.metadata()?;
        if is_link_or_reparse(&metadata)
            || !metadata.is_file()
            || metadata.dev() != self.root_device
            || metadata.nlink() != 2
        {
            return Err(invalid("linked install artifact is not safe"));
        }
        let expected_identity = identity(&metadata);
        let limit = u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .checked_add(1)
            .ok_or_else(|| invalid("linked artifact byte limit overflow"))?;
        let mut bytes = Vec::with_capacity(
            usize::try_from(metadata.len())
                .unwrap_or(max_bytes)
                .min(max_bytes),
        );
        file.by_ref().take(limit).read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(invalid(
                "linked transaction artifact exceeds its byte limit",
            ));
        }
        let final_metadata = file.metadata()?;
        if identity(&final_metadata) != expected_identity
            || final_metadata.nlink() != 2
            || !self.same_file_as_artifact(artifact)?
        {
            return Err(invalid("linked install changed while it was inspected"));
        }
        Ok(PresentEvidence {
            sha256: Sha256Hash::compute(&bytes),
            bytes: bytes.len() as u64,
            identity: expected_identity,
            permissions: PortablePermissions::from_permissions(&metadata.permissions()),
        })
    }

    /// Atomically replaces a present target with a verified single-link artifact.
    pub(crate) fn replace_from(&self, artifact: &str) -> io::Result<()> {
        self.safe_artifact_identity(artifact, 1)?;
        self.rename_from(artifact)
    }

    /// Removes a target only after checking its complete exact state.
    pub(crate) fn remove_exact(
        &self,
        expected: PresentEvidence,
        max_bytes: usize,
    ) -> io::Result<()> {
        self.verify_slot(SlotEvidence::Present(expected), max_bytes)?;
        self.parent.remove_file(&self.name)?;
        self.sync_parent()?;
        require_absent(self)
    }

    /// Detects the crash intermediate where target and stage are two links.
    pub(crate) fn same_file_as_artifact(&self, artifact: &str) -> io::Result<bool> {
        validate_artifact_name(artifact)?;
        let target = self.parent.symlink_metadata(&self.name)?;
        let staged = self.parent.symlink_metadata(artifact)?;
        if is_link_or_reparse(&target) || is_link_or_reparse(&staged) {
            return Err(invalid("linked install contains a link or reparse point"));
        }
        if !target.is_file()
            || !staged.is_file()
            || target.dev() != self.root_device
            || staged.dev() != self.root_device
        {
            return Err(invalid("linked install is not a safe regular file"));
        }
        Ok(identity(&target) == identity(&staged) && target.nlink() == 2 && staged.nlink() == 2)
    }

    pub(crate) fn verify_parent_handle(&self) -> io::Result<ParentIdentity> {
        let metadata = self.parent.dir_metadata()?;
        if is_link_or_reparse(&metadata) || metadata.dev() != self.root_device {
            return Err(invalid("target parent handle is no longer safe"));
        }
        let actual = ParentIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        if actual != self.parent_identity {
            return Err(invalid("target parent identity changed"));
        }
        Ok(actual)
    }

    pub(crate) fn artifact_evidence(
        &self,
        artifact: &str,
        max_bytes: usize,
    ) -> io::Result<PresentEvidence> {
        let bytes = self.read_artifact(artifact, max_bytes)?;
        let identity = self.safe_artifact_identity(artifact, 1)?;
        let metadata = self.parent.symlink_metadata(artifact)?;
        Ok(PresentEvidence {
            sha256: Sha256Hash::compute(&bytes),
            bytes: bytes.len() as u64,
            identity,
            permissions: PortablePermissions::from_permissions(&metadata.permissions()),
        })
    }

    pub(super) fn safe_artifact_identity(
        &self,
        artifact: &str,
        expected_links: u64,
    ) -> io::Result<FileIdentity> {
        validate_artifact_name(artifact)?;
        let metadata = self.parent.symlink_metadata(artifact)?;
        if is_link_or_reparse(&metadata)
            || !metadata.is_file()
            || metadata.dev() != self.root_device
            || metadata.nlink() != expected_links
        {
            return Err(invalid("transaction artifact identity is not safe"));
        }
        Ok(identity(&metadata))
    }
}

fn require_absent(access: &TargetAccess) -> io::Result<()> {
    match access.probe_slot()? {
        SlotProbe::Absent => Ok(()),
        SlotProbe::Present(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "target must remain absent",
        )),
    }
}

#[cfg(test)]
mod tests;
