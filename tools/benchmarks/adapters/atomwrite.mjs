#!/usr/bin/env node

import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';

const SCHEMA = 'weavatrix.worktree-benchmark-adapter.v1';

function parseArgs(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 1) {
    const item = argv[index];
    if (item === '--version') {
      values.set('version', true);
      continue;
    }
    if (!item.startsWith('--')) {
      throw new Error(`unexpected positional argument: ${item}`);
    }
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
  if (typeof value !== 'string' || value.length === 0) {
    throw new Error(`--${name} is required`);
  }
  return value;
}

function parseNdjson(stdout) {
  const events = [];
  for (const line of stdout.split(/\r?\n/u)) {
    if (line.trim().length === 0) {
      continue;
    }
    try {
      events.push(JSON.parse(line));
    } catch (error) {
      throw new Error(`atomwrite emitted non-JSON stdout: ${line.slice(0, 200)}`, {
        cause: error,
      });
    }
  }
  return events;
}

function atomwriteVersion(binary) {
  const result = spawnSync(binary, ['--version'], {
    encoding: 'utf8',
    windowsHide: true,
    timeout: 30_000,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`atomwrite --version exited ${String(result.status)}: ${result.stderr}`);
  }
  return result.stdout.trim();
}

async function run(values) {
  const binary = path.resolve(required(values, 'atomwrite-bin'));
  const workspace = path.resolve(required(values, 'workspace'));
  const manifestPath = path.resolve(required(values, 'manifest'));
  const mode = required(values, 'mode');
  if (mode !== 'dry-run' && mode !== 'durable-apply') {
    throw new Error(`unsupported mode: ${mode}`);
  }
  const workersRequested = Number(required(values, 'workers'));
  if (!Number.isInteger(workersRequested) || workersRequested < 1) {
    throw new Error('--workers must be a positive integer');
  }
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  if (manifest.schema !== 'weavatrix.worktree-benchmark-manifest.v1') {
    throw new Error(`unsupported manifest schema: ${String(manifest.schema)}`);
  }

  const operations = [];
  for (const file of manifest.files) {
    for (const edit of file.edits) {
      operations.push({
        op: 'replace',
        path: file.path,
        pattern: edit.expected,
        replacement: edit.replacement,
        backup: false,
      });
    }
  }
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
  if (mode === 'dry-run') {
    args.push('--dry-run');
  }

  const started = process.hrtime.bigint();
  const result = spawnSync(binary, args, {
    input,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    timeout: 120_000,
    windowsHide: true,
  });
  const elapsed = process.hrtime.bigint() - started;
  if (result.error) {
    throw result.error;
  }
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
    schema: SCHEMA,
    ok: underlyingOk,
    adapter: 'atomwrite-batch-transaction',
    atomwrite_version: atomwriteVersion(binary),
    mode,
    files: manifest.files.length,
    operations: operations.length,
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
    stale_guard: 'NO_EXPECTED_CHECKSUM_FIELD_IN_BATCH_SCHEMA_0_1_35',
    durability_contract: mode === 'dry-run'
      ? 'READ_AND_EXACT_MATCH_PREVIEW_WITHOUT_SOURCE_HASH_CAS'
      : 'PER_FILE_ATOMIC_AUTO_DURABILITY_WITH_COMPENSATING_BACKUP_ROLLBACK',
    equivalent_to_weavatrix_recoverable_batch: false,
    error: underlyingOk ? null : 'atomwrite batch did not report a fully successful summary',
  };
}

async function main() {
  try {
    const values = parseArgs(process.argv.slice(2));
    const binary = values.get('atomwrite-bin');
    if (values.get('version') === true) {
      if (typeof binary !== 'string') {
        throw new Error('--atomwrite-bin is required with --version');
      }
      process.stdout.write(`${atomwriteVersion(path.resolve(binary))}\n`);
      return;
    }
    const result = await run(values);
    process.stdout.write(`${JSON.stringify(result)}\n`);
    if (!result.ok) {
      process.exitCode = 1;
    }
  } catch (error) {
    process.stdout.write(`${JSON.stringify({
      schema: SCHEMA,
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
