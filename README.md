# weavatrix-worktree

Bounded, crash-recoverable multi-file execution of
[`weavatrix-refactor-plan`](https://github.com/sergii-ziborov/weavatrix-refactor-plan)
contracts.

`weavatrix-edit` owns exact immutable-text edits. `weavatrix-refactor-plan`
owns the versioned modify/create/delete/rename contract, its evidence,
validation profiles, limits, and canonical fingerprint. `weavatrix-worktree`
adds only the filesystem execution layer needed by a refactoring engine:
repository-relative path confinement, SHA-256 compare-and-swap, bounded
parallel preparation, durable adjacent stage/backup files, deterministic
multi-file commit, rollback, and journal-based crash recovery.

## Status

Version `0.2.1` retains the existing `EditPlan` runtime API and executes the
versioned `RefactorPlan` contract for exact `modify`, `create`, `delete`, and
`rename` operations. The compatibility names `WorktreePlan`,
`WorktreeOperation`, `WorktreePlanLimits`, and `WORKTREE_PLAN_SCHEMA` are public
aliases; the contract itself has one owner in `weavatrix-refactor-plan`.
Rename chains and cycles are compiled into one transition per unique path, and
renames may carry exact `weavatrix-edit` edits that are applied in transit.
Git operations, hooks, formatters, tests, and language analysis remain higher
layers.

Install the released crates from crates.io:

```toml
[dependencies]
weavatrix-refactor-plan = "0.1.1"
weavatrix-worktree = "0.2.1"
```

The crate requires Rust 1.88 or newer and contains no async runtime or unsafe
Rust.

## Example

```no_run
use weavatrix_refactor_plan::{EditPlan, FileEdit, Position, Provenance, TextEdit};
use weavatrix_worktree::{Sha256Hash, Worktree};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let source = "let old_name = 1;\n";
let edit = TextEdit::replace(
    weavatrix_refactor_plan::TextRange::new(Position::new(1, 4), Position::new(1, 12)),
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

Resource operations use the parallel `*_plan` API:

```no_run
use weavatrix_refactor_plan::{
    CreateFile, DeleteFile, RefactorOperation, RefactorPlan, RenameFile,
};
use weavatrix_worktree::{Sha256Hash, Worktree};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let old = "pub fn old() {}\n";
let plan = RefactorPlan::new(
    "move_module",
    vec![
        RefactorOperation::Rename(RenameFile::new(
            "src/old.rs",
            "src/new.rs",
            Sha256Hash::compute(old.as_bytes()).to_string(),
        )),
        RefactorOperation::Create(CreateFile::new("src/generated.rs", "// generated\n")),
        RefactorOperation::Delete(DeleteFile::new(
            "src/obsolete.rs",
            Sha256Hash::compute(b"obsolete\n").to_string(),
        )),
    ],
);

let worktree = Worktree::open(".")?;
let preview = worktree.dry_run_plan(&plan)?;
assert_eq!(preview.files().len(), 3);
worktree.apply_plan(&plan)?;
# Ok(())
# }
```

`prepare_plan()` returns a `PreparedWorktreeTransaction` with the same explicit
`commit()` / `abort()` boundary. Create and rename destinations must be absent
unless the destination is consumed by another rename in the same chain or
cycle. Parent directories must already exist; directory creation is not part of
the v0.2 transaction contract.

## Retained undo

`apply_plan_retained` (or `commit_retained` on a prepared transaction) commits
a plan while keeping each replaced file's exact backup and writing a
checksummed `undo-<transaction>.json` receipt under `.weavatrix/worktree`. The
retained backup files are moved into that state directory after the durable
commit record, so successful applies leave no `.weavatrix-*.backup` files
beside source files. Interrupted moves are completed idempotently by recovery.
returned `RetainedApplyReport` carries the `UndoId`; `undo_receipts()` and
`undo_usage()` inspect the bounded store (32 receipts / 384 MiB by default),
`rollback_undo(&id)` restores the exact before state, and `discard_undo(&id)`
verifies and removes the retained artifacts.

Rollback is compare-and-swap over complete slot evidence: every path must
still match the receipt's committed after state (hash, size, permissions, and
file identity) and every retained artifact its recorded evidence, otherwise it
fails with `UNDO_CONFLICT` before touching the tree. Any later transaction on
the same path therefore deliberately invalidates the older receipt. A rollback
interrupted mid-flight leaves a durable `active-undo.jsonl` journal; the next
`recover()` completes the restore idempotently and consumes the receipt, and
new transactions are refused until it does.

The worktree uses `validate_executor_plan`: an omitted semantic-completeness
claim or a reviewed `PARTIAL` claim does not prevent execution of otherwise
exact, filesystem-safe operations. An explicit `COMPLETE` claim is still
checked for truthful typed proof and must not contain recorded evidence gaps.
Use the plan crate's stricter planner profile when admitting producer output.

## Parallel multi-file execution

Reading, hashing, edit preparation, backup creation, and output staging can run
concurrently. The default is
`min(available_parallelism, 4, file_count)` workers; callers can lower it and
the hard maximum is 16. Limits also cover file count, edits, per-file and total
source/output bytes, artifact bytes, journal size, and plan metadata. By
default an operation label is limited to 4,096 UTF-8 bytes (as is the optional
legacy `EditPlan` completeness value). All extension maps in one plan share an
aggregate budget of 256 KiB of serialized JSON, 4,096 JSON value nodes, and a
maximum nesting depth of 32. The aggregate includes plan-, file-, operation-,
and text-edit-level extensions rather than granting each map a fresh budget.
Typed refactor evidence is separately bounded to 10,000 entries, 8 MiB of text,
and 256 bytes per status/code value.

These metadata ceilings are part of plan admission, not only journal sizing.
An over-budget plan fails validation before target traversal and before a new
journal, stage/backup artifact, or target mutation is created. Callers may
customize the ceilings through `WorktreeLimits`; invalid or zero limits fail
closed.

Commit is intentionally serial in portable path order, and rollback uses the
exact reverse order. For the expected 5–10 file refactor this retains most of
the useful parallel speedup while making failures and recovery reproducible.
Worker completion order never changes reports or the selected primary error.

## Transaction guarantee

There is no portable filesystem primitive that atomically replaces unrelated
paths. Another process may briefly observe a mixture of old and new files.
The guarantee is therefore **crash-recoverable all-or-restored**, not
observational multi-file atomicity:

- no path changes before every input and absent destination is validated and
  every required backup/output is staged and durably recorded;
- a successful commit leaves every target at the recorded new SHA-256;
- a normal failure rolls committed targets back in reverse order;
- an interrupted transaction remains recoverable from a synchronized journal;
- recovery refuses to overwrite a file matching neither its old nor new hash.

See the normative [transaction model](docs/transaction-model.md) for state
transitions, path protections, durability boundaries, and metadata limits.

## Safety boundaries

Plans and paths fail closed when they contain path aliases, reserved paths,
symlink/reparse traversal, cross-filesystem parents, hard links, read-only or
special files, invalid UTF-8, stale SHA-256 evidence, conflicting edits, or
resource-budget overflow. Active-transaction stage and backup files are
created exclusively next to their target; retained backups move into the
state directory only after a durable commit. Create uses deterministic
writable `0o644` (`0o755` when
explicitly executable); modify and rename preserve portable permissions.
Version 0.2 does not
promise ownership, ACL, xattr, alternate-stream, or sparse-layout cloning.
File and journal contents are synchronized on every platform. Parent-directory
`fsync` is required on Unix and any failure is surfaced. This is an
OS/filesystem synchronization contract, not proof that a storage device honored
its flushes. Windows rejects `FlushFileBuffers` for directory handles, so
directory-entry persistence there is best-effort. Version 0.2 does not claim
absolute power-loss durability on every filesystem or storage device.

The root lock coordinates cooperating `weavatrix-worktree` callers. A hostile
or unaware process with write access can still race the final hash check and
rename; portable filesystems provide no atomic compare-and-replace primitive.
Callers needing hostile-writer isolation must provide it outside this crate.

## Competitors and benchmarks

The [competitor audit](docs/competitors.md) compares this contract with
`atomwrite`, `textum`, `rustfix`, Cargo Fix, the system Git CLI's `git apply`,
libgit2, and gix. The Git CLI baseline is unrelated to the read-only
[`weavatrix-git`](https://github.com/sergii-ziborov/weavatrix-git) project.
The [benchmark protocol](docs/benchmarks.md) defines correctness-gated 1, 5,
10, and 64-file scenarios, concurrency sweeps, durable/non-durable separation,
fault injection, and resource measurements. Benchmark numbers are published
only with exact commands, machine data, and output-equivalence evidence. The
[2026-08-02 benchmark report](docs/benchmark-results-2026-08-02.md) links its
complete public raw evidence bundle.

## License

MIT
