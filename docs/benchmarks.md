# Benchmark methodology

This document defines reproducible benchmarks for `weavatrix-worktree` and
comparable tools. It intentionally does not contain a universal speed claim.
Results are valid only for the recorded machine, filesystem, versions, workload,
and safety/durability mode.

The competitor capabilities and limitations that determine fair comparison
modes are documented in [competitors.md](competitors.md).

## Questions the suite must answer

1. What is end-to-end latency for editing 1, 5, 10, and 64 files?
2. Which phases benefit from bounded parallel preparation?
3. What does file synchronization, directory synchronization, journaling, and
   rollback preparation cost separately?
4. Does every measured mode pass its advertised correctness and recovery gates?
5. At what file count and size does additional worker concurrency stop helping
   on the measured system?

The suite does not use benchmark throughput as evidence of atomicity,
durability, path safety, or crash recovery. Those properties have separate
gates below.

## Two non-interchangeable tracks

### Library track

Measure an already-running process from a fully constructed operation plan to
the returned report. Process startup, command-line parsing, and result encoding
are excluded. Compare only libraries through adapters with equivalent input and
output semantics.

Potential references:

- `weavatrix-worktree` library API;
- [`textum::PatchSet`](https://docs.rs/textum/0.4.0/src/textum/composer.rs.html)
  for non-durable patch/write behavior;
- [`libgit2::git_apply`](https://libgit2.org/docs/reference/main/apply/git_apply.html)
  for repository patch application;
- [`atomic-write-file`](https://docs.rs/atomic-write-file/0.3.0/atomic_write_file/)
  as a single-file atomic-replacement building-block baseline.

### CLI track

Measure subprocess wall-clock time from process creation through exit, including
manifest/diff parsing and machine-readable result emission. All tools must have
stdout and stderr captured in the same way.

Potential references:

- a future `weavatrix-worktree` CLI;
- [`atomwrite batch`](https://docs.rs/crate/atomwrite/0.1.35);
- the system Git CLI's [`git apply`](https://git-scm.com/docs/git-apply), not
  the separate read-only `weavatrix-git` project.

Do not compare a library-track number with a CLI-track number. `cargo fix` is not
a filesystem throughput peer because compiler execution and validation dominate
its workflow. LSP `WorkspaceEdit` is a protocol and delegates application to an
editor client, so it is also excluded from raw throughput rankings.

## Workload definitions

Every fixture is generated from a recorded seed and immutable template. The
expected output tree is generated independently of the implementation under
test.

### Core agent workload

| Axis | Required values |
| --- | --- |
| Files modified | `1`, `5`, `10`, `64` |
| Input size per file | `64 KiB` |
| Edits per file | Three unique exact markers near 10%, 50%, and 90% of the file |
| Encoding | Valid UTF-8 with LF line endings |
| Replacement size | Same byte length as the marker, to isolate scheduling and I/O from growth allocation |
| Directory layout | All files in one target directory |
| Initial state | Every existing target has an expected BLAKE3 or SHA-256 content hash |

Markers must include the file index and position so a search-based engine cannot
silently match the wrong occurrence. Every implementation receives semantically
equivalent edits and must produce byte-identical output.

### Size sweep

Repeat the core workload at `1 KiB`, `64 KiB`, and `1 MiB` per file. Keep three
edits per file and the same file-count set. Report input and output bytes rather
than deriving them from nominal fixture size.

### Directory durability sweep

Run both layouts:

- **shared-parent**: all destinations are in one directory;
- **many-parent**: every destination has its own parent directory.

This sweep is required for durable modes because syncing one parent directory is
not equivalent to syncing 64 distinct parent directories.

### Growth workload

As a separate result, replace the three markers with content that grows each
file by 1%, then by 25%. Do not mix growth results into the same headline table
as equal-length replacement.

### Mixed-operation workload

After create and delete operations are implemented, use a ten-path plan with six
modifications, two creations, and two deletions. Add rename only after its cycle,
collision, and rollback semantics are implemented. A competitor that does not
support an operation is marked unsupported rather than emulated with a different
contract.

## Safety and durability modes

Map actual implementation options to these conceptual tiers and record the exact
mapping in raw results.

| Tier | Required behavior |
| --- | --- |
| `render-only` | Read/validate/render in memory; no filesystem mutation. |
| `direct-write` | Visible writes without temp-file replacement, journal, or explicit sync. This is an unsafe baseline only. |
| `atomic-file` | Same-filesystem staging and per-file atomic replacement; no durability assertion unless synchronization is enabled. |
| `durable-file` | `atomic-file` plus successful staged-file sync and affected-parent-directory sync where supported. |
| `recoverable-batch` | Complete preflight, staged outputs, persistent transaction progress, deterministic commit, rollback/recovery metadata, and the `durable-file` sync policy. |

Cross-tool speed comparisons are permitted only within equivalent tiers. In
particular:

- `textum::write_to_files` belongs to `direct-write`; its source explicitly
  permits partial output on a write error.
- default `git apply` provides whole-patch applicability preflight, but that does
  not by itself establish `durable-file` or `recoverable-batch` behavior.
- `atomwrite batch` without `--transaction` and with `--transaction` are separate
  modes. Its audited transaction path uses backups and compensating rollback;
  label it accordingly rather than equating it with group atomic visibility.
- `atomic-write-file` is a per-file baseline and must not be labelled a
  multi-file transaction.

## Concurrency matrix

For `weavatrix-worktree`, run each core file count with worker limits `1`, `2`,
`4`, `8`, and `auto`. `auto` must report the resolved worker count. Never create
more active file jobs than the number of files.

Parallel work may include:

1. safe open and metadata inspection;
2. reading and hashing;
3. edit rendering;
4. temporary-file creation and writing;
5. synchronization of independent staged files.

Record visible commit separately. The default design keeps commit deterministic
and sequential; benchmark data must not silently change that policy. Competitors
whose worker count is not configurable run with their documented default, which
is recorded rather than presented as an equal-worker comparison.

The scaling table reports speedup only against the same implementation, workload,
and durability tier at one worker:

```text
speedup(N) = median_time(1 worker) / median_time(N workers)
efficiency(N) = speedup(N) / N
```

This is not a cross-product speed claim.

## Timed phases

End-to-end elapsed time is authoritative. Internal instrumentation additionally
records non-overlapping phases:

1. plan decode and normalization;
2. path graph and policy validation;
3. lock acquisition;
4. source read and expected-state hashing;
5. edit rendering;
6. staging-file write;
7. staging-file sync;
8. pre-commit source revalidation;
9. journal write and journal sync;
10. visible commit;
11. affected-directory sync;
12. journal completion and cleanup.

If a mode omits a phase, record zero with an explicit `not_enabled` flag rather
than silently removing the field. Phase instrumentation overhead must be measured
once and reported; it must not replace external end-to-end timing.

Fixture generation, fixture reset, correctness verification, and collection of
system metadata occur outside the timed interval.

## Execution protocol

1. Build every Rust target with locked dependencies and the same release profile.
   Record compiler version, target triple, enabled features, and build commit.
2. Pin exact competitor versions or commit IDs. Save `--version` output with raw
   results.
3. Use a local filesystem. Record filesystem type, volume/device, free space,
   encryption/compression, mount options where available, and whether the path is
   local, virtualized, or network-backed.
4. Record OS build, CPU model, physical/logical cores, RAM, power profile,
   available parallelism, and relevant antivirus/indexer state. Do not disable a
   user security control merely to improve a result; report its state.
5. Restore the fixture from the immutable template before every sample. Verify
   the input tree hash before starting the timer.
6. Randomize tool order within each repetition so thermal drift, storage cache,
   and background activity do not always favor one tool.
7. Use five untimed warmups followed by 30 measured samples by default. A
   predeclared slow profile may use 15 samples; if fewer than 20 samples are
   collected, do not publish a p95 value. Never change sample count after seeing
   which tool is ahead.
8. Report sample count, median, p95 when valid, median absolute deviation, and a
   bootstrap 95% confidence interval for the median. Preserve every raw sample.
9. Report files/s and MiB/s only as derived workload-specific metrics. Median
   latency remains the primary result for 1/5/10-file agent workloads.
10. Keep cold-cache experiments separate. A process restart is not an OS cache
    flush. If the platform cannot produce a documented cold-cache condition,
    label the result warm/unspecified instead of calling it cold.

## Correctness gates

No throughput result is publishable until the exact measured mode passes the
applicable gates.

### Successful application

- The complete output tree is byte-identical to the independent expected tree.
- The report contains one stable input-order result for every requested path.
- Source encodings and line endings are unchanged except where the plan requests
  a change.
- The implementation does not modify paths outside the workspace or protected
  metadata such as `.git`.
- Successful cleanup leaves no unaccounted temporary, backup, lock, or journal
  artifacts. Intentionally retained recovery material is reported explicitly.

### Stale input and concurrency

- Mutating one source after prepare but before commit causes expected-state
  rejection before unrelated visible changes.
- Two overlapping cooperative transactions cannot lose an update or deadlock.
- Disjoint transactions can make progress according to the documented lock
  scope.
- An external writer that ignores locks is detected by immediate pre-commit hash
  revalidation when it changes a target before that check.
- Tests explicitly document the remaining race window after revalidation; they
  do not claim a portable atomic compare-and-swap that the filesystem does not
  provide.

### Path adversaries

- Absolute paths, `..`, empty components where invalid, and protected repository
  paths are rejected.
- Final and intermediate symlinks are rejected by default.
- Windows junctions and other reparse points receive equivalent coverage.
- Case-folded, Unicode-normalized, and alternate-separator aliases cannot enter
  the same plan as independent targets on platforms where they collide.
- FIFO, socket, device, and directory targets are rejected as appropriate.
- Hard links follow the explicit policy; default rejection must not silently
  switch to an in-place write.
- A concurrent intermediate-directory-to-symlink swap cannot escape the root in
  the handle-relative implementation.

### Injected failures

Inject a deterministic failure at every phase and, for multi-file phases, before
and after each file index:

- validation/read/hash failure: no visible mutation;
- staging write/sync failure: no visible mutation;
- journal failure: no visible mutation;
- commit failure: original tree restored or an explicit `RecoveryRequired` /
  `RollbackIncomplete` result with enough persisted state to recover;
- rollback failure: never reported as a clean rollback;
- directory-sync and cleanup failure: surfaced distinctly from content commit.

Verify the tree, journal, backups, and report after each injected failure. Fault
injection is a correctness suite, not part of normal latency samples.

### Process interruption and recovery

Terminate a subprocess at every persistent transaction-state boundary and start
the recovery entry point in a fresh process. The final state must be one of the
documented outcomes, never an unexplained mixture.

A process-kill test validates journal/recovery logic; it does **not** simulate
power loss or prove that a storage device honored flushes. A stronger durability
claim needs a platform-specific filesystem/storage fault harness, or must be
limited to the synchronization calls the implementation successfully completed.

Resource-bound gates, report tables, and the permitted claim language are in
[Benchmark reporting](benchmark-reporting.md).
