#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import path from 'node:path';

const ADAPTER_SCHEMA = 'weavatrix.worktree-benchmark-adapter.v2';
const MANIFEST_SCHEMA = 'weavatrix.worktree-benchmark-manifest.v2';

function parseArgs(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 1) {
    const item = argv[index];
    if (item === '--version') {
      values.set('version', true);
      continue;
    }
    if (!item.startsWith('--')) throw new Error(`unexpected positional argument: ${item}`);
    const equals = item.indexOf('=');
    if (equals !== -1) {
      values.set(item.slice(2, equals), item.slice(equals + 1));
      continue;
    }
    const next = argv[index + 1];
    if (next === undefined || next.startsWith('--')) {
      throw new Error(`missing value for ${item}`);
    }
    values.set(item.slice(2), next);
    index += 1;
  }
  return values;
}

function required(values, name) {
  const value = values.get(name);
  if (typeof value !== 'string' || value.length === 0) throw new Error(`--${name} is required`);
  return value;
}

function parseNdjson(stdout) {
  const events = [];
  for (const line of stdout.split(/\r?\n/u)) {
    if (line.trim().length === 0) continue;
    try {
      events.push(JSON.parse(line));
    } catch (error) {
      throw new Error(`atomwrite emitted non-JSON stdout: ${line.slice(0, 200)}`, { cause: error });
    }
  }
  return events;
}

function atomwriteVersion(binary) {
  const result = spawnSync(binary, ['--version'], {
    encoding: 'utf8', windowsHide: true, timeout: 30_000,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`atomwrite --version exited ${String(result.status)}: ${result.stderr}`);
  }
  return result.stdout.trim();
}

function validatedVersion(values) {
  const version = required(values, 'atomwrite-version');
  if (!/^atomwrite 0\.1\.36(?:\s|$)/u.test(version)) {
    throw new Error(`atomwrite version mismatch: expected 0.1.36, found ${version}`);
  }
  return version;
}

function childTimeout(values) {
  const outer = Number(required(values, 'timeout-ms'));
  if (!Number.isSafeInteger(outer) || outer < 1) throw new Error('--timeout-ms is invalid');
  const grace = Math.min(1_000, Math.max(1, Math.floor(outer / 10)));
  return Math.max(1, outer - grace);
}

function validateManifest(manifest) {
  if (manifest.schema !== MANIFEST_SCHEMA) {
    throw new Error(`unsupported manifest schema: ${String(manifest.schema)}`);
  }
  if (!Array.isArray(manifest.operations)
    || manifest.operations.length !== Number(manifest.operation_count)
    || manifest.operations.length === 0) {
    throw new Error('manifest operation count mismatch');
  }
  if (!Number.isSafeInteger(manifest.touched_path_count) || manifest.touched_path_count < 1) {
    throw new Error('manifest touched_path_count is invalid');
  }
}

function atomwriteOperations(manifest) {
  const operations = [];
  for (const operation of manifest.operations) {
    if (operation.type === 'modify') {
      for (const edit of operation.edits) {
        operations.push({
          op: 'replace', path: operation.path,
          pattern: edit.expected, replacement: edit.replacement, backup: false,
        });
      }
    } else if (operation.type === 'create') {
      operations.push({ op: 'write', path: operation.path, content: operation.content, backup: false });
    } else if (operation.type === 'delete') {
      operations.push({ op: 'delete', path: operation.path, backup: false });
    } else if (operation.type === 'rename') {
      operations.push({
        op: 'move', source: operation.source, target: operation.target,
        force: false, backup: false,
      });
    } else {
      throw new Error(`unsupported manifest operation: ${String(operation.type)}`);
    }
  }
  return operations;
}

async function run(values) {
  const binary = path.resolve(required(values, 'atomwrite-bin'));
  const workspace = path.resolve(required(values, 'workspace'));
  const manifestPath = path.resolve(required(values, 'manifest'));
  const mode = required(values, 'mode');
  const version = validatedVersion(values);
  if (mode !== 'dry-run' && mode !== 'durable-apply') {
    throw new Error(`unsupported mode: ${mode}`);
  }
  const workersRequested = Number(required(values, 'workers'));
  if (!Number.isInteger(workersRequested) || workersRequested < 1) {
    throw new Error('--workers must be a positive integer');
  }
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  validateManifest(manifest);
  const operations = atomwriteOperations(manifest);
  const input = `${operations.map((operation) => JSON.stringify(operation)).join('\n')}\n`;
  const args = [
    '--workspace', workspace,
    '--no-progress',
    'batch',
    '--threads', String(workersRequested),
    '--transaction',
    '--no-backup',
    '--retention', '1',
  ];
  if (mode === 'dry-run') args.push('--dry-run');

  const started = process.hrtime.bigint();
  const result = spawnSync(binary, args, {
    input,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    timeout: childTimeout(values),
    windowsHide: true,
  });
  const elapsed = process.hrtime.bigint() - started;
  if (result.error) throw result.error;
  const events = parseNdjson(result.stdout);
  const summary = [...events].reverse().find((event) => event.type === 'summary') ?? null;
  const eventTypeCounts = {};
  for (const event of events) {
    const type = typeof event.type === 'string' ? event.type : 'untyped';
    eventTypeCounts[type] = (eventTypeCounts[type] ?? 0) + 1;
  }
  const underlyingOk = result.status === 0
    && summary !== null
    && Number(summary.failed) === 0
    && Number(summary.succeeded) === operations.length;
  return {
    schema: ADAPTER_SCHEMA,
    ok: underlyingOk,
    adapter: 'atomwrite-batch-transaction',
    atomwrite_version: version,
    mode,
    workload: manifest.workload,
    operations: manifest.operation_count,
    touched_paths: manifest.touched_path_count,
    underlying_operations: operations.length,
    workers_requested: workersRequested,
    workers_effective: workersRequested,
    worker_control: 'BATCH_THREADS_EXPLICIT',
    adapter_elapsed_ns: elapsed.toString(),
    underlying_exit_code: result.status,
    underlying_signal: result.signal,
    underlying_stdout_sha256: createHash('sha256').update(result.stdout).digest('hex'),
    underlying_stderr: result.stderr.slice(0, 16_384),
    event_type_counts: eventTypeCounts,
    summary,
    stale_guard: 'NO_EXPECTED_CHECKSUM_FIELD_IN_BATCH_SCHEMA_0_1_36',
    capability_class: 'ORDERED_CREATE_DELETE_MOVE_AND_EXACT_REPLACE_BATCH',
    durability_contract: mode === 'dry-run'
      ? 'NO_WRITE_OPERATION_PREFLIGHT_WITHOUT_FULL_SOURCE_HASH_CAS'
      : 'PER_OPERATION_ATOMIC_WRITES_WITH_COMPENSATING_BACKUP_ROLLBACK',
    equivalent_to_weavatrix_recoverable_batch: false,
    error: underlyingOk ? null : 'atomwrite batch did not report a fully successful summary',
  };
}

async function main() {
  try {
    const values = parseArgs(process.argv.slice(2));
    const binary = values.get('atomwrite-bin');
    if (values.get('version') === true) {
      if (typeof binary !== 'string') throw new Error('--atomwrite-bin is required with --version');
      process.stdout.write(`${atomwriteVersion(path.resolve(binary))}\n`);
      return;
    }
    const result = await run(values);
    process.stdout.write(`${JSON.stringify(result)}\n`);
    if (!result.ok) process.exitCode = 1;
  } catch (error) {
    process.stdout.write(`${JSON.stringify({
      schema: ADAPTER_SCHEMA,
      ok: false,
      adapter: 'atomwrite-batch-transaction',
      error: error instanceof Error ? error.message : String(error),
      durability_contract: 'UNKNOWN_BECAUSE_ADAPTER_FAILED',
      equivalent_to_weavatrix_recoverable_batch: false,
    })}\n`);
    process.exitCode = 1;
  }
}

await main();
