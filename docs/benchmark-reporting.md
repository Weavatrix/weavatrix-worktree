# Benchmark reporting

This companion to [Benchmark methodology](benchmarks.md) defines resource gates,
table shapes, and the claims that may be made from recorded results.

## Resource bounds

- Enforce maximum files, per-file input/output bytes, total input/output bytes,
  open files, workers, and temporary disk bytes before runaway allocation.
- Cancellation leaves the same documented outcome classes as an I/O failure.
- A 64-file plan with one oversized target fails according to the plan's
  all-or-none preflight policy rather than silently skipping that target.

## Result tables

Keep library and CLI tables separate. A minimal latency table is:

| Tool/version | Track | Mode/tier | Files | Bytes/file | Layout | Workers | Samples | p50 ms | p95 ms | MAD ms | Median CI | Correctness gates |
| --- | --- | --- | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| _example only_ | library | atomic-file | 5 | 65536 | shared-parent | 4 | 30 | — | — | — | — | pass/fail |

The phase table is:

| Tool/version | Mode | Files | Plan/validate | Read/hash | Render | Stage write | Stage sync | Revalidate | Journal | Commit | Dir sync | Cleanup |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| _example only_ | recoverable-batch | 10 | — | — | — | — | — | — | — | — | — | — |

Publish the raw machine-readable samples, fixture seed, expected tree hashes,
exact configuration, and environment record alongside any summarized table.

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
