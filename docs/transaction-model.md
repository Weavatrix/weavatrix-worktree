# Weavatrix Worktree transaction model

Status: normative architecture contract for `weavatrix-worktree` v0.1.

## Purpose and boundary

`weavatrix-worktree` is the filesystem transaction layer above
`weavatrix-edit`. The edit crate validates and prepares deterministic changes
for one immutable UTF-8 source. This crate resolves repository-relative paths,
verifies current file hashes, stages output, commits several files, and
recovers interrupted transactions.

Version 0.1 modifies existing regular UTF-8 files only. It does not create,
delete, or rename user files. It does not run Git, hooks, formatters, compilers,
tests, MCP, or network operations. Those concerns belong to callers such as
`weavatrix-refactor-rust`.

## Guarantee: recoverable, not observationally atomic

No portable filesystem primitive atomically replaces a set of unrelated
paths. Commit therefore replaces targets one at a time in deterministic path
order. Another process may briefly observe a mixture of old and new files.

The v0.1 guarantee is instead **crash-recoverable all-or-restored**:

- no target changes before every file is validated, backed up, staged, and
  durably recorded;
- a normal successful commit leaves every target at its new hash;
- a reported commit failure rolls already-committed targets back in reverse
  order;
- an interrupted process leaves a durable journal from which `recover()` can
  restore the old set;
- recovery never overwrites a target whose contents match neither the recorded
  old hash nor the recorded new hash.

The crate must not describe this contract as an observationally atomic
multi-file commit.

## Runtime invariants

1. Plan paths are portable, repository-relative, unique, and outside the
   reserved `.weavatrix/worktree` namespace.
2. Every target is an existing regular file beneath the opened root capability.
3. Root, parent components, and targets are not symlinks, junctions, or other
   reparse points.
4. Every target is on the root filesystem or volume.
5. Different plan paths cannot resolve to the same filesystem identity.
6. Hard-linked, read-only, non-UTF-8, oversized, and concurrently changed
   targets fail closed in v0.1.
7. The plan SHA-256 is checked before staging and checked again immediately
   before the target's replace operation.
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
best-effort there. Version 0.1 does not claim absolute power-loss durability on
every filesystem or storage device.

The advisory root lock serializes cooperating library users. An unrelated
writer can still race the immediate hash/identity revalidation and the target
rename because portable filesystems provide no atomic compare-and-replace.
This residual leaf race is outside the v0.1 guarantee and must not be described
as protection against a hostile same-root writer.

## Component boundaries

The runtime dependency direction is:

```text
contract --------------------------------------------------------------+
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

- `contract`: stable limits, options, reports, hashes, phases, and typed errors;
- `filesystem`: capability-rooted path traversal, identity, metadata,
  create-new artifacts, atomic single-file replace, and durability barriers;
- `journal`: bounded record codec, checksums, append/sync, and replay;
- `scheduler`: bounded scoped workers, cancellation, deterministic collection,
  memory permits, and artifact-byte permits;
- `edit_bridge`: the only runtime component that consumes `weavatrix-edit`
  plans and prepared output;
- `transaction`: prepare, stage, commit, rollback, and recovery state machine;
- `facade`: the small public `Worktree` and `PreparedTransaction` API.

Dependencies point inward only. Filesystem, journal, and scheduler code must not
depend on `weavatrix-edit`. Journal code must not call transaction orchestration.
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
```

`Worktree::apply` is a convenience for `prepare` followed by `commit`.
`PreparedTransaction::abort` removes verified artifacts without changing a
target. Dropping a prepared transaction may attempt best-effort cleanup, but a
drop error is represented by the remaining journal and handled by `recover()`.

`dry_run` performs the same plan, path, source, hash, edit, and budget checks but
does not create the state directory, lock, journal, backup, or stage files. It
is a snapshot and must be revalidated by a later `prepare`.

## Bounded parallel preparation

Only the non-mutating and staging phase is parallel:

1. validate the complete plan and compute stable portable-path order;
2. acquire the exclusive worktree lock and reject pending recovery;
3. write a journal header containing every target and deterministic artifact
   name;
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
| Files per transaction | 64 |
| Edits per file | 2,000 |
| Source bytes per file | 16 MiB |
| Output bytes per file | 64 MiB |
| Total source bytes | 128 MiB |
| Total staged output bytes | 256 MiB |
| Total backup plus stage bytes | 384 MiB |
| Journal bytes | 1 MiB |
| Worker threads | 4 automatic, 16 hard maximum |

All additions and conversions use checked arithmetic. Callers may lower these
ceilings. Raising them does not disable the hard worker and journal bounds.

## Filesystem artifact layout

The control directory is `.weavatrix/worktree`. It contains the exclusive lock
and at most one active transaction journal. Stage and backup files are adjacent
to their target and use a random 128-bit transaction identifier plus the stable
file index. Artifact creation is exclusive and never follows an existing link.

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
v0.1 and must be documented as metadata boundaries.

## Journal format

The append-only, size-bounded journal uses schema
`weavatrix.worktree-journal.v1`. Each newline-terminated record contains:

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

| Target state | Recovery action |
| --- | --- |
| Matches old hash | Treat file as not committed |
| Matches new hash and backup matches old | Restore backup atomically |
| Matches neither hash | Stop with `RECOVERY_REQUIRED` |
| Missing or wrong backup when needed | Stop with `RECOVERY_REQUIRED` |

Default recovery rolls back; it never rolls an incomplete transaction forward.
If `Finished(COMMITTED)` exists, recovery verifies the new hashes and finishes
cleanup. Cleanup is confined to generated transaction names and removes only
nofollow, regular, single-link artifacts on the root filesystem. It never
recursively deletes an unresolved path and fails closed on unsafe metadata.

## Failure reporting

Errors expose a stable code, transaction phase, optional portable path,
original plan index, transaction identifier, and whether recovery is required.
Primary categories cover invalid roots/plans, busy roots, reserved paths,
symlinks/reparse points, cross-filesystem targets, aliases/hardlinks, source or
transaction limits, non-UTF-8 data, hash mismatch, concurrent modification,
edit rejection, stage/durability/commit/rollback failures, corrupt journals,
and worker panics.

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
