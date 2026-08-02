use crate::error::WorktreeErrorCode;

use super::*;

fn journal_exists(temp: &TempDir) -> bool {
    temp.path()
        .join(".weavatrix/worktree/active.jsonl")
        .exists()
}

#[test]
fn infers_a_rename_without_a_committed_record_and_rolls_it_back() -> TestResult {
    let (temp, root, plan, originals) = fixture(1)?;
    let options = WorktreeOptions::default();
    let mut transaction = prepare_transaction(&root, options, &plan)?;
    record_intent(&mut transaction, 0)?;
    rename_stage(&transaction, 0)?;
    drop(transaction);

    let report = recover_transaction(&root, options)?;

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert_originals(&temp, &originals)?;
    assert!(!journal_exists(&temp));
    Ok(())
}

#[test]
fn rolls_back_multiple_committed_files_and_cleans_every_artifact() -> TestResult {
    let (temp, root, plan, originals) = fixture(3)?;
    let options = WorktreeOptions::default().with_parallelism(3);
    let mut transaction = prepare_transaction(&root, options, &plan)?;
    record_commit(&mut transaction, 0)?;
    record_commit(&mut transaction, 1)?;
    drop(transaction);

    let report = recover_transaction(&root, options)?;

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert_eq!(report.files().len(), 3);
    // Restore consumes committed backups; cleanup removes the untouched pair.
    assert_eq!(report.artifacts_removed(), 2);
    assert_originals(&temp, &originals)?;
    Ok(())
}

#[test]
fn completes_an_already_rolled_back_unfinished_transaction_idempotently() -> TestResult {
    let (temp, root, plan, originals) = fixture(1)?;
    let options = WorktreeOptions::default();
    let mut transaction = prepare_transaction(&root, options, &plan)?;
    record_commit(&mut transaction, 0)?;
    record_rollback(&mut transaction, 0)?;
    drop(transaction);

    let report = recover_transaction(&root, options)?;

    assert_eq!(report.action(), RecoveryAction::RolledBack);
    assert_originals(&temp, &originals)?;
    assert!(!journal_exists(&temp));
    Ok(())
}

#[test]
fn verifies_finished_rollback_and_abort_before_cleanup() -> TestResult {
    for outcome in [FinishOutcome::RolledBack, FinishOutcome::Aborted] {
        let (temp, root, plan, originals) = fixture(1)?;
        let options = WorktreeOptions::default();
        let mut transaction = prepare_transaction(&root, options, &plan)?;
        if outcome == FinishOutcome::RolledBack {
            record_commit(&mut transaction, 0)?;
            record_rollback(&mut transaction, 0)?;
        }
        transaction
            .journal
            .append(&JournalRecord::Finished { outcome })?;
        drop(transaction);

        let report = recover_transaction(&root, options)?;
        assert_eq!(report.action(), RecoveryAction::DiscardedStaging);
        assert_originals(&temp, &originals)?;
        assert!(!journal_exists(&temp));
    }
    Ok(())
}

#[test]
fn discards_prepared_staging_when_commit_never_started() -> TestResult {
    let (temp, root, plan, originals) = fixture(2)?;
    let options = WorktreeOptions::default();
    let transaction = prepare_transaction(&root, options, &plan)?;
    drop(transaction);

    let report = recover_transaction(&root, options)?;

    assert_eq!(report.action(), RecoveryAction::DiscardedStaging);
    assert_eq!(report.artifacts_removed(), 4);
    assert_originals(&temp, &originals)?;
    Ok(())
}

#[test]
fn refuses_rollback_when_backup_no_longer_matches_journal_evidence() -> TestResult {
    let (temp, root, plan, _) = fixture(1)?;
    let options = WorktreeOptions::default();
    let mut transaction = prepare_transaction(&root, options, &plan)?;
    record_commit(&mut transaction, 0)?;
    let backup_name = transaction.files[0].backup_name.clone();
    std::fs::write(temp.path().join(backup_name), b"tampered backup")?;
    drop(transaction);

    let error = recovery_error(&root, options)?;

    assert_eq!(error.code(), WorktreeErrorCode::RecoveryRequired);
    assert!(error.recovery_required());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("file-0.rs"))?,
        "old_0!\n"
    );
    assert!(journal_exists(&temp));
    Ok(())
}

#[test]
fn finished_commit_rejects_an_external_target_change() -> TestResult {
    let (temp, root, plan, _) = fixture(1)?;
    let options = WorktreeOptions::default();
    let mut transaction = prepare_transaction(&root, options, &plan)?;
    record_commit(&mut transaction, 0)?;
    transaction.journal.append(&JournalRecord::Finished {
        outcome: FinishOutcome::Committed,
    })?;
    std::fs::write(temp.path().join("file-0.rs"), "external after finish\n")?;
    drop(transaction);

    let error = recovery_error(&root, options)?;

    assert_eq!(error.code(), WorktreeErrorCode::RecoveryRequired);
    assert!(error.recovery_required());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("file-0.rs"))?,
        "external after finish\n"
    );
    assert!(journal_exists(&temp));
    Ok(())
}

#[test]
fn rejects_a_committed_record_without_a_matching_intent() -> TestResult {
    let (temp, root, plan, originals) = fixture(1)?;
    let options = WorktreeOptions::default();
    let mut transaction = prepare_transaction(&root, options, &plan)?;
    transaction
        .journal
        .append(&JournalRecord::Committed { index: 0 })?;
    drop(transaction);

    let error = recovery_error(&root, options)?;

    assert_eq!(error.code(), WorktreeErrorCode::JournalCorrupt);
    assert!(error.recovery_required());
    assert_originals(&temp, &originals)?;
    assert!(journal_exists(&temp));
    Ok(())
}
