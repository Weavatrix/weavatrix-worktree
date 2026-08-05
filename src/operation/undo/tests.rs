use std::fs;
use std::path::Path;

use tempfile::{TempDir, tempdir};
use weavatrix_refactor_plan::{Position, Provenance, TextEdit, TextRange};

use crate::{
    CreateFile, DeleteFile, FileEdit, RecoveryAction, Sha256Hash, UndoRetention, Worktree,
    WorktreeOperation, WorktreeOptions, WorktreePlan,
    filesystem::{ControlDir, FsRoot},
    operation::journal::{Record, Writer},
};

mod faults;

const EDIT_BEFORE: &str = "old body\n";
const EDIT_AFTER: &str = "new body\n";
const GONE: &str = "delete me\n";
const MADE: &str = "created\n";

fn hash(text: &str) -> String {
    Sha256Hash::compute(text.as_bytes()).to_string()
}

/// One plan covering all three restore shapes: replace, remove, reinstall.
fn fixture() -> (TempDir, WorktreePlan) {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("edit.rs"), EDIT_BEFORE).unwrap();
    fs::write(temp.path().join("gone.rs"), GONE).unwrap();
    let modify = FileEdit::new(
        "edit.rs",
        hash(EDIT_BEFORE),
        vec![TextEdit::replace(
            TextRange::new(Position::new(1, 0), Position::new(1, 3)),
            "old",
            "new",
            Provenance::EXACT_LSP,
        )],
    );
    let plan = WorktreePlan::new(
        "retained_fixture",
        vec![
            WorktreeOperation::Modify(modify),
            WorktreeOperation::Create(CreateFile::new("made.rs", MADE)),
            WorktreeOperation::Delete(DeleteFile::new("gone.rs", hash(GONE))),
        ],
    );
    (temp, plan)
}

fn assert_originals(root: &Path) {
    assert_eq!(
        fs::read_to_string(root.join("edit.rs")).unwrap(),
        EDIT_BEFORE
    );
    assert_eq!(fs::read_to_string(root.join("gone.rs")).unwrap(), GONE);
    assert!(!root.join("made.rs").exists());
}

fn assert_committed(root: &Path) {
    assert_eq!(
        fs::read_to_string(root.join("edit.rs")).unwrap(),
        EDIT_AFTER
    );
    assert_eq!(fs::read_to_string(root.join("made.rs")).unwrap(), MADE);
    assert!(!root.join("gone.rs").exists());
}

fn control(root: &FsRoot) -> ControlDir {
    root.open_control(false).unwrap().unwrap()
}

fn undo_journal_exists(temp: &TempDir) -> bool {
    temp.path()
        .join(".weavatrix/worktree/active-undo.jsonl")
        .exists()
}

fn receipt_exists(temp: &TempDir, id: &str) -> bool {
    temp.path()
        .join(format!(".weavatrix/worktree/undo-{id}.json"))
        .exists()
}

fn backup_name(id: &str, index: u32) -> String {
    format!(".weavatrix-{id}-{index:04}.backup")
}

#[test]
fn crash_before_any_commit_intent_discards_the_transitional_receipt() {
    let (temp, plan) = fixture();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = WorktreeOptions::default();
    let prepared = crate::operation::prepare_operation_plan(&root, options, &plan).unwrap();
    let transaction_id = prepared.transaction_id().to_owned();
    super::prepare_retention(&prepared, UndoRetention::default()).unwrap();
    assert!(receipt_exists(&temp, &transaction_id));
    drop(prepared);

    let report = crate::operation::recover_operation_transaction(&root, options)
        .unwrap()
        .unwrap();

    assert_eq!(report.action(), RecoveryAction::DiscardedStaging);
    assert!(!receipt_exists(&temp, &transaction_id));
    assert_originals(temp.path());
}

#[test]
fn crash_after_partial_mutation_rolls_back_and_removes_the_receipt() {
    let (temp, plan) = fixture();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = WorktreeOptions::default();
    let mut prepared = crate::operation::prepare_operation_plan(&root, options, &plan).unwrap();
    let transaction_id = prepared.transaction_id().to_owned();
    super::prepare_retention(&prepared, UndoRetention::default()).unwrap();
    mutate_path(&mut prepared, 0, options);
    drop(prepared);

    let report = crate::operation::recover_operation_transaction(&root, options)
        .unwrap()
        .unwrap();

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert!(!receipt_exists(&temp, &transaction_id));
    assert_originals(temp.path());
}

#[test]
fn crash_after_finished_commit_keeps_receipt_and_backups_for_rollback() {
    let (temp, plan) = fixture();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = WorktreeOptions::default();
    let mut prepared = crate::operation::prepare_operation_plan(&root, options, &plan).unwrap();
    let transaction_id = prepared.transaction_id().to_owned();
    super::prepare_retention(&prepared, UndoRetention::default()).unwrap();
    for index in 0..prepared.paths.len() {
        mutate_path(&mut prepared, index, options);
    }
    prepared
        .journal
        .append(&Record::Finished {
            outcome: crate::journal::FinishOutcome::Committed,
        })
        .unwrap();
    drop(prepared);

    let report = crate::operation::recover_operation_transaction(&root, options)
        .unwrap()
        .unwrap();

    assert_eq!(report.action(), RecoveryAction::FinishedCommitCleanup);
    assert!(receipt_exists(&temp, &transaction_id));
    assert_committed(temp.path());

    let worktree = Worktree::open(temp.path()).unwrap();
    let receipts = worktree.undo_receipts().unwrap();
    assert_eq!(receipts.len(), 1);
    worktree.rollback_undo(receipts[0].id()).unwrap();
    assert_originals(temp.path());
}

fn mutate_path(
    prepared: &mut crate::operation::PreparedWorktreeTransaction,
    index: usize,
    options: WorktreeOptions,
) {
    let path = &prepared.paths[index];
    let stable_index = path.stable_index;
    prepared
        .journal
        .append(&Record::CommitIntent {
            index: stable_index,
        })
        .unwrap();
    let path = &prepared.paths[index];
    match (path.before, path.after) {
        (
            crate::filesystem::SlotEvidence::Present(_),
            crate::filesystem::SlotEvidence::Present(_),
        ) => {
            path.access
                .replace_from(path.stage_name.as_deref().unwrap())
                .unwrap();
        }
        (crate::filesystem::SlotEvidence::Absent, _) => {
            path.access
                .install_absent_from(path.stage_name.as_deref().unwrap())
                .unwrap();
        }
        (
            crate::filesystem::SlotEvidence::Present(before),
            crate::filesystem::SlotEvidence::Absent,
        ) => {
            path.access
                .remove_exact(before, options.limits.max_source_bytes_per_file)
                .unwrap();
        }
    }
    path.access.sync_parent().unwrap();
    prepared
        .journal
        .append(&Record::Committed {
            index: stable_index,
        })
        .unwrap();
}
