# weavatrix-worktree

Bounded, crash-recoverable multi-file application of
[`weavatrix-edit`](https://github.com/sergii-ziborov/weavatrix-edit) plans.

`weavatrix-edit` proves that edits are valid for one immutable UTF-8 string.
`weavatrix-worktree` adds the filesystem layer needed by a refactoring engine:
repository-relative path confinement, SHA-256 compare-and-swap, bounded
parallel preparation, durable adjacent stage/backup files, deterministic
multi-file commit, rollback, and journal-based crash recovery.

## Status

Version `0.1.0` is an initial Rust implementation. Its deliberately narrow
write contract modifies existing regular UTF-8 files only. Create, delete,
rename, Git operations, hooks, formatters, tests, and language analysis belong
to higher layers such as `weavatrix-refactor-rust`.

The Git dependency is currently the supported installation path:

```toml
[dependencies]
weavatrix-edit = { git = "https://github.com/sergii-ziborov/weavatrix-edit", rev = "f37584be0bcb28f69cc75d9e59bd300ff8964ba6" }
weavatrix-worktree = { git = "https://github.com/sergii-ziborov/weavatrix-worktree" }
```

The crate requires Rust 1.88 or newer and contains no async runtime or unsafe
Rust.

## Example

```no_run
use weavatrix_edit::{EditPlan, FileEdit, Position, Provenance, TextEdit};
use weavatrix_worktree::{Sha256Hash, Worktree};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let source = "let old_name = 1;\n";
let edit = TextEdit::replace(
    weavatrix_edit::TextRange::new(Position::new(1, 4), Position::new(1, 12)),
    "old_name",
    "new_name",
    Provenance::EXACT_LSP,
);
let plan = EditPlan::new(
    "rename_symbol",
    vec![FileEdit::new(
        "src/lib.rs",
        Sha256Hash::compute(source.as_bytes()).to_string(),
        vec![edit],
    )],
);

let worktree = Worktree::open(".")?;
let preview = worktree.dry_run(&plan)?; // no lock, journal, or temp files
assert_eq!(preview.files().len(), 1);
let report = worktree.apply(&plan)?;
assert_eq!(report.files().len(), 1);
# Ok(())
# }
```

For callers that need a confirmation boundary, use `prepare()` and then call
`PreparedTransaction::commit()` or `abort()`. A process interrupted after
durable preparation is repaired with `Worktree::recover()`.

## Parallel multi-file execution

Reading, hashing, edit preparation, backup creation, and output staging can run
concurrently. The default is
`min(available_parallelism, 4, file_count)` workers; callers can lower it and
the hard maximum is 16. Limits also cover file count, edits, per-file and total
source/output bytes, artifact bytes, and journal size.

Commit is intentionally serial in portable path order, and rollback uses the
exact reverse order. For the expected 5–10 file refactor this retains most of
the useful parallel speedup while making failures and recovery reproducible.
Worker completion order never changes reports or the selected primary error.

## Transaction guarantee

There is no portable filesystem primitive that atomically replaces unrelated
paths. Another process may briefly observe a mixture of old and new files.
The guarantee is therefore **crash-recoverable all-or-restored**, not
observational multi-file atomicity:

- no target changes before every file is validated, backed up, staged, and
  durably recorded;
- a successful commit leaves every target at the recorded new SHA-256;
- a normal failure rolls committed targets back in reverse order;
- an interrupted transaction remains recoverable from a synchronized journal;
- recovery refuses to overwrite a file matching neither its old nor new hash.

See the normative [transaction model](docs/transaction-model.md) for state
transitions, path protections, durability boundaries, and metadata limits.

## Safety boundaries

Plans and targets fail closed when they contain path aliases, reserved paths,
symlink/reparse traversal, cross-filesystem parents, hard links, read-only or
special files, invalid UTF-8, stale SHA-256 evidence, conflicting edits, or
resource-budget overflow. Stage and backup files are created exclusively next
to their target. Version 0.1 preserves portable file permissions; it does not
promise ownership, ACL, xattr, alternate-stream, or sparse-layout cloning.
File and journal contents are synchronized on every platform. Parent-directory
`fsync` is required on Unix and any failure is surfaced. This is an
OS/filesystem synchronization contract, not proof that a storage device honored
its flushes. Windows rejects `FlushFileBuffers` for directory handles, so
directory-entry persistence there is best-effort. Version 0.1 does not claim
absolute power-loss durability on every filesystem or storage device.

The root lock coordinates cooperating `weavatrix-worktree` callers. A hostile
or unaware process with write access can still race the final hash check and
rename; portable filesystems provide no atomic compare-and-replace primitive.
Callers needing hostile-writer isolation must provide it outside this crate.

## Competitors and benchmarks

The [competitor audit](docs/competitors.md) compares this contract with
`atomwrite`, `textum`, `rustfix`, Cargo Fix, `git apply`, libgit2, and gix.
The [benchmark protocol](docs/benchmarks.md) defines correctness-gated 1, 5,
10, and 64-file scenarios, concurrency sweeps, durable/non-durable separation,
fault injection, and resource measurements. Benchmark numbers are published
only with exact commands, machine data, and output-equivalence evidence. The
[2026-08-02 benchmark report](docs/benchmark-results-2026-08-02.md) links its
complete public raw evidence bundle.

## License

MIT
