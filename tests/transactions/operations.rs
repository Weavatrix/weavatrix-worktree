use std::fs;

use tempfile::tempdir;
use weavatrix_refactor_plan::{FileEdit, Position, Provenance, TextEdit};
use weavatrix_worktree::{
    CreateFile, DeleteFile, RecoveryAction, RenameFile, Sha256Hash, TransactionPhase, Worktree,
    WorktreeErrorCode, WorktreeLimits, WorktreeOperation, WorktreeOptions, WorktreePlan,
};

use super::support::assert_no_transaction_artifacts;

fn hash(text: &str) -> String {
    Sha256Hash::compute(text.as_bytes()).to_string()
}

#[test]
fn applies_create_delete_rename_and_modify_as_one_transaction() {
    let root = tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/modify.rs"), "old\n").unwrap();
    fs::write(root.path().join("src/delete.rs"), "delete me\n").unwrap();
    fs::write(root.path().join("src/from.rs"), "moved\n").unwrap();

    let modify = FileEdit::new(
        "src/modify.rs",
        hash("old\n"),
        vec![TextEdit::replace(
            weavatrix_refactor_plan::TextRange::new(Position::new(1, 0), Position::new(1, 3)),
            "old",
            "new",
            Provenance::EXACT_LSP,
        )],
    );
    let plan = WorktreePlan::new(
        "mixed",
        vec![
            WorktreeOperation::Modify(modify),
            WorktreeOperation::Create(CreateFile::new("src/create.rs", "created\n")),
            WorktreeOperation::Delete(DeleteFile::new("src/delete.rs", hash("delete me\n"))),
            WorktreeOperation::Rename(RenameFile::new("src/from.rs", "src/to.rs", hash("moved\n"))),
        ],
    );

    let report = Worktree::open(root.path())
        .unwrap()
        .apply_plan(&plan)
        .unwrap();

    assert_eq!(report.files().len(), 4);
    assert_eq!(
        fs::read_to_string(root.path().join("src/modify.rs")).unwrap(),
        "new\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("src/create.rs")).unwrap(),
        "created\n"
    );
    assert!(!root.path().join("src/delete.rs").exists());
    assert!(!root.path().join("src/from.rs").exists());
    assert_eq!(
        fs::read_to_string(root.path().join("src/to.rs")).unwrap(),
        "moved\n"
    );
    assert_no_transaction_artifacts(root.path());
}

#[test]
fn rename_can_apply_exact_edits_to_the_moved_source() {
    let root = tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/old.rs"), "fn old() {}\n").unwrap();
    let rename =
        RenameFile::new("src/old.rs", "src/new.rs", hash("fn old() {}\n")).with_edits(vec![
            TextEdit::replace(
                weavatrix_refactor_plan::TextRange::new(Position::new(1, 3), Position::new(1, 6)),
                "old",
                "new",
                Provenance::EXACT_LSP,
            ),
        ]);
    let plan = WorktreePlan::new("move_symbol_file", vec![WorktreeOperation::Rename(rename)]);

    Worktree::open(root.path())
        .unwrap()
        .apply_plan(&plan)
        .unwrap();

    assert!(!root.path().join("src/old.rs").exists());
    assert_eq!(
        fs::read_to_string(root.path().join("src/new.rs")).unwrap(),
        "fn new() {}\n"
    );
}

#[test]
fn unedited_rename_obeys_the_output_byte_limit() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("old.rs"), "12345").unwrap();
    let plan = WorktreePlan::new(
        "bounded_rename",
        vec![WorktreeOperation::Rename(RenameFile::new(
            "old.rs",
            "new.rs",
            hash("12345"),
        ))],
    );
    let limits = WorktreeLimits {
        max_output_bytes_per_file: 4,
        ..WorktreeLimits::default()
    };
    let worktree =
        Worktree::open_with(root.path(), WorktreeOptions::default().with_limits(limits)).unwrap();

    assert_eq!(
        worktree.dry_run_plan(&plan).unwrap_err().code(),
        WorktreeErrorCode::TransactionTooLarge
    );
    assert_eq!(
        fs::read_to_string(root.path().join("old.rs")).unwrap(),
        "12345"
    );
    assert!(!root.path().join("new.rs").exists());
}

#[test]
fn rename_chains_and_cycles_commit_each_path_once() {
    let chain = tempdir().unwrap();
    fs::write(chain.path().join("a.rs"), "A").unwrap();
    fs::write(chain.path().join("b.rs"), "B").unwrap();
    let plan = WorktreePlan::new(
        "chain",
        vec![
            WorktreeOperation::Rename(RenameFile::new("a.rs", "b.rs", hash("A"))),
            WorktreeOperation::Rename(RenameFile::new("b.rs", "c.rs", hash("B"))),
        ],
    );
    Worktree::open(chain.path())
        .unwrap()
        .apply_plan(&plan)
        .unwrap();
    assert!(!chain.path().join("a.rs").exists());
    assert_eq!(fs::read_to_string(chain.path().join("b.rs")).unwrap(), "A");
    assert_eq!(fs::read_to_string(chain.path().join("c.rs")).unwrap(), "B");

    let cycle = tempdir().unwrap();
    fs::write(cycle.path().join("a.rs"), "A").unwrap();
    fs::write(cycle.path().join("b.rs"), "B").unwrap();
    let plan = WorktreePlan::new(
        "cycle",
        vec![
            WorktreeOperation::Rename(RenameFile::new("a.rs", "b.rs", hash("A"))),
            WorktreeOperation::Rename(RenameFile::new("b.rs", "a.rs", hash("B"))),
        ],
    );
    Worktree::open(cycle.path())
        .unwrap()
        .apply_plan(&plan)
        .unwrap();
    assert_eq!(fs::read_to_string(cycle.path().join("a.rs")).unwrap(), "B");
    assert_eq!(fs::read_to_string(cycle.path().join("b.rs")).unwrap(), "A");
}

#[test]
fn occupied_create_or_rename_destination_fails_without_writes() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("source.rs"), "source").unwrap();
    fs::write(root.path().join("occupied.rs"), "external").unwrap();
    let plan = WorktreePlan::new(
        "occupied",
        vec![WorktreeOperation::Rename(RenameFile::new(
            "source.rs",
            "occupied.rs",
            hash("source"),
        ))],
    );

    assert!(
        Worktree::open(root.path())
            .unwrap()
            .apply_plan(&plan)
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(root.path().join("source.rs")).unwrap(),
        "source"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("occupied.rs")).unwrap(),
        "external"
    );
    assert_no_transaction_artifacts(root.path());
}

#[test]
fn destination_created_after_prepare_is_not_clobbered() {
    let root = tempdir().unwrap();
    let plan = WorktreePlan::new(
        "create",
        vec![WorktreeOperation::Create(CreateFile::new(
            "new.rs", "planned",
        ))],
    );
    let worktree = Worktree::open(root.path()).unwrap();
    let prepared = worktree.prepare_plan(&plan).unwrap();
    fs::write(root.path().join("new.rs"), "external").unwrap();

    assert!(prepared.commit().is_err());
    assert_eq!(
        fs::read_to_string(root.path().join("new.rs")).unwrap(),
        "external"
    );
    assert_no_transaction_artifacts(root.path());
}

#[test]
fn dry_run_is_side_effect_free_and_reports_are_worker_deterministic() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("a.rs"), "A").unwrap();
    let plan = WorktreePlan::new(
        "operations",
        vec![
            WorktreeOperation::Rename(RenameFile::new("a.rs", "b.rs", hash("A"))),
            WorktreeOperation::Create(CreateFile::new("c.rs", "C")),
        ],
    );
    let one = Worktree::open_with(
        root.path(),
        weavatrix_worktree::WorktreeOptions::default().with_parallelism(1),
    )
    .unwrap()
    .dry_run_plan(&plan)
    .unwrap();
    let four = Worktree::open_with(
        root.path(),
        weavatrix_worktree::WorktreeOptions::default().with_parallelism(4),
    )
    .unwrap()
    .dry_run_plan(&plan)
    .unwrap();

    assert_eq!(one, four);
    assert_eq!(fs::read_to_string(root.path().join("a.rs")).unwrap(), "A");
    assert!(!root.path().join("b.rs").exists());
    assert!(!root.path().join("c.rs").exists());
    assert_no_transaction_artifacts(root.path());
}

#[test]
fn explicit_abort_and_dropped_prepare_leave_original_paths() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("old.rs"), "old").unwrap();
    let plan = WorktreePlan::new(
        "move_and_create",
        vec![
            WorktreeOperation::Rename(RenameFile::new("old.rs", "new.rs", hash("old"))),
            WorktreeOperation::Create(CreateFile::new("created.rs", "created")),
        ],
    );
    let engine = Worktree::open(root.path()).unwrap();
    let prepared = engine.prepare_plan(&plan).unwrap();
    let report = prepared.abort().unwrap();
    assert_eq!(report.prepared_files(), 3);
    assert_eq!(
        fs::read_to_string(root.path().join("old.rs")).unwrap(),
        "old"
    );
    assert!(!root.path().join("new.rs").exists());
    assert!(!root.path().join("created.rs").exists());

    let prepared = engine.prepare_plan(&plan).unwrap();
    let transaction_id = prepared.transaction_id().to_owned();
    drop(prepared);
    let recovered = engine.recover().unwrap();
    assert_eq!(recovered.transaction_id(), Some(transaction_id.as_str()));
    assert_eq!(recovered.action(), RecoveryAction::DiscardedStaging);
    assert_eq!(
        fs::read_to_string(root.path().join("old.rs")).unwrap(),
        "old"
    );
    assert!(!root.path().join("new.rs").exists());
    assert!(!root.path().join("created.rs").exists());
    assert_no_transaction_artifacts(root.path());
}

#[test]
fn late_create_conflict_rolls_back_an_earlier_create() {
    let root = tempdir().unwrap();
    let plan = WorktreePlan::new(
        "two_creates",
        vec![
            WorktreeOperation::Create(CreateFile::new("a.rs", "planned-a")),
            WorktreeOperation::Create(CreateFile::new("z.rs", "planned-z")),
        ],
    );
    let prepared = Worktree::open(root.path())
        .unwrap()
        .prepare_plan(&plan)
        .unwrap();
    fs::write(root.path().join("z.rs"), "external").unwrap();

    let error = prepared.commit().unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::ConcurrentModification);
    assert!(!root.path().join("a.rs").exists());
    assert_eq!(
        fs::read_to_string(root.path().join("z.rs")).unwrap(),
        "external"
    );
    assert_no_transaction_artifacts(root.path());
}

#[test]
fn late_conflict_restores_an_earlier_delete_from_its_exact_backup() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("a-delete.rs"), "keep me").unwrap();
    let plan = WorktreePlan::new(
        "delete_then_create",
        vec![
            WorktreeOperation::Delete(DeleteFile::new("a-delete.rs", hash("keep me"))),
            WorktreeOperation::Create(CreateFile::new("z-create.rs", "planned")),
        ],
    );
    let prepared = Worktree::open(root.path())
        .unwrap()
        .prepare_plan(&plan)
        .unwrap();
    fs::write(root.path().join("z-create.rs"), "external").unwrap();

    let error = prepared.commit().unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::ConcurrentModification);
    assert_eq!(
        fs::read_to_string(root.path().join("a-delete.rs")).unwrap(),
        "keep me"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("z-create.rs")).unwrap(),
        "external"
    );
    assert_no_transaction_artifacts(root.path());
}

fn truncate_refactor_journal_after_path_intent(root: &std::path::Path) {
    let journal = root.join(".weavatrix/worktree/active-v3.jsonl");
    let contents = fs::read_to_string(&journal).unwrap();
    let truncated = contents.lines().take(3).collect::<Vec<_>>().join("\n") + "\n";
    fs::write(journal, truncated).unwrap();
}

#[test]
fn recovery_discards_a_partial_stage_written_before_path_staged() {
    let root = tempdir().unwrap();
    let plan = WorktreePlan::new(
        "create",
        vec![WorktreeOperation::Create(CreateFile::new(
            "new.rs",
            "complete staged output",
        ))],
    );
    let engine = Worktree::open(root.path()).unwrap();
    let prepared = engine.prepare_plan(&plan).unwrap();
    let transaction_id = prepared.transaction_id().to_owned();
    drop(prepared);
    truncate_refactor_journal_after_path_intent(root.path());
    fs::write(
        root.path()
            .join(format!(".weavatrix-{transaction_id}-0000.stage")),
        "partial",
    )
    .unwrap();

    let report = engine.recover().unwrap();

    assert_eq!(report.action(), RecoveryAction::DiscardedStaging);
    assert!(!root.path().join("new.rs").exists());
    assert_no_transaction_artifacts(root.path());
}

#[test]
fn recovery_discards_a_partial_backup_written_before_path_staged() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("old.rs"), "complete original").unwrap();
    let plan = WorktreePlan::new(
        "delete",
        vec![WorktreeOperation::Delete(DeleteFile::new(
            "old.rs",
            hash("complete original"),
        ))],
    );
    let engine = Worktree::open(root.path()).unwrap();
    let prepared = engine.prepare_plan(&plan).unwrap();
    let transaction_id = prepared.transaction_id().to_owned();
    drop(prepared);
    truncate_refactor_journal_after_path_intent(root.path());
    fs::write(
        root.path()
            .join(format!(".weavatrix-{transaction_id}-0000.backup")),
        "partial",
    )
    .unwrap();

    let report = engine.recover().unwrap();

    assert_eq!(report.action(), RecoveryAction::DiscardedStaging);
    assert_eq!(
        fs::read_to_string(root.path().join("old.rs")).unwrap(),
        "complete original"
    );
    assert_no_transaction_artifacts(root.path());
}

#[test]
fn every_operation_role_rejects_portable_aliases_of_the_state_namespace() {
    let root = tempdir().unwrap();
    let reserved = ".WeAvAtRiX/worktree/owned.rs";
    let cases = [
        WorktreeOperation::Create(CreateFile::new(reserved, "new")),
        WorktreeOperation::Modify(FileEdit::new(
            reserved,
            hash("old"),
            vec![TextEdit::insert(
                Position::new(1, 0),
                "new",
                Provenance::EXACT_LSP,
            )],
        )),
        WorktreeOperation::Delete(DeleteFile::new(reserved, hash("old"))),
        WorktreeOperation::Rename(RenameFile::new(reserved, "safe.rs", hash("old"))),
        WorktreeOperation::Rename(RenameFile::new("safe.rs", reserved, hash("old"))),
    ];

    for (index, operation) in cases.into_iter().enumerate() {
        let plan = WorktreePlan::new(format!("reserved-{index}"), vec![operation]);
        let error = Worktree::open(root.path())
            .unwrap()
            .dry_run_plan(&plan)
            .unwrap_err();
        assert_eq!(
            error.code(),
            WorktreeErrorCode::ReservedPath,
            "operation case {index} did not reject the reserved namespace"
        );
        assert!(
            !root.path().join(".weavatrix").exists(),
            "invalid plan case {index} touched the filesystem before validation"
        );
    }
}

#[test]
fn refactor_contract_validation_precedes_target_traversal_and_locking() {
    let root = tempdir().unwrap();
    let mut plan = WorktreePlan::new(
        "bounded_evidence",
        vec![WorktreeOperation::Create(CreateFile::new(
            "missing-parent/new.rs",
            "new",
        ))],
    );
    plan.evidence.follow_up = Some("too much evidence".to_owned());
    let limits = WorktreeLimits {
        max_evidence_text_bytes: 4,
        ..WorktreeLimits::default()
    };
    let worktree =
        Worktree::open_with(root.path(), WorktreeOptions::default().with_limits(limits)).unwrap();

    let error = worktree.prepare_plan(&plan).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::InvalidPlan);
    assert_eq!(error.phase(), TransactionPhase::Validate);
    assert!(!root.path().join(".weavatrix").exists());
}

#[cfg(windows)]
fn windows_short_name(root: &std::path::Path, entry: &str) -> Option<String> {
    let command = format!("for %I in ({entry}) do @echo %~sI");
    let output = std::process::Command::new("cmd")
        .args(["/d", "/c", &command])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let name = std::path::Path::new(&value)
        .file_name()?
        .to_string_lossy()
        .into_owned();
    (!name.eq_ignore_ascii_case(entry)).then_some(name)
}

#[cfg(windows)]
#[test]
fn windows_filesystem_aliases_cannot_enter_git_or_state_roots() {
    let root = tempdir().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    fs::write(root.path().join(".git/config"), "protected").unwrap();
    fs::create_dir_all(root.path().join(".weavatrix/worktree")).unwrap();
    fs::write(
        root.path().join(".weavatrix/worktree/owned.rs"),
        "protected",
    )
    .unwrap();

    for (entry, suffix) in [(".git", "config"), (".weavatrix", "worktree/owned.rs")] {
        let Some(alias) = windows_short_name(root.path(), entry) else {
            continue;
        };
        let path = format!("{alias}/{suffix}");
        let plan = WorktreePlan::new(
            "short_alias",
            vec![WorktreeOperation::Delete(DeleteFile::new(
                &path,
                hash("protected"),
            ))],
        );
        let error = Worktree::open(root.path())
            .unwrap()
            .dry_run_plan(&plan)
            .unwrap_err();
        assert_eq!(error.code(), WorktreeErrorCode::ReservedPath, "{path}");
    }

    let file_root = tempdir().unwrap();
    fs::write(file_root.path().join(".git"), "gitdir: protected").unwrap();
    if let Some(alias) = windows_short_name(file_root.path(), ".git") {
        let plan = WorktreePlan::new(
            "short_alias_file",
            vec![WorktreeOperation::Delete(DeleteFile::new(
                &alias,
                hash("gitdir: protected"),
            ))],
        );
        let error = Worktree::open(file_root.path())
            .unwrap()
            .dry_run_plan(&plan)
            .unwrap_err();
        assert_eq!(error.code(), WorktreeErrorCode::ReservedPath, "{alias}");
    }
}
