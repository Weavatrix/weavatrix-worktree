# Benchmark reporting

This companion to [Benchmark methodology](benchmarks.md) defines resource gates,
table shapes, and the claims that may be made from recorded results.

## Resource bounds

- Enforce maximum files, per-file input/output bytes, total input/output bytes,
  open files, workers, and temporary disk bytes before runaway allocation.
- Cancellation leaves the same documented outcome classes as an I/O failure.
- A 64-file plan with one oversized target fails according to the plan's
  all-or-none preflight policy rather than silently skipping that target.
- The runnable harness caps logical operations at 64, file size at 1 MiB, and
  requested workers at 64 before allocating fixture buffers or spawning tools.

## Result tables

Keep library and CLI tables separate. A minimal latency table is:

| Tool/version | Track | Mode/tier | Workload | Operations / paths | Bytes/file | Layout | Workers | Samples | p50 ms | p95 ms | MAD ms | Correctness gates |
| --- | --- | --- | --- | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | --- |
| _example only_ | subprocess | recoverable-batch | rename | 5 / 10 | 65536 | shared-parent | 4 | 30 | — | — | — | pass/fail |

The phase table is:

| Tool/version | Mode | Files | Plan/validate | Read/hash | Render | Stage write | Stage sync | Revalidate | Journal | Commit | Dir sync | Cleanup |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| _example only_ | recoverable-batch | 10 | — | — | — | — | — | — | — | — | — | — |

Publish the raw machine-readable samples, fixture seed, expected tree hashes,
exact configuration, and environment record alongside any summarized table.

### Conservative two-times gate

A `2x` claim is publishable only for the same workload, track, guard contract,
durability tier, file size, layout, and operation count. Use the deliberately
adverse cross-quantile ratio:

```text
fastest_competitor_p25 / weavatrix_p75 >= 2.0
```

The gate must pass every row named by the claim, not only the geometric mean or
the largest workload. Git's non-durable apply class and atomwrite batch without
full-file CAS are adjacent baselines; neither can establish a two-times claim
for the recoverable full-CAS class.

The standard publication profile is five warmups and 30 recorded samples for
every row. Short CI/smoke runs are always labelled non-publishable, even when
all correctness gates pass. A cross-tool claim additionally requires an
interleaved schedule; separately completed whole-tool runs are not eligible.

### Stronger-contract performance floor

Keep this diagnostic separate from the strict equal-contract gate. It asks
whether Weavatrix retains a conservative speed floor even while doing more:

```text
weaker_competitor_p25 / stronger_weavatrix_p75 >= 2.0
```

Predeclared pairs are Weavatrix full-CAS dry-run versus atomwrite/Git dry-run,
and Weavatrix recoverable durable apply versus atomwrite durable apply or Git
non-durable apply. Track, workload, logical operations, touched paths, file size,
and compatible effective workers must match. Every row is explicitly
`equivalent_contracts = false` and `universal_ranking = false`; it can measure a
performance floor, but can never satisfy or replace the strict contract-peer
`2x` gate.

Atomwrite 0.1.36 durable rows are collected only in the separately named
`atomwrite-durable-audit` interleaved profile. The harness does not remove its
successful-transaction `.bak` snapshots. Any cleanup-gate failure suppresses
that configuration's latency statistics automatically, and publication policy
also marks every atomwrite durable configuration non-publishable even when an
operation such as create happens not to require a rollback snapshot. Therefore
the predeclared atomwrite durable pair remains visible in the schema but cannot
produce an eligible performance-floor row for this audited version.

## Claim policy

Permitted wording:

> On the recorded Windows/NTFS system, version X completed the 10-file, 64 KiB,
> atomic-file workload with a median of Y ms across N samples.

> Bounded preparation at four workers was Z times the one-worker median for the
> same build, fixture, and durability mode.

Not permitted:

- "fastest Rust editor" from one machine or one fixture;
- "atomic batch" when only per-file rename or compensating rollback is present;
- comparing a durable/recoverable mode with a direct-write competitor as if the
  safety contracts were equal;
- using a passing performance sample as evidence that fault injection, recovery,
  symlink safety, or source-staleness gates pass;
- omitting failed or outlier samples without a predeclared, auditable rule.
