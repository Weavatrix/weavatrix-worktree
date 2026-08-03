use std::fs;

use tempfile::{TempDir, tempdir};

use crate::{
    DeleteFile, RecoveryAction, Sha256Hash, WorktreeOperation, WorktreeOptions, WorktreePlan,
    filesystem::{FsRoot, SlotEvidence},
    operation::{
        PreparedWorktreeTransaction, journal::Record, prepare_operation_plan,
        recover_operation_transaction,
    },
};

fn prepare_linked_delete_restore(
    temp: &TempDir,
    root: &FsRoot,
    options: WorktreeOptions,
) -> PreparedWorktreeTransaction {
    fs::write(temp.path().join("old.rs"), "old").unwrap();
    let plan = WorktreePlan::new(
        "delete",
        vec![WorktreeOperation::Delete(DeleteFile::new(
            "old.rs",
            Sha256Hash::compute(b"old").to_string(),
        ))],
    );
    let mut prepared = prepare_operation_plan(root, options, &plan).unwrap();
    let path = &prepared.paths[0];
    let index = path.stable_index;
    let SlotEvidence::Present(before) = path.before else {
        panic!("delete input is absent");
    };
    let backup = path.backup_name.clone().unwrap();
    prepared
        .journal
        .append(&Record::CommitIntent { index })
        .unwrap();
    prepared.paths[0]
        .access
        .remove_exact(before, options.limits.max_source_bytes_per_file)
        .unwrap();
    prepared
        .journal
        .append(&Record::Committed { index })
        .unwrap();
    prepared
        .journal
        .append(&Record::RollbackIntent { index })
        .unwrap();
    prepared.paths[0].access.link_absent_from(&backup).unwrap();
    prepared
}

#[test]
fn recovery_finishes_a_two_link_delete_backup_restore() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = WorktreeOptions::default();
    let prepared = prepare_linked_delete_restore(&temp, &root, options);
    drop(prepared);

    let report = recover_operation_transaction(&root, options)
        .unwrap()
        .unwrap();

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert_eq!(
        fs::read_to_string(temp.path().join("old.rs")).unwrap(),
        "old"
    );
    assert!(!journal_exists(&temp));
}

#[test]
fn recovery_replays_after_linked_backup_finalization() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = WorktreeOptions::default();
    let prepared = prepare_linked_delete_restore(&temp, &root, options);
    let path = &prepared.paths[0];
    let backup = path.backup_name.as_deref().unwrap();
    let identity = path.backup.unwrap().identity;
    path.access.finish_linked_install(backup, identity).unwrap();
    drop(prepared);

    let report = recover_operation_transaction(&root, options)
        .unwrap()
        .unwrap();

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert_eq!(
        fs::read_to_string(temp.path().join("old.rs")).unwrap(),
        "old"
    );
    assert!(!journal_exists(&temp));
}

#[test]
fn recovery_refuses_a_tampered_two_link_delete_backup_restore() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = WorktreeOptions::default();
    let prepared = prepare_linked_delete_restore(&temp, &root, options);
    let backup = prepared.paths[0].backup_name.clone().unwrap();
    fs::write(temp.path().join(backup), "tampered").unwrap();
    drop(prepared);

    let error = recover_operation_transaction(&root, options).unwrap_err();

    assert!(error.recovery_required());
    assert_eq!(
        fs::read_to_string(temp.path().join("old.rs")).unwrap(),
        "tampered"
    );
}

#[test]
fn recovery_refuses_a_foreign_target_during_linked_delete_restore() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = WorktreeOptions::default();
    let prepared = prepare_linked_delete_restore(&temp, &root, options);
    fs::remove_file(temp.path().join("old.rs")).unwrap();
    fs::write(temp.path().join("old.rs"), "external").unwrap();
    drop(prepared);

    let error = recover_operation_transaction(&root, options).unwrap_err();

    assert!(error.recovery_required());
    assert_eq!(
        fs::read_to_string(temp.path().join("old.rs")).unwrap(),
        "external"
    );
}

fn journal_exists(temp: &TempDir) -> bool {
    temp.path()
        .join(".weavatrix/worktree/active-v3.jsonl")
        .exists()
}
