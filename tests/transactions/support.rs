use std::path::{Path, PathBuf};

use tempfile::TempDir;
use weavatrix_edit::{EditPlan, FileEdit, Position, Provenance, TextEdit};
use weavatrix_worktree::{Sha256Hash, Worktree, WorktreeOptions};

pub(crate) fn fixture(count: usize) -> (TempDir, EditPlan, Vec<(String, String)>) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("src")).unwrap();
    let mut files = Vec::with_capacity(count);
    let mut originals = Vec::with_capacity(count);
    for index in (0..count).rev() {
        let path = format!("src/file-{index:02}.rs");
        let source = format!("value_{index}\n");
        std::fs::write(temp.path().join(&path), &source).unwrap();
        let position = Position::new(1, u32::try_from(source.len() - 1).unwrap());
        files.push(FileEdit::new(
            &path,
            Sha256Hash::compute(source.as_bytes()).to_string(),
            vec![TextEdit::insert(position, "!", Provenance::EXACT_LSP)],
        ));
        originals.push((path, source));
    }
    (temp, EditPlan::new("benchmark_edit", files), originals)
}

pub(crate) fn worktree(root: &Path, workers: usize) -> Worktree {
    Worktree::open_with(root, WorktreeOptions::default().with_parallelism(workers)).unwrap()
}

pub(crate) fn plan_for_existing(root: &Path, count: usize) -> EditPlan {
    let mut files = Vec::with_capacity(count);
    for index in 0..count {
        let path = format!("src/file-{index:02}.rs");
        let source = std::fs::read_to_string(root.join(&path)).unwrap();
        files.push(FileEdit::new(
            path,
            Sha256Hash::compute(source.as_bytes()).to_string(),
            vec![TextEdit::insert(
                Position::new(1, u32::try_from(source.len() - 1).unwrap()),
                "!",
                Provenance::EXACT_LSP,
            )],
        ));
    }
    EditPlan::new("crash_recovery", files)
}

pub(crate) fn assert_no_transaction_artifacts(root: &Path) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                pending.push(entry.path());
            } else {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                assert!(
                    !name.starts_with(".weavatrix-") && name != "active.jsonl",
                    "left transaction artifact: {}",
                    entry.path().display()
                );
            }
        }
    }
}

pub(crate) fn find_artifact(root: &Path, suffix: &str) -> PathBuf {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else if entry.file_name().to_string_lossy().ends_with(suffix) {
                return entry.path();
            }
        }
    }
    panic!("missing artifact ending in {suffix}");
}
