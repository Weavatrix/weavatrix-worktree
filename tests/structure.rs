use std::path::{Path, PathBuf};

#[test]
fn production_modules_stay_within_the_architecture_limit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut pending = vec![root];
    let mut oversized = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|value| value == "rs") {
                check_lines(&path, &mut oversized);
            }
        }
    }
    assert!(
        oversized.is_empty(),
        "production Rust modules exceed 300 lines: {oversized:?}"
    );
}

fn check_lines(path: &Path, oversized: &mut Vec<(PathBuf, usize)>) {
    let lines = std::fs::read_to_string(path).unwrap().lines().count();
    if lines > 300 {
        oversized.push((path.to_path_buf(), lines));
    }
}
