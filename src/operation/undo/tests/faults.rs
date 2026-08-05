use tempfile::TempDir;

use super::*;

const ROLLBACK_ID: &str = "0123456789abcdef0123456789abcdef";

fn retained_apply() -> (TempDir, String) {
    let (temp, plan) = fixture();
    let report = Worktree::open(temp.path())
        .unwrap()
        .apply_plan_retained(&plan, UndoRetention::default())
        .unwrap();
    (temp, report.undo_id().as_str().to_owned())
}

fn undo_writer(root: &FsRoot, undo_id: &str, records: &[Record]) -> Writer {
    let options = WorktreeOptions::default();
    let control = control(root);
    let stored = super::super::inspect(&control, &undo_id.parse().unwrap(), options).unwrap();
    let file = control.create_undo_journal().unwrap();
    let mut writer = Writer::new(file, options.limits.max_journal_bytes as u64).unwrap();
    writer
        .append(&Record::Header {
            transaction_id: ROLLBACK_ID.to_owned(),
            contract_hash: stored.checksum.to_string(),
            operation: undo_id.to_owned(),
            operation_count: 0,
            path_count: u32::try_from(stored.paths().len()).unwrap(),
        })
        .unwrap();
    for record in records {
        writer.append(record).unwrap();
    }
    writer
}

#[test]
fn empty_undo_journal_is_discarded_and_the_receipt_survives() {
    let (temp, undo_id) = retained_apply();
    let root = FsRoot::open(temp.path()).unwrap();
    drop(control(&root).create_undo_journal().unwrap());

    let worktree = Worktree::open(temp.path()).unwrap();
    let report = worktree.recover().unwrap();

    assert_eq!(report.action(), RecoveryAction::DiscardedStaging);
    assert!(!undo_journal_exists(&temp));
    assert!(receipt_exists(&temp, &undo_id));
    worktree.rollback_undo(&undo_id.parse().unwrap()).unwrap();
    assert_originals(temp.path());
}

#[test]
fn header_only_undo_journal_aborts_the_undo_without_mutation() {
    let (temp, undo_id) = retained_apply();
    let root = FsRoot::open(temp.path()).unwrap();
    drop(undo_writer(&root, &undo_id, &[]));

    let worktree = Worktree::open(temp.path()).unwrap();
    let report = worktree.recover().unwrap();

    assert_eq!(report.action(), RecoveryAction::DiscardedStaging);
    assert_eq!(report.transaction_id(), Some(ROLLBACK_ID));
    assert_committed(temp.path());
    assert!(receipt_exists(&temp, &undo_id));
    worktree.rollback_undo(&undo_id.parse().unwrap()).unwrap();
    assert_originals(temp.path());
}

#[test]
fn crash_after_a_durable_intent_completes_the_whole_rollback() {
    let (temp, undo_id) = retained_apply();
    let root = FsRoot::open(temp.path()).unwrap();
    drop(undo_writer(
        &root,
        &undo_id,
        &[Record::RollbackIntent { index: 2 }],
    ));

    let report = Worktree::open(temp.path()).unwrap().recover().unwrap();

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert_originals(temp.path());
    assert!(!receipt_exists(&temp, &undo_id));
    assert!(!undo_journal_exists(&temp));
}

#[test]
fn crash_after_a_restore_without_its_completion_record_converges() {
    let (temp, undo_id) = retained_apply();
    let root = FsRoot::open(temp.path()).unwrap();
    drop(undo_writer(
        &root,
        &undo_id,
        &[Record::RollbackIntent { index: 2 }],
    ));
    // The restore of the created path completed, but the crash struck before
    // the RolledBack record became durable.
    fs::remove_file(temp.path().join("made.rs")).unwrap();

    let report = Worktree::open(temp.path()).unwrap().recover().unwrap();

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert_originals(temp.path());
    assert!(!receipt_exists(&temp, &undo_id));
}

#[test]
fn crash_at_the_two_link_backup_intermediate_is_finished_exactly() {
    let (temp, undo_id) = retained_apply();
    let root = FsRoot::open(temp.path()).unwrap();
    drop(undo_writer(
        &root,
        &undo_id,
        &[
            Record::RollbackIntent { index: 2 },
            Record::RolledBack { index: 2 },
            Record::RollbackIntent { index: 1 },
        ],
    ));
    fs::remove_file(temp.path().join("made.rs")).unwrap();
    let access = root.open_target("gone.rs").unwrap();
    access.link_absent_from(&backup_name(&undo_id, 1)).unwrap();

    let report = Worktree::open(temp.path()).unwrap().recover().unwrap();

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert_originals(temp.path());
    assert!(!receipt_exists(&temp, &undo_id));
}

#[test]
fn crash_after_finished_rollback_consumes_receipt_and_journal() {
    let (temp, undo_id) = retained_apply();
    let root = FsRoot::open(temp.path()).unwrap();
    fs::remove_file(temp.path().join("made.rs")).unwrap();
    root.open_target("gone.rs")
        .unwrap()
        .install_absent_from(&backup_name(&undo_id, 1))
        .unwrap();
    root.open_target("edit.rs")
        .unwrap()
        .replace_from(&backup_name(&undo_id, 0))
        .unwrap();
    drop(undo_writer(
        &root,
        &undo_id,
        &[
            Record::RollbackIntent { index: 2 },
            Record::RolledBack { index: 2 },
            Record::RollbackIntent { index: 1 },
            Record::RolledBack { index: 1 },
            Record::RollbackIntent { index: 0 },
            Record::RolledBack { index: 0 },
            Record::Finished {
                outcome: crate::journal::FinishOutcome::RolledBack,
            },
        ],
    ));

    let report = Worktree::open(temp.path()).unwrap().recover().unwrap();

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert_originals(temp.path());
    assert!(!receipt_exists(&temp, &undo_id));
    assert!(!undo_journal_exists(&temp));
}

#[test]
fn foreign_change_under_an_active_undo_journal_fails_closed() {
    let (temp, undo_id) = retained_apply();
    let root = FsRoot::open(temp.path()).unwrap();
    drop(undo_writer(
        &root,
        &undo_id,
        &[Record::RollbackIntent { index: 0 }],
    ));
    fs::write(temp.path().join("edit.rs"), "external\n").unwrap();

    let error = Worktree::open(temp.path()).unwrap().recover().unwrap_err();

    assert!(error.recovery_required());
    assert!(undo_journal_exists(&temp));
    assert!(receipt_exists(&temp, &undo_id));
    assert_eq!(
        fs::read_to_string(temp.path().join("edit.rs")).unwrap(),
        "external\n"
    );
}

#[test]
fn simultaneous_operation_and_undo_journals_fail_closed_as_corruption() {
    let (temp, undo_id) = retained_apply();
    let root = FsRoot::open(temp.path()).unwrap();
    let control = control(&root);
    drop(control.create_undo_journal().unwrap());
    drop(control.create_operation_journal().unwrap());

    let worktree = Worktree::open(temp.path()).unwrap();
    let error = worktree.recover().unwrap_err();

    assert_eq!(error.code(), crate::WorktreeErrorCode::JournalCorrupt);
    assert!(error.recovery_required());
    assert!(receipt_exists(&temp, &undo_id));

    let (_, plan) = fixture();
    let error = worktree.apply_plan(&plan).unwrap_err();
    assert_eq!(error.code(), crate::WorktreeErrorCode::RecoveryRequired);
}
