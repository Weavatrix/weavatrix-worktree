# Benchmark adapter protocol

An adapter is an executable invoked as:

```text
ADAPTER --workspace ROOT --manifest MANIFEST.json \
        --mode dry-run|durable-apply|non-durable-apply [--workers N]
```

It must write exactly one JSON object to stdout and diagnostics to stderr. A
successful object contains at least:

```json
{
  "schema": "weavatrix.worktree-benchmark-adapter.v1",
  "ok": true,
  "adapter": "adapter-name",
  "mode": "dry-run",
  "files": 5,
  "workers_requested": 4,
  "workers_effective": 4,
  "durability_contract": "adapter-specific exact wording",
  "equivalent_to_weavatrix_recoverable_batch": true
}
```

On failure, the adapter should emit the same schema with `ok: false`, an
`error` string, and exit non-zero. The harness does not trust an adapter's
correctness claim: it independently hashes the fixture and scans artifacts.

Adapters without worker control report both worker fields as `null`, and the
harness creates one configuration for that axis. `non-durable-apply` is a
separate result class and is never recoverable-batch equivalent.

## Manifest schema

The harness writes a JSON object with this shape:

```json
{
  "schema": "weavatrix.worktree-benchmark-manifest.v1",
  "operation": "benchmark_exact_replace",
  "file_count": 5,
  "file_bytes": 65536,
  "files": [
    {
      "path": "src/file-0000.rs",
      "sha256": "hex source digest",
      "expected_sha256": "hex output digest",
      "bytes_before": 65536,
      "bytes_after": 65536,
      "edits": [
        {
          "start": { "line": 52, "character": 64 },
          "end": { "line": 52, "character": 96 },
          "expected": "unique 32-byte source marker",
          "replacement": "unique 32-byte output marker"
        }
      ]
    }
  ]
}
```

Line numbers are one-based and characters are zero-based, matching the public
`weavatrix-edit::Position` API used by the Rust adapter. Core fixtures are ASCII,
so byte, Unicode scalar, and UTF-16 columns coincide; separate Unicode-position
correctness tests belong in the product suite rather than this I/O benchmark.

For `git-apply`, the harness also writes `changes.patch` beside the manifest,
outside the scanned workspace and before the subprocess timer starts. The Git
adapter receives it with `--patch`; expected output hashes remain authoritative.
