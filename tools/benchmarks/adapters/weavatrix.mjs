#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import path from 'node:path';

const SCHEMA = 'weavatrix.worktree-benchmark-adapter.v2';

function parseArgs(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 1) {
    const item = argv[index];
    if (!item.startsWith('--')) throw new Error(`unexpected positional argument: ${item}`);
    const equals = item.indexOf('=');
    if (equals !== -1) {
      values.set(item.slice(2, equals), item.slice(equals + 1));
      continue;
    }
    const next = argv[index + 1];
    if (next === undefined || next.startsWith('--')) throw new Error(`missing value for ${item}`);
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

function childTimeout(values) {
  const outer = Number(required(values, 'timeout-ms'));
  if (!Number.isSafeInteger(outer) || outer < 1) throw new Error('--timeout-ms is invalid');
  const grace = Math.min(1_000, Math.max(1, Math.floor(outer / 10)));
  return Math.max(1, outer - grace);
}

function run(values) {
  const binary = path.resolve(required(values, 'weavatrix-bin'));
  const args = [
    '--workspace', path.resolve(required(values, 'workspace')),
    '--manifest', path.resolve(required(values, 'manifest')),
    '--mode', required(values, 'mode'),
    '--workers', required(values, 'workers'),
  ];
  const result = spawnSync(binary, args, {
    encoding: 'utf8',
    timeout: childTimeout(values),
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.stderr.length > 0) process.stderr.write(result.stderr);
  if (result.error) throw result.error;
  if (result.stdout.trim().length === 0) {
    throw new Error(`weavatrix adapter exited ${String(result.status)} without JSON`);
  }
  process.stdout.write(result.stdout);
  process.exitCode = result.status ?? 1;
}

try {
  run(parseArgs(process.argv.slice(2)));
} catch (error) {
  process.stdout.write(`${JSON.stringify({
    schema: SCHEMA,
    ok: false,
    adapter: 'weavatrix-worktree-rust',
    error: error instanceof Error ? error.message : String(error),
    durability_contract: 'UNKNOWN_BECAUSE_ADAPTER_FAILED',
    equivalent_to_weavatrix_recoverable_batch: false,
  })}\n`);
  process.exitCode = 1;
}
