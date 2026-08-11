use std::error::Error;

use tempfile::TempDir;
use weavatrix_refactor_plan::{EditPlan, FileEdit, Position, Provenance, TextEdit};

use crate::{
    error::WorktreeError,
    filesystem::FsRoot,
    hash::Sha256Hash,
    journal::{FinishOutcome, JournalRecord},
    options::WorktreeOptions,
    report::RecoveryAction,
};

use super::{prepare_transaction, recover_transaction};

type TestError = Box<dyn Error>;
type TestResult = Result<(), TestError>;

mod faults;

fn fixture(count: usize) -> Result<(TempDir, FsRoot, EditPlan, Vec<String>), TestError> {
    let temp = tempfile::tempdir()?;
    let mut files = Vec::with_capacity(count);
    let mut originals = Vec::with_capacity(count);
    for index in 0..count {
        let path = format!("file-{index}.rs");
        let source = format!("old_{index}\n");
        std::fs::write(temp.path().join(&path), &source)?;
        files.push(FileEdit::new(
            path,
            Sha256Hash::compute(source.as_bytes()).to_string(),
            vec![TextEdit::insert(
                Position::new(1, u32::try_from(source.len() - 1)?),
                "!",
                Provenance::EXACT_LSP,
            )],
        ));
        originals.push(source);
    }
    let root = FsRoot::open(temp.path())?;
    Ok((temp, root, EditPlan::new("recovery_test", files), originals))
}

fn record_commit(transaction: &mut super::PreparedTransaction, index: usize) -> TestResult {
    record_intent(transaction, index)?;
    rename_stage(transaction, index)?;
    let file = &transaction.files[index];
    transaction.journal.append(&JournalRecord::Committed {
        index: file.stable_index,
    })?;
    Ok(())
}

fn record_intent(transaction: &mut super::PreparedTransaction, index: usize) -> TestResult {
    let stable_index = transaction.files[index].stable_index;
    transaction.journal.append(&JournalRecord::CommitIntent {
        index: stable_index,
    })?;
    Ok(())
}

fn rename_stage(transaction: &super::PreparedTransaction, index: usize) -> TestResult {
    let file = &transaction.files[index];
    file.access.rename_from(&file.stage_name)?;
    file.access.sync_parent()?;
    Ok(())
}

fn record_rollback(transaction: &mut super::PreparedTransaction, index: usize) -> TestResult {
    let file = &transaction.files[index];
    transaction.journal.append(&JournalRecord::RollbackIntent {
        index: file.stable_index,
    })?;
    file.access.rename_from(&file.backup_name)?;
    file.access.sync_parent()?;
    transaction.journal.append(&JournalRecord::RolledBack {
        index: file.stable_index,
    })?;
    Ok(())
}

fn assert_originals(temp: &TempDir, originals: &[String]) -> TestResult {
    for (index, original) in originals.iter().enumerate() {
        assert_eq!(
            std::fs::read_to_string(temp.path().join(format!("file-{index}.rs")))?,
            *original
        );
    }
    Ok(())
}

fn recovery_error(root: &FsRoot, options: WorktreeOptions) -> Result<WorktreeError, TestError> {
    let Err(error) = recover_transaction(root, options) else {
        return Err(std::io::Error::other("recovery unexpectedly succeeded").into());
    };
    Ok(error)
}

#[test]
fn recovers_a_crash_after_one_committed_target() -> TestResult {
    let (temp, root, plan, originals) = fixture(2)?;
    let options = WorktreeOptions::default().with_parallelism(2);
    let mut transaction = prepare_transaction(&root, options, &plan)?;
    record_commit(&mut transaction, 0)?;
    drop(transaction);

    let report = recover_transaction(&root, options)?;

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert_originals(&temp, &originals)?;
    Ok(())
}

#[test]
fn resolves_an_intent_without_a_rename_as_not_committed() -> TestResult {
    let (_temp, root, plan, _) = fixture(1)?;
    let options = WorktreeOptions::default();
    let mut transaction = prepare_transaction(&root, options, &plan)?;
    transaction
        .journal
        .append(&JournalRecord::CommitIntent { index: 0 })?;
    drop(transaction);

    let report = recover_transaction(&root, options)?;

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    Ok(())
}

#[test]
fn refuses_to_overwrite_an_external_recovery_change() -> TestResult {
    let (temp, root, plan, _) = fixture(1)?;
    let options = WorktreeOptions::default();
    let mut transaction = prepare_transaction(&root, options, &plan)?;
    transaction
        .journal
        .append(&JournalRecord::CommitIntent { index: 0 })?;
    std::fs::write(temp.path().join("file-0.rs"), "external\n")?;
    drop(transaction);

    let error = recovery_error(&root, options)?;

    assert!(error.recovery_required());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("file-0.rs"))?,
        "external\n"
    );
    assert!(
        temp.path()
            .join(".weavatrix/worktree/active.jsonl")
            .exists()
    );
    Ok(())
}

#[test]
fn finished_commit_is_verified_and_only_cleaned() -> TestResult {
    let (temp, root, plan, _) = fixture(2)?;
    let options = WorktreeOptions::default();
    let mut transaction = prepare_transaction(&root, options, &plan)?;
    record_commit(&mut transaction, 0)?;
    record_commit(&mut transaction, 1)?;
    transaction.journal.append(&JournalRecord::Finished {
        outcome: FinishOutcome::Committed,
    })?;
    drop(transaction);

    let report = recover_transaction(&root, options)?;

    assert_eq!(report.action(), RecoveryAction::FinishedCommitCleanup);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("file-0.rs"))?,
        "old_0!\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("file-1.rs"))?,
        "old_1!\n"
    );
    Ok(())
}

#[test]
fn no_journal_reports_no_pending_transaction() -> TestResult {
    let (_temp, root, _plan, _) = fixture(1)?;
    let report = recover_transaction(&root, WorktreeOptions::default())?;
    assert_eq!(report.action(), RecoveryAction::NoPendingTransaction);
    Ok(())
}

#[test]
#[allow(clippy::used_underscore_binding)]
fn dropping_a_prepared_transaction_unlocks_even_with_a_live_duplicate() -> TestResult {
    let (_temp, root, plan, _) = fixture(1)?;
    let options = WorktreeOptions::default();
    let transaction = prepare_transaction(&root, options, &plan)?;
    let _duplicate = transaction._lock.try_clone()?;
    drop(transaction);

    let report = recover_transaction(&root, options)?;

    assert_eq!(report.action(), RecoveryAction::DiscardedStaging);
    Ok(())
}
