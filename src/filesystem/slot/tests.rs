use std::io::Write;

use super::TargetAccess;
use crate::{
    filesystem::{FsRoot, PresentEvidence, SlotEvidence, SlotProbe, SlotSnapshot},
    hash::Sha256Hash,
};

fn stage(access: &TargetAccess, name: &str, bytes: &[u8]) {
    let mut file = access.create_new(name).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

#[test]
fn probes_absence_and_installs_without_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let access = root.open_target("new.rs").unwrap();
    assert_eq!(access.probe_slot().unwrap(), SlotProbe::Absent);
    stage(&access, ".weavatrix-new.stage", b"new\n");

    access.install_absent_from(".weavatrix-new.stage").unwrap();

    assert_eq!(std::fs::read(temp.path().join("new.rs")).unwrap(), b"new\n");
    assert!(!temp.path().join(".weavatrix-new.stage").exists());
    let SlotProbe::Present(probe) = access.probe_slot().unwrap() else {
        panic!("installed target is absent");
    };
    assert_eq!(probe.bytes, 4);
}

#[test]
fn absent_install_never_clobbers_a_competing_destination() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("new.rs"), "external\n").unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let access = root.open_target("new.rs").unwrap();
    stage(&access, ".weavatrix-new.stage", b"planned\n");

    let error = access
        .install_absent_from(".weavatrix-new.stage")
        .unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        std::fs::read(temp.path().join("new.rs")).unwrap(),
        b"external\n"
    );
    assert!(temp.path().join(".weavatrix-new.stage").exists());
}

#[test]
fn exact_state_includes_hash_identity_size_and_permissions() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("file.rs"), "old\n").unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let access = root.open_target("file.rs").unwrap();
    let SlotSnapshot::Present(snapshot) = access.snapshot_slot(1024).unwrap() else {
        panic!("target is absent");
    };
    let expected = PresentEvidence {
        sha256: Sha256Hash::compute(&snapshot.source),
        bytes: snapshot.source.len() as u64,
        identity: snapshot.identity,
        permissions: snapshot.portable_permissions,
    };

    assert_eq!(
        access
            .verify_slot(SlotEvidence::Present(expected), 1024)
            .unwrap(),
        SlotEvidence::Present(expected)
    );
    std::fs::write(temp.path().join("file.rs"), "new\n").unwrap();
    assert!(
        access
            .verify_slot(SlotEvidence::Present(expected), 1024)
            .is_err()
    );
}

#[test]
fn recognizes_the_two_link_install_crash_intermediate() {
    let temp = tempfile::tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let access = root.open_target("new.rs").unwrap();
    stage(&access, ".weavatrix-linked.stage", b"new\n");
    access
        .parent
        .hard_link(".weavatrix-linked.stage", &access.parent, "new.rs")
        .unwrap();

    assert!(
        access
            .same_file_as_artifact(".weavatrix-linked.stage")
            .unwrap()
    );
    let identity = access
        .safe_artifact_identity(".weavatrix-linked.stage", 2)
        .unwrap();
    access
        .finish_linked_install(".weavatrix-linked.stage", identity)
        .unwrap();
    assert!(!temp.path().join(".weavatrix-linked.stage").exists());
    assert!(temp.path().join("new.rs").exists());
}

#[test]
fn reverses_the_two_link_install_crash_intermediate() {
    let temp = tempfile::tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let access = root.open_target("new.rs").unwrap();
    stage(&access, ".weavatrix-linked.stage", b"new\n");
    access
        .parent
        .hard_link(".weavatrix-linked.stage", &access.parent, "new.rs")
        .unwrap();

    access
        .rollback_linked_install(".weavatrix-linked.stage")
        .unwrap();

    assert!(!temp.path().join("new.rs").exists());
    assert!(temp.path().join(".weavatrix-linked.stage").exists());
}

#[test]
fn root_revalidates_the_original_parent_identity() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("src")).unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let access = root.open_target("src/new.rs").unwrap();

    root.revalidate_parent(&access).unwrap();
    assert_eq!(
        access.parent_identity(),
        access.verify_parent_handle().unwrap()
    );
}

#[test]
fn permission_and_slot_evidence_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("file.rs"), "old\n").unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let access = root.open_target("file.rs").unwrap();
    let SlotSnapshot::Present(snapshot) = access.snapshot_slot(1024).unwrap() else {
        panic!("target is absent");
    };
    let evidence = SlotEvidence::Present(PresentEvidence {
        sha256: Sha256Hash::compute(&snapshot.source),
        bytes: snapshot.source.len() as u64,
        identity: snapshot.identity,
        permissions: snapshot.portable_permissions,
    });

    let encoded = blazingly_json::to_vec(&evidence).unwrap();
    let decoded: SlotEvidence = blazingly_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, evidence);
}
