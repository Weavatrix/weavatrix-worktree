# Benchmark evidence — 2026-08-02

This is a reproducible single-machine snapshot, not a universal ranking.
The measured source commit is
[`c8e527a`](https://github.com/sergii-ziborov/weavatrix-worktree/commit/c8e527accd1be09c3e48455d57f36b3cb770cbf6).

The complete config, machine metadata, CSV, JSONL, and summaries are attached
to the public
[`benchmarks-2026-08-02` evidence release](https://github.com/sergii-ziborov/weavatrix-worktree/releases/tag/benchmarks-2026-08-02).
The ZIP is 1,969,128 bytes and its SHA-256 is
`c65f8de639eecde341340624502bbebf354f97dc97a3bda0e270f7ba00330b53`.
The release was downloaded again after upload and matched that digest.

## Recorded environment and workload

- Windows 11 Enterprise `10.0.26200`, x64;
- Intel Core Ultra 7 255U, 14 logical CPUs, 50,992,144,384 bytes RAM;
- NTFS, Balanced power plan;
- Node.js 24.15.0, Rust 1.97.1, system Git CLI 2.54.0.windows.1;
- fresh 64 KiB UTF-8 files with three exact same-length edits per file;
- 1, 5, 10, and 64 files; five warmups and 30 recorded repetitions;
- subprocess end-to-end latency, with fixture reset and independent hash/tree
  verification outside the timed interval;
- deterministic matrix shuffle seed `20260802`.

The Weavatrix and atomwrite matrices requested 1, 2, 4, and 8 workers. The
system Git CLI has no worker control, so its worker axis is one explicit `null`
configuration.

The row arithmetic is:

- Weavatrix and atomwrite: 32 configurations × (5 warmups + 30 recorded) =
  1,120 total rows, of which 960 are recorded;
- system Git CLI: 8 configurations × (5 warmups + 30 recorded) = 280 total
  rows, of which 240 are recorded;
- atomwrite transaction mode is half its matrix: 16 configurations × 35 runs =
  560 total transaction rows: 80 warmups plus 480 recorded rows.

## Correctness and contract gates

| Adapter | Total rows | Recorded rows | Passing result classes | Non-publishable result classes |
| --- | ---: | ---: | --- | --- |
| weavatrix-worktree 0.1.0 | 1,120 | 960 | 16 dry-run and 16 recoverable-durable configurations | none |
| atomwrite 0.1.35 | 1,120 | 960 | 16 dry-run configurations | all 16 transaction/durable configurations |
| system Git CLI (`git apply`) 2.54.0 | 280 | 240 | 4 dry-run and 4 non-durable-apply configurations | none |

All 560 atomwrite transaction executions — 80 warmups and 480 recorded rows —
exited successfully and produced every expected content hash. All 560 also
left timestamped `.bak` files, failed the artifact-cleanup gate, and emitted a
Windows `fsync_file best-effort` warning. They therefore have no publishable
latency statistics and are not treated as equivalent to the Weavatrix
recoverable-durable contract.

The system Git CLI passed every content and cleanup gate for its declared modes,
but documents no worktree fsync, journal, restart recovery, full-file CAS, or
worker control. Its apply results remain in the separate non-durable class.
This adapter is unrelated to [`weavatrix-git`](https://github.com/sergii-ziborov/weavatrix-git),
which is a read-only Git intelligence crate and deliberately provides no
checkout, patch application, or repository mutation.

## Dry-run latency

This table selects the lowest recorded p50 among each adapter's declared worker
settings for each file count. The guard semantics still differ: Weavatrix uses
whole-file SHA-256 plus exact edits, atomwrite batch uses exact replacement
patterns, and the system Git CLI uses unified-diff context.

| Files | Weavatrix workers | Weavatrix p50 / p95 ms | atomwrite workers | atomwrite p50 / p95 ms | System Git p50 / p95 ms | Weavatrix p50 advantage vs atomwrite / system Git |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 8 | 27.16 / 31.84 | 2 | 141.98 / 175.92 | 134.59 / 172.87 | 5.23× / 4.96× |
| 5 | 2 | 27.94 / 32.98 | 2 | 139.08 / 190.92 | 144.08 / 165.87 | 4.98× / 5.16× |
| 10 | 4 | 30.78 / 35.74 | 8 | 147.90 / 179.29 | 134.30 / 160.65 | 4.81× / 4.36× |
| 64 | 8 | 41.48 / 52.31 | 1 | 195.56 / 220.25 | 173.56 / 224.14 | 4.71× / 4.18× |

## Weavatrix 64-file worker sweep

Only Weavatrix passed the recoverable-durable gates. Its 64-file durable p50
improved 2.96× from one to eight workers; commit order and reports remained
deterministic.

| Workers | Dry-run p50 / p95 ms | Recoverable-durable p50 / p95 ms |
| ---: | ---: | ---: |
| 1 | 54.30 / 65.85 | 1,621.54 / 1,841.45 |
| 2 | 48.05 / 63.11 | 978.95 / 1,067.00 |
| 4 | 42.68 / 59.91 | 624.24 / 692.89 |
| 8 | 41.48 / 52.31 | 547.00 / 619.32 |

For context only, the system Git CLI's separate non-durable 64-file apply
result was 213.10 ms p50 and 270.52 ms p95. It must not be compared as if it
provided the same durability or recovery contract.

See the runnable [benchmark harness](../tools/benchmarks/README.md) for exact
commands, adapter contracts, correctness gates, and reproduction instructions.
