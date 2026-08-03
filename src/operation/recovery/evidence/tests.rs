use super::matches_present_evidence;
use crate::{
    filesystem::{FileIdentity, PortablePermissions, PresentEvidence},
    hash::Sha256Hash,
    operation::recovery_model::PresentSpec,
};

#[test]
fn exact_match_includes_identity_and_permissions() {
    let identity = FileIdentity {
        device: 1,
        inode: 2,
    };
    let permissions = PortablePermissions {
        readonly: false,
        unix_mode: Some(0o644),
    };
    let expected = PresentSpec {
        sha256: Sha256Hash::compute(b"x"),
        bytes: 1,
        permissions,
        identity: Some(identity),
    };
    let actual = PresentEvidence {
        sha256: expected.sha256,
        bytes: 1,
        permissions,
        identity,
    };
    assert!(matches_present_evidence(&expected, actual, Some(identity)));
    assert!(!matches_present_evidence(
        &expected,
        PresentEvidence {
            identity: FileIdentity {
                device: 1,
                inode: 3,
            },
            ..actual
        },
        Some(identity)
    ));
}
