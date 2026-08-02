use crate::support::{assert_no_transaction_artifacts, fixture, worktree};

#[test]
fn dry_run_is_side_effect_free_and_worker_deterministic() {
    let (temp, plan, _) = fixture(10);
    let serial = worktree(temp.path(), 1).dry_run(&plan).unwrap();
    let parallel = worktree(temp.path(), 8).dry_run(&plan).unwrap();

    assert_eq!(serial, parallel);
    assert_eq!(serial.files().len(), 10);
    assert!(
        serial
            .files()
            .windows(2)
            .all(|pair| pair[0].path() < pair[1].path())
    );
    assert!(!temp.path().join(".weavatrix").exists());
}

#[test]
fn applies_five_files_and_cleans_every_artifact() {
    let (temp, plan, originals) = fixture(5);
    let report = worktree(temp.path(), 4).apply(&plan).unwrap();

    assert_eq!(report.files().len(), 5);
    for (path, source) in originals {
        let expected = source.replace('\n', "!\n");
        assert_eq!(
            std::fs::read_to_string(temp.path().join(path)).unwrap(),
            expected
        );
    }
    assert_no_transaction_artifacts(temp.path());
}
