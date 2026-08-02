use weavatrix_edit::{EditPlan, FileEdit, Position, Provenance, TextEdit};
use weavatrix_worktree::{Sha256Hash, WorktreeErrorCode};

use crate::support::{assert_no_transaction_artifacts, find_artifact, fixture, worktree};

#[test]
fn stale_plan_changes_no_target() {
    let (temp, mut plan, originals) = fixture(5);
    plan.files[2].sha256 = "0".repeat(64);

    let error = worktree(temp.path(), 4).apply(&plan).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::SourceHashMismatch);
    for (path, source) in originals {
        assert_eq!(
            std::fs::read_to_string(temp.path().join(path)).unwrap(),
            source
        );
    }
    assert_no_transaction_artifacts(temp.path());
}

#[test]
fn a_second_preparation_observes_the_root_lock() {
    let (temp, plan, _) = fixture(1);
    let engine = worktree(temp.path(), 1);
    let prepared = engine.prepare(&plan).unwrap();

    let error = engine.prepare(&plan).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::RootBusy);
    prepared.abort().unwrap();
}

#[test]
fn tampered_backup_stops_commit_before_any_target_change() {
    let (temp, plan, originals) = fixture(2);
    let prepared = worktree(temp.path(), 2).prepare(&plan).unwrap();
    std::fs::write(find_artifact(temp.path(), ".backup"), "tampered\n").unwrap();

    let error = prepared.commit().unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::CommitFailed);
    for (path, source) in originals {
        assert_eq!(
            std::fs::read_to_string(temp.path().join(path)).unwrap(),
            source
        );
    }
    assert_no_transaction_artifacts(temp.path());
}

#[test]
fn tampered_stage_stops_commit_before_any_target_change() {
    let (temp, plan, originals) = fixture(2);
    let prepared = worktree(temp.path(), 2).prepare(&plan).unwrap();
    std::fs::write(find_artifact(temp.path(), ".stage"), "tampered\n").unwrap();

    let error = prepared.commit().unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::CommitFailed);
    for (path, source) in originals {
        assert_eq!(
            std::fs::read_to_string(temp.path().join(path)).unwrap(),
            source
        );
    }
    assert_no_transaction_artifacts(temp.path());
}

#[test]
fn cleanup_refuses_a_hardlink_substituted_for_an_artifact() {
    let (temp, plan, originals) = fixture(1);
    let prepared = worktree(temp.path(), 1).prepare(&plan).unwrap();
    let stage = find_artifact(temp.path(), ".stage");
    let unrelated = temp.path().join("unrelated.txt");
    std::fs::write(&unrelated, "do not remove\n").unwrap();
    std::fs::remove_file(&stage).unwrap();
    std::fs::hard_link(&unrelated, &stage).unwrap();

    let error = prepared.commit().unwrap_err();

    assert!(error.recovery_required());
    assert!(stage.exists());
    assert_eq!(
        std::fs::read_to_string(unrelated).unwrap(),
        "do not remove\n"
    );
    for (path, source) in originals {
        assert_eq!(
            std::fs::read_to_string(temp.path().join(path)).unwrap(),
            source
        );
    }
}

#[test]
fn read_only_target_fails_before_staging() {
    let (temp, plan, _) = fixture(1);
    let path = temp.path().join("src/file-00.rs");
    let original_permissions = std::fs::metadata(&path).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).unwrap();

    let error = worktree(temp.path(), 1).apply(&plan).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::ReadOnlyFile);
    std::fs::set_permissions(path, original_permissions).unwrap();
}

#[test]
fn non_utf8_target_fails_before_staging() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = [0xff, 0xfe, 0xfd];
    std::fs::write(temp.path().join("bad.rs"), bytes).unwrap();
    let plan = EditPlan::new(
        "encoding_check",
        vec![FileEdit::new(
            "bad.rs",
            Sha256Hash::compute(&bytes).to_string(),
            vec![TextEdit::insert(
                Position::new(1, 0),
                "x",
                Provenance::EXACT_LSP,
            )],
        )],
    );

    let error = worktree(temp.path(), 1).apply(&plan).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::NonUtf8Source);
    assert_eq!(std::fs::read(temp.path().join("bad.rs")).unwrap(), bytes);
}

#[test]
fn hard_link_target_is_rejected() {
    let (temp, plan, _) = fixture(1);
    std::fs::hard_link(
        temp.path().join("src/file-00.rs"),
        temp.path().join("alias.rs"),
    )
    .unwrap();

    let error = worktree(temp.path(), 1).apply(&plan).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::HardlinkNotAllowed);
}

#[test]
fn directory_target_is_rejected_as_non_regular() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("directory.rs")).unwrap();
    let plan = EditPlan::new(
        "special_file",
        vec![FileEdit::new(
            "directory.rs",
            Sha256Hash::compute(b"").to_string(),
            vec![TextEdit::insert(
                Position::new(1, 0),
                "x",
                Provenance::EXACT_LSP,
            )],
        )],
    );

    let error = worktree(temp.path(), 1).apply(&plan).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::NotRegularFile);
}

#[test]
fn reserved_control_path_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join(".weavatrix/worktree")).unwrap();
    std::fs::write(temp.path().join(".weavatrix/worktree/user.rs"), "old\n").unwrap();
    let plan = EditPlan::new(
        "reserved_path",
        vec![FileEdit::new(
            ".weavatrix/worktree/user.rs",
            Sha256Hash::compute(b"old\n").to_string(),
            vec![TextEdit::insert(
                Position::new(1, 3),
                "!",
                Provenance::EXACT_LSP,
            )],
        )],
    );

    let error = worktree(temp.path(), 1).dry_run(&plan).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::ReservedPath);
}

#[cfg(unix)]
#[test]
fn parent_symlink_is_rejected() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("real")).unwrap();
    std::fs::write(temp.path().join("real/file.rs"), "old\n").unwrap();
    symlink("real", temp.path().join("link")).unwrap();
    let plan = EditPlan::new(
        "link_attack",
        vec![FileEdit::new(
            "link/file.rs",
            Sha256Hash::compute(b"old\n").to_string(),
            vec![TextEdit::insert(
                Position::new(1, 3),
                "!",
                Provenance::EXACT_LSP,
            )],
        )],
    );

    let error = worktree(temp.path(), 1).apply(&plan).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::SymlinkNotAllowed);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("real/file.rs")).unwrap(),
        "old\n"
    );
}

#[cfg(windows)]
#[test]
fn parent_junction_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real");
    let link = temp.path().join("link");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("file.rs"), "old\n").unwrap();
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&real)
        .output()
        .unwrap();
    assert!(output.status.success(), "mklink failed: {output:?}");
    let plan = EditPlan::new(
        "junction_attack",
        vec![FileEdit::new(
            "link/file.rs",
            Sha256Hash::compute(b"old\n").to_string(),
            vec![TextEdit::insert(
                Position::new(1, 3),
                "!",
                Provenance::EXACT_LSP,
            )],
        )],
    );

    let error = worktree(temp.path(), 1).apply(&plan).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::SymlinkNotAllowed);
    assert_eq!(
        std::fs::read_to_string(real.join("file.rs")).unwrap(),
        "old\n"
    );
}
