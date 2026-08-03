use std::fs;
use std::path::Path;

use tempfile::{TempDir, tempdir};
use weavatrix_refactor_plan::{FileEdit, Position, Provenance, TextEdit, TextRange};
use weavatrix_worktree::{
    CreateFile, DeleteFile, RenameFile, Sha256Hash, UndoId, UndoRetention, Worktree,
    WorktreeErrorCode, WorktreeOperation, WorktreePlan,
};

use super::support::assert_no_transaction_artifacts;

const EDIT_BEFORE: &str = "fn old() {}\n";
const EDIT_AFTER: &str = "fn new() {}\n";
const GONE: &str = "obsolete\n";
const MOVED: &str = "moved body\n";
const MADE: &str = "created\n";

fn hash(text: &str) -> String {
    Sha256Hash::compute(text.as_bytes()).to_string()
}

fn fixture() -> (TempDir, WorktreePlan) {
    let temp = tempdir().unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/edit.rs"), EDIT_BEFORE).unwrap();
    fs::write(temp.path().join("src/gone.rs"), GONE).unwrap();
    fs::write(temp.path().join("src/from.rs"), MOVED).unwrap();
    let modify = FileEdit::new(
        "src/edit.rs",
        hash(EDIT_BEFORE),
        vec![TextEdit::replace(
            TextRange::new(Position::new(1, 3), Position::new(1, 6)),
            "old",
            "new",
            Provenance::EXACT_LSP,
        )],
    );
    let plan = WorktreePlan::new(
        "retained_mixed",
        vec![
            WorktreeOperation::Modify(modify),
            WorktreeOperation::Create(CreateFile::new("src/made.rs", MADE)),
            WorktreeOperation::Delete(DeleteFile::new("src/gone.rs", hash(GONE))),
            WorktreeOperation::Rename(RenameFile::new("src/from.rs", "src/to.rs", hash(MOVED))),
        ],
    );
    (temp, plan)
}

fn assert_originals(root: &Path) {
    assert_eq!(
        fs::read_to_string(root.join("src/edit.rs")).unwrap(),
        EDIT_BEFORE
    );
    assert_eq!(fs::read_to_string(root.join("src/gone.rs")).unwrap(), GONE);
    assert_eq!(fs::read_to_string(root.join("src/from.rs")).unwrap(), MOVED);
    assert!(!root.join("src/made.rs").exists());
    assert!(!root.join("src/to.rs").exists());
}

fn assert_committed(root: &Path) {
    assert_eq!(
        fs::read_to_string(root.join("src/edit.rs")).unwrap(),
        EDIT_AFTER
    );
    assert_eq!(fs::read_to_string(root.join("src/made.rs")).unwrap(), MADE);
    assert_eq!(fs::read_to_string(root.join("src/to.rs")).unwrap(), MOVED);
    assert!(!root.join("src/gone.rs").exists());
    assert!(!root.join("src/from.rs").exists());
}

#[test]
fn retained_apply_then_rollback_restores_exact_bytes_and_absence() {
    let (temp, plan) = fixture();
    let worktree = Worktree::open(temp.path()).unwrap();

    let report = worktree
        .apply_plan_retained(&plan, UndoRetention::default())
        .unwrap();
    assert_committed(temp.path());
    assert_eq!(report.apply().files().len(), 4);
    let receipt = report.receipt();
    assert_eq!(receipt.touched_paths(), 5);
    assert!(receipt.retained_bytes() > 0);
    assert_ne!(receipt.before_fingerprint(), receipt.after_fingerprint());

    let listed = worktree.undo_receipts().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(&listed[0], receipt);
    let usage = worktree.undo_usage().unwrap();
    assert_eq!(usage.receipts(), 1);
    assert!(usage.bytes() > receipt.retained_bytes());

    let rollback = worktree.rollback_undo(report.undo_id()).unwrap();
    assert_eq!(rollback.undo_id(), report.undo_id());
    assert_eq!(rollback.restored_paths(), 5);
    assert!(rollback.artifacts_removed() > 0);
    assert_ne!(
        rollback.rollback_transaction_id(),
        rollback.undo_id().as_str()
    );
    assert!(rollback.rollback_transaction_id().parse::<UndoId>().is_ok());

    assert_originals(temp.path());
    assert!(worktree.undo_receipts().unwrap().is_empty());
    assert_eq!(worktree.undo_usage().unwrap().receipts(), 0);
    assert_eq!(worktree.undo_usage().unwrap().bytes(), 0);
    assert_no_transaction_artifacts(temp.path());
}

#[test]
fn receipts_persist_across_worktree_instances() {
    let (temp, plan) = fixture();
    let undo_id = Worktree::open(temp.path())
        .unwrap()
        .apply_plan_retained(&plan, UndoRetention::default())
        .unwrap()
        .undo_id()
        .clone();

    let reopened = Worktree::open(temp.path()).unwrap();
    let listed = reopened.undo_receipts().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id(), &undo_id);
    reopened.rollback_undo(&undo_id).unwrap();
    assert_originals(temp.path());
}

#[test]
fn rollback_conflict_leaves_the_tree_and_receipt_untouched() {
    let (temp, plan) = fixture();
    let worktree = Worktree::open(temp.path()).unwrap();
    let report = worktree
        .apply_plan_retained(&plan, UndoRetention::default())
        .unwrap();
    fs::write(temp.path().join("src/edit.rs"), "external change\n").unwrap();

    let error = worktree.rollback_undo(report.undo_id()).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::UndoConflict);
    assert!(!error.recovery_required());
    assert_eq!(error.path(), Some("src/edit.rs"));
    assert_eq!(
        fs::read_to_string(temp.path().join("src/edit.rs")).unwrap(),
        "external change\n"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("src/to.rs")).unwrap(),
        MOVED
    );
    assert_eq!(worktree.undo_receipts().unwrap().len(), 1);
    assert!(
        !temp
            .path()
            .join(".weavatrix/worktree/active-undo.jsonl")
            .exists()
    );
}

#[test]
fn discard_verifies_artifact_evidence_before_removal() {
    let (temp, plan) = fixture();
    let worktree = Worktree::open(temp.path()).unwrap();
    let report = worktree
        .apply_plan_retained(&plan, UndoRetention::default())
        .unwrap();

    let removed = worktree.discard_undo(report.undo_id()).unwrap();

    // Modify, delete, and rename sources each retained one backup artifact.
    assert_eq!(removed, 3);
    assert_committed(temp.path());
    assert!(worktree.undo_receipts().unwrap().is_empty());
    assert_no_transaction_artifacts(temp.path());
    assert_eq!(
        worktree.discard_undo(report.undo_id()).unwrap_err().code(),
        WorktreeErrorCode::UndoNotFound
    );
}

#[test]
fn discard_rejects_a_tampered_retained_artifact() {
    let (temp, plan) = fixture();
    let worktree = Worktree::open(temp.path()).unwrap();
    let report = worktree
        .apply_plan_retained(&plan, UndoRetention::default())
        .unwrap();
    let backup = temp.path().join("src").join(format!(
        ".weavatrix-{}-0000.backup",
        report.undo_id().as_str()
    ));
    fs::write(&backup, "tampered").unwrap();

    let error = worktree.discard_undo(report.undo_id()).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::UndoCorrupt);
    assert_eq!(worktree.undo_receipts().unwrap().len(), 1);
}

#[test]
fn exhausted_retention_fails_closed_before_touching_targets() {
    let (temp, plan) = fixture();
    let worktree = Worktree::open(temp.path()).unwrap();
    let retention = UndoRetention::new(1, UndoRetention::default().max_bytes());
    worktree.apply_plan_retained(&plan, retention).unwrap();

    let second = WorktreePlan::new(
        "second",
        vec![WorktreeOperation::Create(CreateFile::new(
            "src/other.rs",
            "other\n",
        ))],
    );
    let error = worktree
        .apply_plan_retained(&second, retention)
        .unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::UndoStoreFull);
    assert!(!temp.path().join("src/other.rs").exists());
    assert_committed(temp.path());
    assert_eq!(worktree.undo_receipts().unwrap().len(), 1);
}

#[test]
fn missing_and_consumed_receipts_report_not_found() {
    let (temp, plan) = fixture();
    let worktree = Worktree::open(temp.path()).unwrap();
    let unknown: UndoId = "0".repeat(32).parse().unwrap();
    assert_eq!(
        worktree.rollback_undo(&unknown).unwrap_err().code(),
        WorktreeErrorCode::UndoNotFound
    );

    let report = worktree
        .apply_plan_retained(&plan, UndoRetention::default())
        .unwrap();
    worktree.rollback_undo(report.undo_id()).unwrap();
    assert_eq!(
        worktree.rollback_undo(report.undo_id()).unwrap_err().code(),
        WorktreeErrorCode::UndoNotFound
    );
}

#[test]
fn usage_accounts_every_receipt_and_returns_to_zero() {
    let (temp, plan) = fixture();
    let worktree = Worktree::open(temp.path()).unwrap();
    let first = worktree
        .apply_plan_retained(&plan, UndoRetention::default())
        .unwrap();
    // A receipt binds complete slot evidence, so the follow-up plan must not
    // touch first-plan paths: any later transaction on the same path
    // deliberately invalidates the older receipt's exact CAS state.
    let second_plan = WorktreePlan::new(
        "second",
        vec![WorktreeOperation::Create(CreateFile::new(
            "src/other.rs",
            "other\n",
        ))],
    );
    let second = worktree
        .apply_plan_retained(&second_plan, UndoRetention::default())
        .unwrap();

    let usage = worktree.undo_usage().unwrap();
    assert_eq!(usage.receipts(), 2);
    let retained = first.receipt().retained_bytes() + second.receipt().retained_bytes();
    assert!(usage.bytes() > retained);

    worktree.rollback_undo(second.undo_id()).unwrap();
    worktree.rollback_undo(first.undo_id()).unwrap();
    assert_originals(temp.path());
    let usage = worktree.undo_usage().unwrap();
    assert_eq!(usage.receipts(), 0);
    assert_eq!(usage.bytes(), 0);
    assert_no_transaction_artifacts(temp.path());
}
