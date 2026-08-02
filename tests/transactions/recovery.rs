use std::path::Path;

use weavatrix_worktree::{RecoveryAction, WorktreeErrorCode};

use crate::support::{assert_no_transaction_artifacts, fixture, plan_for_existing, worktree};

#[test]
fn explicit_abort_discards_stages_without_touching_targets() {
    let (temp, plan, originals) = fixture(5);
    let prepared = worktree(temp.path(), 4).prepare(&plan).unwrap();
    let report = prepared.abort().unwrap();

    assert_eq!(report.prepared_files(), 5);
    for (path, source) in originals {
        assert_eq!(
            std::fs::read_to_string(temp.path().join(path)).unwrap(),
            source
        );
    }
    assert_no_transaction_artifacts(temp.path());
}

#[test]
fn dropped_preparation_is_recovered_as_staging() {
    let (temp, plan, originals) = fixture(5);
    let engine = worktree(temp.path(), 4);
    let prepared = engine.prepare(&plan).unwrap();
    let transaction_id = prepared.transaction_id().to_owned();
    drop(prepared);

    let report = engine.recover().unwrap();

    assert_eq!(report.transaction_id(), Some(transaction_id.as_str()));
    assert_eq!(report.action(), RecoveryAction::DiscardedStaging);
    for (path, source) in originals {
        assert_eq!(
            std::fs::read_to_string(temp.path().join(path)).unwrap(),
            source
        );
    }
    assert_no_transaction_artifacts(temp.path());
}

#[test]
fn subprocess_exit_after_prepare_is_recoverable() {
    const ENV_ROOT: &str = "WEAVATRIX_WORKTREE_CRASH_ROOT";
    let (temp, _, originals) = fixture(5);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "recovery::crash_after_prepare_helper",
            "--nocapture",
        ])
        .env(ENV_ROOT, temp.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(91));

    let report = worktree(temp.path(), 4).recover().unwrap();

    assert_eq!(report.action(), RecoveryAction::DiscardedStaging);
    for (path, source) in originals {
        assert_eq!(
            std::fs::read_to_string(temp.path().join(path)).unwrap(),
            source
        );
    }
    assert_no_transaction_artifacts(temp.path());
}

#[test]
fn crash_after_prepare_helper() {
    let Ok(root) = std::env::var("WEAVATRIX_WORKTREE_CRASH_ROOT") else {
        return;
    };
    let root = Path::new(&root);
    let plan = plan_for_existing(root, 5);
    let prepared = worktree(root, 4).prepare(&plan).unwrap();
    assert_eq!(prepared.preview().files().len(), 5);
    std::process::exit(91);
}

#[test]
fn commit_conflict_rolls_back_already_replaced_files() {
    let (temp, plan, originals) = fixture(2);
    let engine = worktree(temp.path(), 2);
    let prepared = engine.prepare(&plan).unwrap();
    let second = temp.path().join("src/file-01.rs");
    std::fs::write(&second, "external\n").unwrap();

    let error = prepared.commit().unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::ConcurrentModification);
    let first_original = originals
        .iter()
        .find(|(path, _)| path.ends_with("file-00.rs"))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(temp.path().join(&first_original.0)).unwrap(),
        first_original.1
    );
    assert_eq!(std::fs::read_to_string(second).unwrap(), "external\n");
    assert_no_transaction_artifacts(temp.path());
}
