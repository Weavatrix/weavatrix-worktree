use serde::{Deserialize, Serialize};

use crate::hash::Sha256Hash;

/// Stable identity of an opened regular file on its filesystem.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub(crate) struct FileIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

/// Stable identity of the parent directory held by a target capability.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub(crate) struct ParentIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

/// Permission evidence that can be persisted and compared across recovery.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PortablePermissions {
    pub(crate) readonly: bool,
    pub(crate) unix_mode: Option<u32>,
}

impl PortablePermissions {
    pub(crate) fn from_permissions(permissions: &cap_std::fs::Permissions) -> Self {
        #[cfg(unix)]
        let unix_mode = {
            use cap_std::fs::PermissionsExt;
            Some(permissions.mode() & 0o7777)
        };
        #[cfg(not(unix))]
        let unix_mode = None;
        Self {
            readonly: permissions.readonly(),
            unix_mode,
        }
    }

    pub(crate) fn apply_to(self, permissions: &mut cap_std::fs::Permissions) {
        permissions.set_readonly(self.readonly);
        #[cfg(unix)]
        if let Some(mode) = self.unix_mode {
            use cap_std::fs::PermissionsExt;
            permissions.set_mode(mode);
        }
    }
}

/// Complete exact-state evidence for one present path.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PresentEvidence {
    pub(crate) sha256: Sha256Hash,
    pub(crate) bytes: u64,
    pub(crate) identity: FileIdentity,
    pub(crate) permissions: PortablePermissions,
}

/// Expected or observed state of one repository-relative path.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "evidence")]
pub(crate) enum SlotEvidence {
    Absent,
    Present(PresentEvidence),
}
