use std::path::Path;

use weavatrix_refactor_plan::{EditPlan, FileEdit, Position, Provenance, TextEdit, TextRange};
use weavatrix_worktree::{
    Sha256Hash, Worktree, WorktreeErrorCode, WorktreeLimits, WorktreeOptions,
};

use crate::support::{fixture, worktree};

#[test]
fn unicode_apply_preserves_evidence_and_portable_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("unicode.rs");
    let source = "שלום 🌍\n";
    std::fs::write(&path, source).unwrap();
    let original_permissions = configured_permissions(&path);
    let plan = EditPlan::new(
        "unicode_replace",
        vec![FileEdit::new(
            "unicode.rs",
            Sha256Hash::compute(source.as_bytes()).to_string(),
            vec![TextEdit::replace(
                TextRange::new(Position::new(1, 5), Position::new(1, 7)),
                "🌍",
                "עולם",
                Provenance::EXACT_LSP,
            )],
        )],
    );

    let report = Worktree::open(temp.path()).unwrap().apply(&plan).unwrap();

    let expected = "שלום עולם\n";
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    let change = &report.files()[0];
    assert_eq!(change.path(), "unicode.rs");
    assert_eq!(change.old_sha256(), Sha256Hash::compute(source.as_bytes()));
    assert_eq!(
        change.new_sha256(),
        Sha256Hash::compute(expected.as_bytes())
    );
    assert_eq!(change.edits_applied(), 1);
    let final_permissions = std::fs::metadata(&path).unwrap().permissions();
    assert_eq!(
        final_permissions.readonly(),
        original_permissions.readonly()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(final_permissions.mode() & 0o777, 0o754);
    }
}

#[test]
fn reports_are_identical_for_one_two_four_and_eight_workers() {
    let (temp, plan, _) = fixture(10);
    let expected = worktree(temp.path(), 1).dry_run(&plan).unwrap();

    for workers in [2, 4, 8] {
        assert_eq!(
            worktree(temp.path(), workers).dry_run(&plan).unwrap(),
            expected
        );
    }
    assert_eq!(expected.total_edits(), 10);
    assert_eq!(expected.operation(), "benchmark_edit");
}

#[test]
fn public_plan_guards_reject_unsafe_and_aliased_paths() {
    let temp = tempfile::tempdir().unwrap();
    let cases = [
        vec![plan_file("/absolute.rs")],
        vec![plan_file("src/../outside.rs")],
        vec![plan_file(".git/config")],
        vec![plan_file("same.rs"), plan_file("same.rs")],
        vec![plan_file("Src/A.rs"), plan_file("src/a.rs")],
    ];

    for files in cases {
        let plan = EditPlan::new("invalid_path", files);
        assert_eq!(
            Worktree::open(temp.path())
                .unwrap()
                .dry_run(&plan)
                .unwrap_err()
                .code(),
            WorktreeErrorCode::InvalidPlan
        );
    }
}

#[test]
fn wrong_before_and_structurally_invalid_edits_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let source = "alpha\n";
    std::fs::write(temp.path().join("file.rs"), source).unwrap();
    let range = TextRange::new(Position::new(1, 0), Position::new(1, 1));
    let wrong_before = one_file_plan(
        source,
        TextEdit::replace(range, "z", "A", Provenance::EXACT_LSP),
    );
    let invalid_edit = one_file_plan(
        source,
        TextEdit::replace(range, "a", "a", Provenance::EXACT_LSP),
    );
    let engine = Worktree::open(temp.path()).unwrap();

    assert_eq!(
        engine.dry_run(&wrong_before).unwrap_err().code(),
        WorktreeErrorCode::EditRejected
    );
    assert_eq!(
        engine.dry_run(&invalid_edit).unwrap_err().code(),
        WorktreeErrorCode::InvalidPlan
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("file.rs")).unwrap(),
        source
    );
}

#[test]
fn file_edit_source_and_output_limits_are_enforced() {
    let (two, plan_two, _) = fixture(2);
    let file_limited = WorktreeLimits {
        max_files: 1,
        ..WorktreeLimits::default()
    };
    assert_dry_code(
        two.path(),
        &plan_two,
        file_limited,
        WorktreeErrorCode::InvalidPlan,
    );

    let (one, mut plan_one, _) = fixture(1);
    let duplicate_edit = plan_one.files[0].edits[0].clone();
    plan_one.files[0].edits.push(duplicate_edit);
    let edit_limited = WorktreeLimits {
        max_edits_per_file: 1,
        ..WorktreeLimits::default()
    };
    assert_dry_code(
        one.path(),
        &plan_one,
        edit_limited,
        WorktreeErrorCode::InvalidPlan,
    );

    let (one, plan_one, _) = fixture(1);
    let source_limited = WorktreeLimits {
        max_source_bytes_per_file: 7,
        ..WorktreeLimits::default()
    };
    assert_dry_code(
        one.path(),
        &plan_one,
        source_limited,
        WorktreeErrorCode::TransactionTooLarge,
    );
    let output_limited = WorktreeLimits {
        max_output_bytes_per_file: 8,
        ..WorktreeLimits::default()
    };
    assert_dry_code(
        one.path(),
        &plan_one,
        output_limited,
        WorktreeErrorCode::TransactionTooLarge,
    );
}

#[test]
fn total_artifact_and_journal_limits_are_enforced() {
    let (temp, plan, _) = fixture(2);
    let source_total = WorktreeLimits {
        max_source_bytes_per_file: 8,
        max_total_source_bytes: 8,
        ..WorktreeLimits::default()
    };
    assert_dry_code(
        temp.path(),
        &plan,
        source_total,
        WorktreeErrorCode::TransactionTooLarge,
    );
    let output_total = WorktreeLimits {
        max_output_bytes_per_file: 9,
        max_total_output_bytes: 9,
        ..WorktreeLimits::default()
    };
    assert_dry_code(
        temp.path(),
        &plan,
        output_total,
        WorktreeErrorCode::TransactionTooLarge,
    );

    let invalid_artifacts = WorktreeLimits {
        max_source_bytes_per_file: 8,
        max_output_bytes_per_file: 9,
        max_total_source_bytes: 8,
        max_total_output_bytes: 9,
        max_total_artifact_bytes: 16,
        ..WorktreeLimits::default()
    };
    let result = Worktree::open_with(
        temp.path(),
        WorktreeOptions::default().with_limits(invalid_artifacts),
    );
    let Err(error) = result else {
        panic!("invalid artifact limits unexpectedly opened a worktree");
    };
    assert_eq!(error.code(), WorktreeErrorCode::InvalidOptions);

    let (journal_temp, journal_plan, _) = fixture(1);
    let journal_limited = WorktreeLimits {
        max_journal_bytes: 1,
        ..WorktreeLimits::default()
    };
    let error = engine(journal_temp.path(), journal_limited)
        .apply(&journal_plan)
        .unwrap_err();
    assert_eq!(error.code(), WorktreeErrorCode::JournalCorrupt);
    assert!(error.recovery_required());
}

#[test]
fn legacy_edit_plan_metadata_limits_are_enforced_before_projection() {
    let (temp, mut plan, _) = fixture(1);
    plan.extensions.insert(
        "payload".to_owned(),
        blazingly_json::Value::String("x".repeat(1024 * 1024)),
    );
    let limits = WorktreeLimits {
        max_extension_bytes: 128,
        ..WorktreeLimits::default()
    };

    assert_dry_code(temp.path(), &plan, limits, WorktreeErrorCode::InvalidPlan);
}

fn plan_file(path: &str) -> FileEdit {
    FileEdit::new(
        path,
        "0".repeat(64),
        vec![TextEdit::insert(
            Position::new(1, 0),
            "x",
            Provenance::EXACT_LSP,
        )],
    )
}

fn one_file_plan(source: &str, edit: TextEdit) -> EditPlan {
    EditPlan::new(
        "invalid_edit",
        vec![FileEdit::new(
            "file.rs",
            Sha256Hash::compute(source.as_bytes()).to_string(),
            vec![edit],
        )],
    )
}

fn engine(root: &Path, limits: WorktreeLimits) -> Worktree {
    Worktree::open_with(
        root,
        WorktreeOptions::default()
            .with_limits(limits)
            .with_parallelism(1),
    )
    .unwrap()
}

fn configured_permissions(path: &Path) -> std::fs::Permissions {
    let permissions = std::fs::metadata(path).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = permissions;
        permissions.set_mode(0o754);
        std::fs::set_permissions(path, permissions.clone()).unwrap();
        permissions
    }
    #[cfg(not(unix))]
    {
        permissions
    }
}

fn assert_dry_code(
    root: &Path,
    plan: &EditPlan,
    limits: WorktreeLimits,
    expected: WorktreeErrorCode,
) {
    assert_eq!(
        engine(root, limits).dry_run(plan).unwrap_err().code(),
        expected
    );
}
