# Weavatrix Worktree transaction model

Status: normative architecture contract for `weavatrix-worktree` v0.2.

## Purpose and boundary

`weavatrix-worktree` is the filesystem transaction layer above
`weavatrix-refactor-plan`, which in turn uses `weavatrix-edit` for exact changes
to one immutable UTF-8 source. The plan crate is the sole owner of the operation
schema, evidence, pure validation, bounds, and canonical fingerprint. This
crate resolves repository-relative paths, verifies current file hashes, stages
output, commits several files, and recovers interrupted transactions.

Version 0.2 retains the existing-file `EditPlan` API and executes
`RefactorPlan` with exact modify, create, delete, and rename operations.
`WorktreePlan` remains a public compatibility alias. Rename chains and cycles
are reduced to one state transition per unique path; a rename may apply
validated text edits to its source while moving it. Parent directories must
already exist. The crate does not run Git, hooks, formatters, compilers, tests,
MCP, or network operations.

Execution uses the plan crate's executor profile. It accepts omitted semantic
completeness and reviewed `PARTIAL` plans when every submitted operation is
exact and filesystem-safe. Explicit `COMPLETE` claims remain subject to typed,
truthful proof validation and cannot coexist with recorded evidence gaps. The
strict planner profile belongs at the producer-admission boundary, not in this
filesystem executor.

## Guarantee: recoverable, not observationally atomic

No portable filesystem primitive atomically replaces a set of unrelated
paths. Commit therefore replaces targets one at a time in deterministic path
order. Another process may briefly observe a mixture of old and new files.

The v0.2 guarantee is instead **crash-recoverable all-or-restored**:

- no path changes before every present input and absent destination is
  validated and every required backup/output is staged and durably recorded;
- a normal successful commit leaves every path at its recorded present/absent
  state;
- a reported commit failure rolls already-committed targets back in reverse
  order;
- an interrupted process leaves a durable journal from which `recover()` can
  restore the old set;
- recovery never overwrites a path whose complete state matches neither the
  recorded before state nor the recorded after state.

The crate must not describe this contract as an observationally atomic
multi-file commit.

## Runtime invariants

1. Plan paths are portable, repository-relative, unique, and outside the
   reserved `.weavatrix/worktree` namespace.
2. Every present input is a regular UTF-8 file beneath the opened root
   capability; create and ordinary rename destinations are absent slots whose
   parent already exists.
3. Root, parent components, and targets are not symlinks, junctions, or other
   reparse points.
4. Every target is on the root filesystem or volume.
5. Different plan paths cannot resolve to the same filesystem identity.
6. Hard-linked, read-only, non-UTF-8, oversized, and concurrently changed
   inputs fail closed in v0.2.
7. Full SHA-256, byte count, identity, and portable permission evidence is
   checked before staging and again before a visible path mutation.
8. Stage and backup files are created with `create_new` in the target's parent
   directory, so replacement cannot cross a filesystem boundary.
9. Filesystem actions that may affect recovery happen only after their journal
   intent is flushed and synchronized.
10. Commit order is ascending portable path order; rollback order is its exact
    reverse. Worker completion order never affects reports or errors.

File and journal contents are synchronized on every supported platform.
Parent-directory `fsync` is required on Unix and failures are surfaced; the
guarantee is limited to the synchronization calls accepted by the OS and
filesystem, not proof that a storage device honored its flushes. Windows
directory handles reject `FlushFileBuffers`, so directory-entry persistence is
best-effort there. Version 0.2 does not claim absolute power-loss durability on
every filesystem or storage device.

The advisory root lock serializes cooperating library users. An unrelated
writer can still race the immediate hash/identity revalidation and the target
rename because portable filesystems provide no atomic compare-and-replace.
This residual leaf race is outside the v0.2 guarantee and must not be described
as protection against a hostile same-root writer.

## Component boundaries

The package and runtime dependency direction is:

```text
weavatrix-edit <- weavatrix-refactor-plan <- contract adapter --------+
  ^             ^              ^                 ^                     |
  |             |              |                 |                     |
filesystem   journal        scheduler       edit_bridge                |
  ^             ^              ^                 ^                     |
  +-------------+--------------+-----------------+                     |
                              transaction <-----------------------------+
                                  ^
                                  |
                              facade/API
```

- `contract adapter`: maps `RefactorPlanLimits`, validated plans, canonical
  fingerprints, and plan errors into worktree runtime limits and errors;
- `filesystem`: capability-rooted path traversal, identity, metadata,
  create-new artifacts, atomic single-file replace, and durability barriers;
- `journal`: bounded record codec, checksums, append/sync, and replay;
- `scheduler`: bounded scoped workers, cancellation, deterministic collection,
  memory permits, and artifact-byte permits;
- `edit_bridge`: consumes the edit API re-exported by
  `weavatrix-refactor-plan` for legacy plans and prepared output;
- `transaction` and `operation`: prepare, stage, commit, rollback, and the v1/v3
  recovery state machines;
- `facade`: the small public `Worktree` and `PreparedTransaction` API.

The package has no direct `weavatrix-edit` dependency: the ownership chain is
`weavatrix-edit <- weavatrix-refactor-plan <- weavatrix-worktree`. Filesystem,
journal, and scheduler code do not validate operation contracts. Journal code
must not call transaction orchestration.
The edit bridge must not perform filesystem I/O. Production runtime modules may
not import tests, benchmarks, or tools.

All Rust modules are limited to 300 lines and all functions to 100 lines.
Split state transitions, platform behavior, codecs, and fault handling instead
of adding exceptions to these limits. Runtime code forbids unsafe Rust and does
not add Tokio or another async runtime; bounded concurrency uses scoped standard
threads.

## Public lifecycle

The intended public sequence is:

```text
Worktree::open(root)
    -> dry_run(plan)                     optional, read-only snapshot
    -> prepare(plan)                     lock + validate + stage + journal
    -> PreparedTransaction::commit()     deterministic commit
    -> ApplyReport

Worktree::open(root)
    -> dry_run_plan(worktree_plan)        optional, read-only path projection
    -> prepare_plan(refactor_plan)        validate + lock + stage + V3 journal
    -> PreparedWorktreeTransaction::commit()
    -> WorktreeApplyReport
```

`Worktree::apply` is a convenience for `prepare` followed by `commit`.
`Worktree::apply_plan` is the corresponding convenience for resource
operations. `Worktree::apply_plan_retained` runs the same prepared transaction
through `PreparedWorktreeTransaction::commit_retained`, which additionally
stores a durable undo receipt; `undo_receipts`, `undo_usage`, `rollback_undo`,
and `discard_undo` operate on that retained store under the same root lock.
`PreparedTransaction::abort` removes verified artifacts without changing a
target. Dropping a prepared transaction may attempt best-effort cleanup, but a
drop error is represented by the remaining journal and handled by `recover()`.

`dry_run` performs the same plan, path, source, hash, edit, and budget checks but
does not create the state directory, lock, journal, backup, or stage files. It
is a snapshot and must be revalidated by a later `prepare`.

## Bounded parallel preparation

Only the non-mutating and staging phase is parallel:

1. validate the entire submitted plan and compute stable portable-path order;
2. acquire the exclusive worktree lock and reject pending recovery;
3. write a journal header and deterministic logical-operation records, followed
   by one path intent per unique path and deterministic artifact name;
4. run bounded workers that read, hash, prepare, back up, and stage individual
   files after aggregate source and artifact budgets have been reserved;
5. join every worker, sort results by the preassigned index, and select the
   lowest-index error if more than one worker failed;
6. synchronize every artifact and distinct parent directory before recording
   the transaction as prepared.

The default worker count is
`min(available_parallelism, 4, file_count)`, with a hard maximum of 16.
Preflight metadata and projected sizes enforce the configured aggregate byte
ceilings before staging. Cancellation stops new admissions after an error, but
all started workers are joined and all known artifacts are accounted for.

Commit and rollback are intentionally serial. Parallel renames provide little
benefit for 5-10 files while making recovery order and failure reporting
nondeterministic.

## Default resource ceilings

| Resource | Default |
| --- | ---: |
| Logical operations / touched paths | 64 / 64 |
| Edits per file | 2,000 |
| Source bytes per file | 16 MiB |
| Output bytes per file | 64 MiB |
| Total source bytes | 128 MiB |
| Total staged output bytes | 256 MiB |
| Total backup plus stage bytes | 384 MiB |
| Journal bytes | 1 MiB |
| Operation label | 4,096 UTF-8 bytes |
| Legacy `EditPlan` completeness value | 4,096 UTF-8 bytes |
| Extension JSON serialized bytes, aggregate per plan | 256 KiB |
| Extension JSON value nodes, aggregate per plan | 4,096 |
| Extension JSON nesting depth | 32 |
| Evidence entries / text bytes | 10,000 / 8 MiB |
| Evidence status/code bytes | 256 |
| Worker threads | 4 automatic, 16 hard maximum |

All additions and conversions use checked arithmetic. Callers may lower these
ceilings. Raising them does not disable the hard worker and journal bounds.
The extension byte and node ceilings are shared across the top-level plan and
every nested file-, operation-, and text-edit-level extension map. They are not
per-map allowances; depth is checked for each extension JSON value. Metadata
admission runs before target traversal and before any new journal, stage/backup
artifact, or target mutation is created, so an over-budget plan fails closed
with `INVALID_PLAN` without leaving transaction state behind.

## Filesystem artifact layout

The control directory is `.weavatrix/worktree`. It contains the exclusive lock
and at most one active journal (`active.jsonl` for existing-file plans,
`active-v3.jsonl` for new refactor-plan operations, or `active-undo.jsonl` for
an in-flight undo rollback). A pre-existing `active-v2.jsonl` is accepted only
for recovery. Retained undo receipts are stored as `undo-<transaction>.json`
beside the journals. Stage and backup files are adjacent to their target and
use a random 128-bit transaction identifier plus the stable file index.
Artifact creation is exclusive and never follows an existing link.

Artifacts carry enough evidence for recovery:

- repository-relative target path;
- old and new SHA-256;
- bytes before and after;
- original portable permissions;
- backup and stage names;
- target and parent filesystem identities.

Stage output is written through `PreparedEdits::write_to` and an incremental
SHA-256 writer. Backup and stage files are synchronized before any target
replacement. Portable permission bits are applied to the stage before it is
synchronized. ACLs, extended attributes, alternate data streams, sparse-file
layout, and ownership outside the invoking user's authority are not promised by
v0.2 and must be documented as metadata boundaries. Create uses writable
`0o644` by default and `0o755` only when explicitly marked executable; modify
and rename preserve their source permissions.

## Journal format

The append-only, size-bounded journals use
`weavatrix.worktree-journal.v1` for `EditPlan` and
`weavatrix.worktree-journal.v3` for canonical `RefactorPlan` execution. The V3
header stores the `weavatrix-refactor-plan` fingerprint produced by the same
successful executor validation used for projection; `createdAt` therefore does
not alter transaction identity. Each newline-terminated record contains:

- monotonically increasing `seq`;
- transaction and root identities;
- a typed payload;
- SHA-256 of the deterministic payload bytes.

Record types are:

```text
Header
PreparedFile
Prepared
CommitIntent
Committed
RollbackIntent
RolledBack
Finished
```

V3 replaces `PreparedFile` with deterministic `Operation`, `PathIntent`, and
`PathStaged` records. A `PathIntent` records complete before/after state plus
optional adjacent stage/backup names; `PathStaged` binds those artifacts to
their filesystem identities. Intents for all paths are persisted before
parallel staging, then staged records are appended in stable path order.

New transactions write only V3. Recovery still recognizes an existing
`active-v2.jsonl`, verifies and extends it with the V2 checksum domain, and
removes it only after recovery completes. Its buffered legacy contract hash is
retained as opaque historical evidence and is never reinterpreted as the V3
canonical fingerprint. The undo journal reuses the V3 record framing: its
header binds the fresh rollback transaction identifier, the undo receipt
identifier, and the receipt checksum, followed by `RollbackIntent`,
`RolledBack`, and `Finished` records. Unknown versions, mixed V2/V3 files,
simultaneous V2 and V3 journals, or any combination of edit, operation, and
undo journals active together fail closed.

The header is synchronized before workers create artifacts. A commit or
rollback intent is synchronized before its replace. The completion record is
synchronized after the replace and parent-directory durability barrier. A torn
final record may be ignored; a corrupt complete record, sequence gap, unknown
schema, unexpected transition, or oversized journal fails closed.

## Journal state machine

```text
NEW
  -> STAGING
      -> PREPARED
          -> COMMITTING
              -> COMMITTED
                  -> CLEAN
              -> ROLLING_BACK
                  -> ROLLED_BACK
                      -> CLEAN
                  -> RECOVERY_REQUIRED
      -> ABORTED
          -> CLEAN
```

`CommitIntent(i)` followed by no `Committed(i)` is an ambiguous crash point.
Recovery resolves it from hashes, not timestamps:

| Path state | Recovery action |
| --- | --- |
| Matches complete before evidence | Treat path as not committed |
| Matches complete after evidence and backup matches before | Restore backup or recorded absence |
| Absent-to-present stage and target are the exact two-link intermediate | Verify full staged evidence, then finish or reverse safely |
| Matches neither state | Stop with `RECOVERY_REQUIRED` |
| Missing or wrong backup when needed | Stop with `RECOVERY_REQUIRED` |

Default recovery rolls back; it never rolls an incomplete transaction forward.
If `Finished(COMMITTED)` exists, recovery verifies the new hashes and finishes
cleanup. Cleanup is confined to generated transaction names and removes only
nofollow, regular, single-link artifacts on the root filesystem. It never
recursively deletes an unresolved path and fails closed on unsafe metadata.

## Retained undo receipts

`commit_retained` keeps every backup artifact and writes a
`weavatrix.worktree-undo.v1` receipt: a checksummed record of each path's
complete before and after slot evidence plus its retained artifact evidence,
fingerprinted over the ordered path set. The receipt is written after
commit-time revalidation and before the first target mutation, so it always
exists before the final `Finished(COMMITTED)` record. While the operation
journal is active the receipt is transitional and never observable through the
API: recovery keeps it only when the journal finished as committed and removes
it for every other outcome, and an in-process commit failure removes it before
the backups are consumed by rollback. A crash anywhere between the receipt
write and journal removal therefore resolves deterministically.

`rollback_undo` is an exact compare-and-swap. Before any mutation it verifies
every path against the receipt's complete after evidence (hash, bytes,
portable permissions, and file identity) and every retained artifact against
its recorded evidence; any divergence fails with `UNDO_CONFLICT` while the
tree is untouched. A later commit or rollback that touches a receipt path
changes that path's identity and deliberately invalidates the older receipt.
The restore itself mirrors commit rollback: paths are restored in reverse
index order behind durable `active-undo.jsonl` intents, present before-states
are reinstalled from their consumed backup (verified with the backup artifact
identity), recorded absences are removed exactly, and success deletes the
receipt and then the journal. `discard_undo` verifies each retained artifact
before removing it and the receipt without touching any target.

Undo recovery is idempotent completion: an undo journal without a durable
rollback intent is discarded and the receipt survives; one with any intent is
converged until every path matches its exact before evidence, including the
two-link backup crash intermediate, after which the receipt and journal are
consumed. A missing or mismatched receipt under an unfinished undo journal,
and any foreign path state, fail closed. The store is bounded by
`max_undo_receipts` and `max_total_undo_bytes` (32 receipts / 384 MiB by
default, hard ceilings 1,024 / 2 GiB) plus the caller's `UndoRetention`
policy; exhaustion fails the retained commit closed with `UNDO_STORE_FULL`
before any target changes.

## Failure reporting

Errors expose a stable code, transaction phase, optional portable path,
original plan index, transaction identifier, and whether recovery is required.
Primary categories cover invalid roots/plans, busy roots, reserved paths,
symlinks/reparse points, cross-filesystem targets, aliases/hardlinks, source or
transaction limits, non-UTF-8 data, hash mismatch, concurrent modification,
edit rejection, stage/durability/commit/rollback failures, corrupt journals,
retained-undo conflicts, capacity, and corruption (`UNDO_*`), and worker
panics.

A commit error that is fully rolled back reports the commit error and a
`RolledBack` outcome. If any rollback step cannot be proven safe, the journal
and all still-verifiable artifacts remain and the error reports
`RecoveryRequired`.

## Verification requirements

Acceptance requires:

- exact 1-, 5-, and 10-file success cases, including Unicode edits;
- byte-identical reports for 1, 2, 4, and 8 preparation workers;
- zero target mutations for plan, hash, `before`, encoding, and budget errors;
- path-component symlink and Windows reparse/junction adversarial tests;
- alias, hardlink, read-only, special-file, and cross-filesystem rejection;
- injected read, write, sync, rename, commit, and rollback failures;
- subprocess termination at every journal/action boundary followed by recovery;
- torn-final-record, corrupt-record, and journal-size tests;
- concurrent target modification before each commit index;
- permission preservation and artifact cleanup checks;
- property tests for journal transitions and checked budget arithmetic;
- benchmarks that separate validate, read/hash/prepare, stage/sync, commit, and
  rollback/recovery phases.

Benchmark parity includes complete file contents, paths, permissions, and
reports before timing. Results report median and tail latency plus bounded
workers, process memory, and artifact bytes. Durable apply and read-only dry-run
are not compared as equivalent operations.
