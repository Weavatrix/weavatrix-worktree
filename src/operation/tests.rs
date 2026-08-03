use std::fs;

use tempfile::tempdir;

use crate::{
    CreateFile, DeleteFile, RecoveryAction, Sha256Hash, WorktreeOperation, WorktreePlan,
    filesystem::{FsRoot, SlotEvidence},
};

use super::{journal::Record, prepare_operation_plan, recover_operation_transaction};

mod journal_compat;
mod linked_recovery;

fn create_plan(path: &str, contents: &str) -> WorktreePlan {
    WorktreePlan::new(
        "create",
        vec![WorktreeOperation::Create(CreateFile::new(path, contents))],
    )
}

#[test]
fn recovery_rolls_back_a_create_after_mutation_without_completion_record() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = crate::WorktreeOptions::default();
    let mut prepared =
        prepare_operation_plan(&root, options, &create_plan("new.rs", "new")).unwrap();
    let path = &prepared.paths[0];
    prepared
        .journal
        .append(&Record::CommitIntent {
            index: path.stable_index,
        })
        .unwrap();
    path.access
        .install_absent_from(path.stage_name.as_deref().unwrap())
        .unwrap();
    drop(prepared);

    let report = recover_operation_transaction(&root, options)
        .unwrap()
        .unwrap();

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert!(!temp.path().join("new.rs").exists());
    assert!(
        !temp
            .path()
            .join(".weavatrix/worktree/active-v3.jsonl")
            .exists()
    );
}

#[test]
fn recovery_verifies_and_reverses_the_two_link_create_crash_point() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = crate::WorktreeOptions::default();
    let mut prepared =
        prepare_operation_plan(&root, options, &create_plan("new.rs", "new")).unwrap();
    let path = &prepared.paths[0];
    prepared
        .journal
        .append(&Record::CommitIntent {
            index: path.stable_index,
        })
        .unwrap();
    path.access
        .link_absent_from(path.stage_name.as_deref().unwrap())
        .unwrap();
    drop(prepared);

    let report = recover_operation_transaction(&root, options)
        .unwrap()
        .unwrap();

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert!(!temp.path().join("new.rs").exists());
}

#[test]
fn recovery_refuses_a_tampered_two_link_create_intermediate() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = crate::WorktreeOptions::default();
    let mut prepared =
        prepare_operation_plan(&root, options, &create_plan("new.rs", "planned")).unwrap();
    let path = &prepared.paths[0];
    prepared
        .journal
        .append(&Record::CommitIntent {
            index: path.stable_index,
        })
        .unwrap();
    let stage = path.stage_name.clone().unwrap();
    path.access.link_absent_from(&stage).unwrap();
    fs::write(temp.path().join(&stage), "tampered").unwrap();
    drop(prepared);

    let error = recover_operation_transaction(&root, options).unwrap_err();

    assert!(error.recovery_required());
    assert_eq!(
        fs::read_to_string(temp.path().join("new.rs")).unwrap(),
        "tampered"
    );
    assert!(
        temp.path()
            .join(".weavatrix/worktree/active-v3.jsonl")
            .exists()
    );
}

#[test]
fn recovery_restores_an_interrupted_delete_from_backup() {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("old.rs"), "old").unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = crate::WorktreeOptions::default();
    let plan = WorktreePlan::new(
        "delete",
        vec![WorktreeOperation::Delete(DeleteFile::new(
            "old.rs",
            Sha256Hash::compute(b"old").to_string(),
        ))],
    );
    let mut prepared = prepare_operation_plan(&root, options, &plan).unwrap();
    let path = &prepared.paths[0];
    prepared
        .journal
        .append(&Record::CommitIntent {
            index: path.stable_index,
        })
        .unwrap();
    let SlotEvidence::Present(before) = path.before else {
        panic!("delete input is absent");
    };
    path.access
        .remove_exact(before, options.limits.max_source_bytes_per_file)
        .unwrap();
    drop(prepared);

    let report = recover_operation_transaction(&root, options)
        .unwrap()
        .unwrap();

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert_eq!(
        fs::read_to_string(temp.path().join("old.rs")).unwrap(),
        "old"
    );
}

#[test]
fn recovery_refuses_to_overwrite_a_foreign_create_destination() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = crate::WorktreeOptions::default();
    let mut prepared =
        prepare_operation_plan(&root, options, &create_plan("new.rs", "planned")).unwrap();
    prepared
        .journal
        .append(&Record::CommitIntent { index: 0 })
        .unwrap();
    fs::write(temp.path().join("new.rs"), "external").unwrap();
    drop(prepared);

    let error = recover_operation_transaction(&root, options).unwrap_err();

    assert!(error.recovery_required());
    assert_eq!(
        fs::read_to_string(temp.path().join("new.rs")).unwrap(),
        "external"
    );
    assert!(
        temp.path()
            .join(".weavatrix/worktree/active-v3.jsonl")
            .exists()
    );
}
