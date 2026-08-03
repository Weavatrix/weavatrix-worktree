# Benchmark adapter protocol

An adapter is invoked as:

```text
ADAPTER --workspace ROOT --manifest MANIFEST.json \
        --mode dry-run|durable-apply|non-durable-apply [--workers N] \
        --timeout-ms N
```

It writes exactly one JSON object to stdout and diagnostics to stderr. A
successful object contains at least:

```json
{
  "schema": "weavatrix.worktree-benchmark-adapter.v2",
  "ok": true,
  "adapter": "adapter-name",
  "mode": "dry-run",
  "workload": "rename",
  "operations": 5,
  "touched_paths": 10,
  "workers_requested": 4,
  "workers_effective": 4,
  "durability_contract": "adapter-specific exact wording",
  "equivalent_to_weavatrix_recoverable_batch": true
}
```

The publishable tool adapters all use the same outer Node wrapper plus one
target-process shape. Exact target versions are probed once before the matrix
and passed into wrappers; no adapter may start a separate version process inside
a measured sample. The child timeout is slightly shorter than the outer timeout
so the wrapper can emit a deterministic failure without leaving a grandchild.

On failure, the adapter emits the same schema with `ok: false`, an `error`
string, and exits non-zero. The harness does not trust the adapter's correctness
claim: it independently verifies every expected before/after file-or-missing
state, byte count, SHA-256, unrelated control file, and unexpected artifact.

Adapters without worker control report both worker fields as `null`, and the
harness creates one configuration for that axis. `non-durable-apply` is a
separate result class and is never recoverable-batch equivalent.

## Manifest schema

`weavatrix.worktree-benchmark-manifest.v2` contains explicit logical
operations. Representative entries are:

```json
{
  "schema": "weavatrix.worktree-benchmark-manifest.v2",
  "fixture_generator": "weavatrix-fixed-markers-v2",
  "fixture_seed": null,
  "operation": "benchmark_mixed",
  "workload": "mixed",
  "operation_count": 10,
  "touched_path_count": 10,
  "file_bytes": 65536,
  "operations": [
    {
      "type": "modify",
      "path": "modify/file-0000.rs",
      "source_sha256": "hex source digest",
      "output_sha256": "hex output digest",
      "bytes_before": 65536,
      "bytes_after": 65536,
      "edits": [
        {
          "start": { "line": 52, "character": 64 },
          "end": { "line": 52, "character": 96 },
          "expected": "unique source marker",
          "replacement": "unique output marker"
        }
      ]
    },
    {
      "type": "create",
      "path": "create/file-0006.rs",
      "content": "exact UTF-8 contents",
      "output_sha256": "hex output digest",
      "bytes_after": 65536
    },
    {
      "type": "delete",
      "path": "delete/file-0008.rs",
      "source_sha256": "hex source digest",
      "bytes_before": 65536
    },
    {
      "type": "rename",
      "source": "rename-source/file-0000.rs",
      "target": "rename-target/file-0000.rs",
      "source_sha256": "hex source digest",
      "bytes_before": 65536
    }
  ]
}
```

One rename is one logical operation and two touched paths. The fixed mixed
workload is six modifies, two creates, and two deletes. Homogeneous rename
latency scenarios use independent pairs; chains and cycles are classified and
tested as correctness cases, not mixed into performance headlines.

Line numbers are one-based and characters are zero-based. Fixtures are ASCII,
so byte, scalar, and UTF-16 columns coincide; Unicode-position correctness
belongs to the product suite.

For `git-apply`, the harness also writes `changes.patch` beside the manifest,
outside the scanned workspace and before timing. It uses ordinary modify,
new-file, deleted-file, and 100%-similarity rename patch records. Expected tree
states remain authoritative.
