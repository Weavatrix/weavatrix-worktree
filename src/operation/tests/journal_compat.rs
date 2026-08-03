use std::{fs, fs::OpenOptions};

use tempfile::tempdir;

use crate::{RecoveryAction, WorktreeErrorCode, filesystem::FsRoot};

use super::super::{
    journal::{Record, Writer, read},
    prepare_operation_plan, recover_operation_transaction,
};
use super::create_plan;

const V2_FILE: &str = ".weavatrix/worktree/active-v2.jsonl";
const V3_FILE: &str = ".weavatrix/worktree/active-v3.jsonl";

#[test]
fn v3_header_uses_the_validated_canonical_plan_fingerprint() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = crate::WorktreeOptions::default();
    let mut plan = create_plan("new.rs", "new");
    plan.evidence.created_at = Some("2026-08-02T12:00:00Z".to_owned());
    let expected = plan.fingerprint().unwrap().to_string();
    let buffered = crate::hash::serialized_hash(&plan).unwrap().to_string();
    assert_ne!(expected, buffered, "fixture must distinguish V3 from V2");

    let prepared = prepare_operation_plan(&root, options, &plan).unwrap();
    let file = OpenOptions::new()
        .read(true)
        .open(temp.path().join(V3_FILE))
        .unwrap();
    let entries = read(&file, options.limits.max_journal_bytes as u64).unwrap();
    let Record::Header { contract_hash, .. } = &entries[0].record else {
        panic!("first record is not a header");
    };
    assert_eq!(contract_hash, &expected);
    drop(prepared);
}

#[test]
fn recovery_replays_a_v2_fixture_without_reinterpreting_its_contract_hash() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = crate::WorktreeOptions::default();
    let records = prepared_records(&root, options, temp.path());
    write_v2_fixture(temp.path(), records);

    let report = recover_operation_transaction(&root, options)
        .unwrap()
        .unwrap();

    assert_eq!(report.action(), RecoveryAction::DiscardedStaging);
    assert!(!temp.path().join("new.rs").exists());
    assert!(!temp.path().join(V2_FILE).exists());
    assert!(!temp.path().join(V3_FILE).exists());
}

#[test]
fn corrupt_v2_fixture_fails_closed_and_is_not_removed() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = crate::WorktreeOptions::default();
    let records = prepared_records(&root, options, temp.path());
    write_v2_fixture(temp.path(), records);
    corrupt_first_checksum(&temp.path().join(V2_FILE));

    let error = recover_operation_transaction(&root, options).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::JournalCorrupt);
    assert!(error.recovery_required());
    assert!(temp.path().join(V2_FILE).exists());
}

#[test]
fn unknown_journal_version_fails_closed_and_is_not_removed() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = crate::WorktreeOptions::default();
    let _records = prepared_records(&root, options, temp.path());
    let path = temp.path().join(V3_FILE);
    let contents = fs::read_to_string(&path).unwrap().replace(
        "weavatrix.worktree-journal.v3",
        "weavatrix.worktree-journal.v4",
    );
    fs::write(&path, contents).unwrap();

    let error = recover_operation_transaction(&root, options).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::JournalCorrupt);
    assert!(error.recovery_required());
    assert!(path.exists());
}

#[test]
fn mixed_v2_and_v3_records_in_one_journal_fail_closed() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = crate::WorktreeOptions::default();
    let _records = prepared_records(&root, options, temp.path());
    let path = temp.path().join(V3_FILE);
    let mut contents = fs::read_to_string(&path).unwrap();
    let marker = "weavatrix.worktree-journal.v3";
    let first = contents.find(marker).unwrap();
    let second = first + marker.len() + contents[first + marker.len()..].find(marker).unwrap();
    contents.replace_range(
        second..second + marker.len(),
        "weavatrix.worktree-journal.v2",
    );
    fs::write(&path, contents).unwrap();

    let error = recover_operation_transaction(&root, options).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::JournalCorrupt);
    assert!(error.recovery_required());
    assert!(path.exists());
}

#[test]
fn simultaneous_v2_and_v3_operation_journals_fail_closed() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = crate::WorktreeOptions::default();
    let _records = prepared_records(&root, options, temp.path());
    fs::copy(temp.path().join(V3_FILE), temp.path().join(V2_FILE)).unwrap();

    let error = recover_operation_transaction(&root, options).unwrap_err();

    assert_eq!(error.code(), WorktreeErrorCode::JournalCorrupt);
    assert!(error.recovery_required());
    assert!(temp.path().join(V2_FILE).exists());
    assert!(temp.path().join(V3_FILE).exists());

    let error =
        prepare_operation_plan(&root, options, &create_plan("other.rs", "other")).unwrap_err();
    assert_eq!(error.code(), WorktreeErrorCode::JournalCorrupt);
    assert!(error.recovery_required());
}

#[test]
fn simultaneous_edit_and_operation_journals_block_recovery_and_prepare() {
    let temp = tempdir().unwrap();
    let root = FsRoot::open(temp.path()).unwrap();
    let options = crate::WorktreeOptions::default();
    let _records = prepared_records(&root, options, temp.path());
    let edit_journal = temp.path().join(".weavatrix/worktree/active.jsonl");
    fs::write(&edit_journal, "").unwrap();

    let error = recover_operation_transaction(&root, options).unwrap_err();
    assert_eq!(error.code(), WorktreeErrorCode::JournalCorrupt);
    assert!(error.recovery_required());

    let error =
        prepare_operation_plan(&root, options, &create_plan("another.rs", "another")).unwrap_err();
    assert_eq!(error.code(), WorktreeErrorCode::RecoveryRequired);
    assert!(edit_journal.exists());
    assert!(temp.path().join(V3_FILE).exists());
}

fn prepared_records(
    root: &FsRoot,
    options: crate::WorktreeOptions,
    path: &std::path::Path,
) -> Vec<Record> {
    let prepared = prepare_operation_plan(root, options, &create_plan("new.rs", "new")).unwrap();
    let file = OpenOptions::new()
        .read(true)
        .open(path.join(V3_FILE))
        .unwrap();
    let records = read(&file, options.limits.max_journal_bytes as u64)
        .unwrap()
        .into_iter()
        .map(|entry| entry.record)
        .collect();
    drop(prepared);
    records
}

fn write_v2_fixture(root: &std::path::Path, records: Vec<Record>) {
    fs::remove_file(root.join(V3_FILE)).unwrap();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(root.join(V2_FILE))
        .unwrap();
    let mut writer = Writer::new_legacy_fixture(file, 1024 * 1024).unwrap();
    for mut record in records {
        if let Record::Header { contract_hash, .. } = &mut record {
            *contract_hash = "01".repeat(32);
        }
        writer.append(&record).unwrap();
    }
}

fn corrupt_first_checksum(path: &std::path::Path) {
    let mut contents = fs::read_to_string(path).unwrap();
    let marker = "\"checksum\":\"";
    let index = contents.find(marker).unwrap() + marker.len();
    let replacement = if contents.as_bytes()[index] == b'0' {
        "1"
    } else {
        "0"
    };
    contents.replace_range(index..=index, replacement);
    fs::write(path, contents).unwrap();
}
