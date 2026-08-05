# Runnable worktree benchmark harness

This harness executes correctness-gated subprocess measurements for explicit
modify, create, delete, rename, and mixed UTF-8 worktree plans. It is
intentionally outside the product crate and is excluded from the published
package.

The default matrix is:

- 1, 5, 10, and 64 homogeneous logical operations;
- modify, create, delete, and independent rename workloads;
- one fixed mixed workload with six modifications, two creations, and two deletions;
- three exact same-length replacements per modified file;
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

The adapter preserves every default byte, journal, artifact, and worker limit.
It raises only `WorktreeLimits.max_files` to
`max(default.max_files, manifest.touched_path_count)`, and records that value as
both `effective_max_files` and `effective_max_paths`. This is required for the
64-rename workload: 64 logical operations intentionally touch 128 paths.

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
cargo install atomwrite --version 0.1.36 --locked `
  --root tools/benchmarks/.tools/atomwrite
```

Use `--atomwrite-bin PATH` when testing another explicitly recorded binary.

## Full Weavatrix matrix

Run this only after the product API and its correctness suite are ready:

```powershell
node tools/benchmarks/run.mjs run `
  --adapter weavatrix `
  --workloads modify,create,delete,rename,mixed `
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
  --workloads modify,create,delete,rename,mixed `
  --counts 1,10 `
  --workers 1,2 `
  --modes dry-run,durable-apply `
  --file-bytes 4096 `
  --warmups 1 `
  --repetitions 2
```

## Interleaved competitor run

Use the comparison controller for any cross-tool table. It randomizes the order
of Weavatrix, atomwrite, and Git inside each warmup/measured round and preserves
the order plus every component run under `rounds/`:

```powershell
node tools/benchmarks/compare.mjs `
  --comparison-profile publication `
  --adapters weavatrix,atomwrite,git-apply `
  --workloads modify,create,delete,rename,mixed `
  --counts 1,5,10,64 `
  --workers 1,2,4,8 `
  --file-bytes 65536 `
  --warmups 5 `
  --repetitions 30 `
  --output tools/benchmarks/results/interleaved-full
```

The default `publication` profile runs both Weavatrix modes, atomwrite dry-run,
and both honest Git modes. Atomwrite durable/transaction rows are excluded from
that profile because modify/delete/move leave `.bak` artifacts and fail the
cleanup gate.
The combined summary computes the conservative `2x` ratio only when both rows
are publication-quality and have identical track, guard/durability contract,
workload, path count, file size, and effective workers. A faster adjacent
baseline with a weaker contract produces no eligible `2x` row.

It also emits a separately named `stronger_contract_performance_floor` for the
predeclared weaker baselines. That diagnostic uses the same adverse p25/p75
ratio but always records `equivalent_contracts = false` and
`universal_ranking = false`; it does not turn Git or atomwrite into a contract
peer.

Run the durable competitor behavior as a separate interleaved audit:

```powershell
node tools/benchmarks/compare.mjs `
  --comparison-profile atomwrite-durable-audit `
  --adapters weavatrix,atomwrite `
  --workloads modify,create,delete,rename,mixed `
  --counts 1,5,10,64 `
  --workers 1,2,4,8 `
  --file-bytes 65536 `
  --warmups 5 `
  --repetitions 30 `
  --output tools/benchmarks/results/atomwrite-durable-audit
```

This profile invokes atomwrite's documented strongest batch knobs without any
wrapper cleanup: transaction mode, `--no-backup`, per-operation `backup:false`,
and the default `keep-backup:false`. In atomwrite 0.1.36, transaction rollback
snapshots for pre-existing paths are still retained after success. The audit
records those `.bak` paths and the passing tree-state gate, forces every
atomwrite durable configuration to `publishable = false`, and never turns its
diagnostic latency into a release claim. An exit status of 2 is expected when
the artifact-cleanup gate reproduces the defect.

## `atomwrite` run

```powershell
node tools/benchmarks/run.mjs run `
  --adapter atomwrite `
  --workloads modify,create,delete,rename,mixed `
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

The adapter maps the manifest to exact `replace`, `write`, `delete`, and `move`
operations and invokes `batch --transaction`. `atomwrite` 0.1.36 batch manifests
do not carry per-file expected checksums, and its transaction is backup plus
compensating rollback rather than the Weavatrix durable journal/recovery
contract. Every raw row therefore records:

```text
equivalent_to_weavatrix_recoverable_batch = false
```

The timings are useful as adjacent evidence, but they are not placed in the same
durability-equivalent ranking. Leftover transaction backups also fail the
artifact-cleanup gate rather than being silently removed outside the timed tool
operation. There is no publishable atomwrite durable latency class in this
benchmark; use the separate interleaved durable-audit profile above for raw
diagnostic evidence.

## System Git CLI `git apply` non-durable baseline

`git apply` is a whole-patch applicability baseline, not a durable transaction:

```powershell
node tools/benchmarks/run.mjs run `
  --adapter git-apply `
  --workloads modify,create,delete,rename,mixed `
  --counts 1,5,10,64 `
  --modes dry-run,non-durable-apply `
  --file-bytes 65536 `
  --warmups 5 `
  --repetitions 30
```

The harness generates modify, new-file, deleted-file, and extended rename
patches from the same source and expected tree before timing. It runs
worktree-only `git apply --no-index`; dry-run adds `--check`. It never passes
`--reject`, `--3way`, `--index`, or `--unsafe-paths`.

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
- `summary.json`: per-configuration p25/p75 when there are at least four valid
  samples, p50, p95 and a deterministic 2,000-resample bootstrap 95% confidence
  interval for the median when there are at least 20, MAD, raw bounds, and
  publication-profile status. Functional smoke runs are always non-publishable.

Interleaved comparison directories additionally contain `schedule.json`, every
component run under `rounds/`, and `report.md`, which renders the complete
p50/p95 configuration table plus strictly separate equal-contract and
stronger-contract gate sections from the machine-readable summary. Adapter
versions and machine/filesystem metadata are captured once before the schedule,
written to the comparison root, and reused verbatim by component runs; version
probes and metadata commands never become per-round drift or timed work. The
snapshot includes SHA-256 for every resolved target binary and Node wrapper, so
a version string alone cannot hide a changed executable.

Each JSONL row retains the independently generated expected before/after tree,
including file/missing state, byte count, and SHA-256. Publication-quality
cross-tool reports must interleave tool order and use competitor p25 versus
Weavatrix p75 for the conservative two-times gate; separate whole-tool runs are
not eligible for that claim.

Fixture reset, manifest creation, expected-tree calculation, hashing, and
correctness checks are outside the timed interval. The subprocess interval
uses the same one-Node-wrapper plus one-target-process shape for Weavatrix,
atomwrite, and Git, and includes wrapper startup, plan decoding/translation,
target startup, product execution, and result encoding. Tool versions are probed
once before the matrix, never by a second process inside an individual sample.
Do not mix these subprocess numbers with in-process library benchmarks.

## Correctness gates

Every sample records independent gates:

- adapter exit status and valid adapter JSON;
- exact logical-operation/touched-path counts and a Weavatrix effective path
  budget that covers every touched path;
- dry-run leaves the entire expected before-tree unchanged;
- apply produces the exact per-path file-or-missing after-state and SHA-256;
- create/delete/rename absence preconditions and outcomes are checked independently;
- one unrelated control file remains byte-identical;
- no unaccounted regular file remains in the fixture;
- no stage, backup, journal, or temporary artifact remains, except an adapter's
  explicitly declared stable control file;
- result rows report the exact logical-operation and touched-path counts.

For Weavatrix, `.weavatrix/worktree/lock` is an allowed persistent control file;
`active.jsonl`, adjacent stage/backup files, or any other extra file fail the
gate. The complete observed file list is retained in each JSONL row.

Only non-warmup samples for which every gate passes enter latency statistics.
Any failed recorded sample makes that configuration non-publishable.

## Useful options

```text
--workloads LIST        modify,create,delete,rename,mixed
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
Refresh is refused unless the recorded hostname, platform, architecture, and
original probe root match the current machine. It re-probes that original root,
not the current checkout, so copied historical results cannot acquire unrelated
filesystem metadata.
