# Runnable worktree benchmark harness

This harness executes correctness-gated subprocess measurements for existing
UTF-8 files. It is intentionally outside the product crate and is excluded from
the published package.

The default matrix is:

- 1, 5, 10, and 64 files;
- three exact same-length replacements per file;
- 64 KiB per file;
- dry-run and durable apply as separate modes;
- 1, 2, 4, and 8 requested workers for adapters that expose worker control;
- five warmups and 30 recorded repetitions.

The harness writes raw JSONL and CSV samples, a machine metadata snapshot, the
resolved run configuration, and a correctness-gated summary. It does not print
or derive a universal speed ranking.

## Requirements

- Node.js 20 or newer. The harness uses only Node standard-library modules.
- Rust/Cargo to build the isolated Weavatrix adapter or locally install
  `atomwrite`.
- The system Git CLI for the worktree-only `git apply` baseline. This is not
  the read-only `weavatrix-git` crate.
- A local filesystem with enough free space for fresh fixtures on every sample.

## First run and self-check

From the repository root:

```powershell
node tools/benchmarks/run.mjs self-check
node --test tools/benchmarks/tests/harness.test.mjs
node tools/benchmarks/run.mjs detect
```

`self-check` uses a deliberately non-publishable reference adapter. It verifies
fixture generation, dry-run immutability, apply output hashes, artifact scanning,
JSONL/CSV output, and summaries. It is not a product or competitor benchmark.

## Build the Weavatrix adapter

The adapter is a separate Cargo package and is not registered in the product
workspace:

```powershell
cargo build --release --locked `
  --manifest-path tools/benchmarks/weavatrix-adapter/Cargo.toml `
  --target-dir tools/benchmarks/.tools/weavatrix-target

node tools/benchmarks/run.mjs detect
```

The default detected binary is:

```text
tools/benchmarks/.tools/weavatrix-target/release/weavatrix-worktree-bench-adapter.exe
```

On Unix, omit the `.exe` suffix. A different path can be supplied with
`--weavatrix-bin PATH`.

## Install and detect `atomwrite`

The harness pins the audited competitor version and installs it below
`tools/benchmarks/.tools`; this does not publish anything or change the product
Cargo manifest:

```powershell
node tools/benchmarks/run.mjs install-atomwrite
node tools/benchmarks/run.mjs detect
```

Equivalent explicit Cargo command:

```powershell
cargo install atomwrite --version 0.1.35 --locked `
  --root tools/benchmarks/.tools/atomwrite
```

Use `--atomwrite-bin PATH` when testing another explicitly recorded binary.

## Full Weavatrix matrix

Run this only after the product API and its correctness suite are ready:

```powershell
node tools/benchmarks/run.mjs run `
  --adapter weavatrix `
  --counts 1,5,10,64 `
  --workers 1,2,4,8 `
  --modes dry-run,durable-apply `
  --file-bytes 65536 `
  --warmups 5 `
  --repetitions 30
```

The result directory is printed at completion and defaults below
`tools/benchmarks/results/`.

For a short functional smoke before the full run:

```powershell
node tools/benchmarks/run.mjs run `
  --adapter weavatrix `
  --counts 1,5 `
  --workers 1,2 `
  --modes dry-run,durable-apply `
  --file-bytes 4096 `
  --warmups 1 `
  --repetitions 2
```

## `atomwrite` run

```powershell
node tools/benchmarks/run.mjs run `
  --adapter atomwrite `
  --counts 1,5,10,64 `
  --workers 1,2,4,8 `
  --modes dry-run,durable-apply `
  --file-bytes 65536 `
  --warmups 5 `
  --repetitions 30
```

`atomwrite batch` exposes `--threads` (alias `--max-concurrency`). The adapter
passes every requested worker count explicitly and records it as both requested
and effective. Transaction-visible operations may still be sequential, while
backup and other Rayon helpers use this bound; the benchmark does not infer
internal parallelism from the flag alone.

The adapter uses exact `replace` operations and `batch --transaction`.
`atomwrite` 0.1.35 batch manifests do not carry per-file expected checksums, and
its transaction is backup plus compensating rollback rather than the Weavatrix
durable journal/recovery contract. Every raw row therefore records:

```text
equivalent_to_weavatrix_recoverable_batch = false
```

The timings are useful as adjacent evidence, but they are not placed in the same
durability-equivalent ranking. Leftover transaction backups also fail the
artifact-cleanup gate rather than being silently removed outside the timed tool
operation.

## System Git CLI `git apply` non-durable baseline

`git apply` is a whole-patch applicability baseline, not a durable transaction:

```powershell
node tools/benchmarks/run.mjs run `
  --adapter git-apply `
  --counts 1,5,10,64 `
  --modes dry-run,non-durable-apply `
  --file-bytes 65536 `
  --warmups 5 `
  --repetitions 30
```

The harness generates the unified patch from the same source and expected tree
before timing. It runs worktree-only `git apply --no-index`; dry-run adds
`--check`. It never passes `--reject`, `--3way`, `--index`, or `--unsafe-paths`.

Git exposes no worker control, so the matrix contains one `null` worker value
and rejects `--workers`. Git documents no file/directory sync, journal, restart
recovery, or full-file hash CAS. These results always use
`non-durable-apply`, set `equivalent_to_weavatrix_recoverable_batch = false`,
and must not appear in a durable comparison table.

## Output files

Each run directory contains:

- `config.json`: requested and resolved matrix;
- `machine.json`: OS, CPU, memory, filesystem, runtime, Git, Rust, and adapter
  version metadata, without dumping environment variables or secrets;
- `samples.jsonl`: every warmup and recorded subprocess sample;
- `samples.csv`: the same stable scalar columns for external analysis;
- `summary.json`: per-configuration p50, p95 when there are at least 20 valid
  samples, MAD, confidence-free raw bounds, and publishability status.

Fixture reset, manifest creation, expected-tree calculation, hashing, and
correctness checks are outside the timed interval. The subprocess interval
includes adapter startup, plan decoding, product execution, and result encoding.
Do not mix these subprocess numbers with in-process library benchmarks.

## Correctness gates

Every sample records independent gates:

- adapter exit status and valid adapter JSON;
- dry-run leaves every source hash unchanged;
- durable apply produces every expected SHA-256;
- no expected file is missing;
- no unaccounted regular file remains in the fixture;
- no stage, backup, journal, or temporary artifact remains, except an adapter's
  explicitly declared stable control file;
- result rows are present for the exact requested file count.

For Weavatrix, `.weavatrix/worktree/lock` is an allowed persistent control file;
`active.jsonl`, adjacent stage/backup files, or any other extra file fail the
gate. The complete observed file list is retained in each JSONL row.

Only non-warmup samples for which every gate passes enter latency statistics.
Any failed recorded sample makes that configuration non-publishable.

## Useful options

```text
--timeout-ms N          per-adapter subprocess timeout (default 120000)
--seed N                deterministic scenario-order seed
--output DIR            explicit result directory
--weavatrix-bin PATH    explicit Weavatrix adapter binary
--atomwrite-bin PATH    explicit atomwrite binary
--git-bin PATH          explicit Git binary
```

Use `node tools/benchmarks/run.mjs help` for the complete current syntax.

On Windows, filesystem metadata combines Node's numeric `statfs` values with
`Get-Volume` so `detected_name` records values such as `NTFS`. If an older raw
run has only the numeric type, refresh that field without rerunning timings:

```powershell
node tools/benchmarks/run.mjs refresh-machine-filesystem --result PATH_TO_RUN
```

The refresh timestamp is recorded separately from the original capture time.
