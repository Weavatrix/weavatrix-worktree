#!/usr/bin/env node

import { createHash } from 'node:crypto';
import {
  open,
  readFile,
  rename,
  rm,
  unlink,
} from 'node:fs/promises';
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

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function safeTarget(workspace, relative) {
  if (typeof relative !== 'string' || relative.length === 0 || path.isAbsolute(relative)) {
    throw new Error(`unsafe benchmark path: ${String(relative)}`);
  }
  const parts = relative.split('/');
  if (parts.some((part) => part === '' || part === '.' || part === '..')) {
    throw new Error(`unsafe benchmark path: ${relative}`);
  }
  const root = path.resolve(workspace);
  const target = path.resolve(root, ...parts);
  if (target === root || !target.startsWith(`${root}${path.sep}`)) {
    throw new Error(`benchmark path escapes workspace: ${relative}`);
  }
  return target;
}

function offsetAt(source, position) {
  if (!Number.isInteger(position.line) || position.line < 1) {
    throw new Error(`invalid one-based line: ${position.line}`);
  }
  if (!Number.isInteger(position.character) || position.character < 0) {
    throw new Error(`invalid character: ${position.character}`);
  }
  let offset = 0;
  for (let line = 1; line < position.line; line += 1) {
    const newline = source.indexOf('\n', offset);
    if (newline === -1) throw new Error(`line ${position.line} is outside source`);
    offset = newline + 1;
  }
  const lineEnd = source.indexOf('\n', offset);
  const effectiveEnd = lineEnd === -1 ? source.length : lineEnd;
  const selected = offset + position.character;
  if (selected > effectiveEnd) {
    throw new Error(`character ${position.character} is outside line ${position.line}`);
  }
  return selected;
}

function render(source, edits) {
  const resolved = edits.map((edit) => {
    const start = offsetAt(source, edit.start);
    const end = offsetAt(source, edit.end);
    if (start > end || source.slice(start, end) !== edit.expected) {
      throw new Error(`edit preimage mismatch at ${edit.start.line}:${edit.start.character}`);
    }
    return { ...edit, startOffset: start, endOffset: end };
  });
  resolved.sort((left, right) => right.startOffset - left.startOffset);
  for (let index = 1; index < resolved.length; index += 1) {
    if (resolved[index - 1].startOffset < resolved[index].endOffset) {
      throw new Error('overlapping benchmark edits');
    }
  }
  let output = source;
  for (const edit of resolved) {
    output = `${output.slice(0, edit.startOffset)}${edit.replacement}${output.slice(edit.endOffset)}`;
  }
  return Buffer.from(output, 'utf8');
}

async function mustBeMissing(target) {
  try {
    await readFile(target);
  } catch (error) {
    if (error?.code === 'ENOENT') return;
    throw error;
  }
  throw new Error(`target must be absent: ${target}`);
}

async function mapLimit(items, limit, operation) {
  const results = new Array(items.length);
  let next = 0;
  async function worker() {
    while (true) {
      const index = next;
      next += 1;
      if (index >= items.length) return;
      results[index] = await operation(items[index], index);
    }
  }
  const active = Math.max(1, Math.min(limit, items.length));
  await Promise.all(Array.from({ length: active }, () => worker()));
  return results;
}

async function prepareOperation(workspace, operation, index) {
  if (operation.type === 'modify') {
    const target = safeTarget(workspace, operation.path);
    const source = await readFile(target);
    if (sha256(source) !== operation.source_sha256) throw new Error(`source hash mismatch: ${operation.path}`);
    const output = render(source.toString('utf8'), operation.edits);
    if (sha256(output) !== operation.output_sha256) throw new Error(`output hash mismatch: ${operation.path}`);
    return { type: 'modify', target, output, stage: null, index };
  }
  if (operation.type === 'create') {
    const target = safeTarget(workspace, operation.path);
    await mustBeMissing(target);
    const output = Buffer.from(operation.content, 'utf8');
    if (sha256(output) !== operation.output_sha256) throw new Error(`create hash mismatch: ${operation.path}`);
    return { type: 'create', target, output, stage: null, index };
  }
  if (operation.type === 'delete') {
    const target = safeTarget(workspace, operation.path);
    const source = await readFile(target);
    if (sha256(source) !== operation.source_sha256) throw new Error(`delete hash mismatch: ${operation.path}`);
    return { type: 'delete', target, index };
  }
  if (operation.type === 'rename') {
    const sourceTarget = safeTarget(workspace, operation.source);
    const destination = safeTarget(workspace, operation.target);
    const source = await readFile(sourceTarget);
    if (sha256(source) !== operation.source_sha256) throw new Error(`rename hash mismatch: ${operation.source}`);
    await mustBeMissing(destination);
    return { type: 'rename', source: sourceTarget, target: destination, index };
  }
  throw new Error(`unsupported operation: ${String(operation.type)}`);
}

async function stageOutput(operation) {
  if (operation.type !== 'modify' && operation.type !== 'create') return;
  const stage = `${operation.target}.wvx-reference-${process.pid}-${operation.index}.stage`;
  const handle = await open(stage, 'wx');
  try {
    await handle.writeFile(operation.output);
    await handle.sync();
  } finally {
    await handle.close();
  }
  operation.stage = stage;
}

async function syncParent(parent) {
  let handle;
  try {
    handle = await open(parent, 'r');
    await handle.sync();
    return true;
  } catch {
    return false;
  } finally {
    await handle?.close().catch(() => {});
  }
}

async function run(values) {
  const workspace = path.resolve(required(values, 'workspace'));
  const manifestPath = path.resolve(required(values, 'manifest'));
  const mode = required(values, 'mode');
  if (mode !== 'dry-run' && mode !== 'durable-apply') throw new Error(`unsupported mode: ${mode}`);
  const requestedWorkers = Number(required(values, 'workers'));
  if (!Number.isInteger(requestedWorkers) || requestedWorkers < 1) {
    throw new Error('--workers must be a positive integer');
  }
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  if (manifest.schema !== MANIFEST_SCHEMA) throw new Error(`unsupported manifest schema: ${String(manifest.schema)}`);
  if (!Array.isArray(manifest.operations)
    || manifest.operations.length !== Number(manifest.operation_count)
    || manifest.operations.length === 0) {
    throw new Error('manifest operation count mismatch');
  }
  const workers = Math.min(requestedWorkers, manifest.operations.length);
  const started = process.hrtime.bigint();
  const prepared = await mapLimit(
    manifest.operations, workers,
    (operation, index) => prepareOperation(workspace, operation, index),
  );

  let directorySyncSupported = null;
  if (mode === 'durable-apply') {
    await mapLimit(prepared, workers, stageOutput);
    try {
      const parents = new Set();
      for (const operation of prepared) {
        if (operation.type === 'modify' || operation.type === 'create') {
          await rename(operation.stage, operation.target);
          operation.stage = null;
          parents.add(path.dirname(operation.target));
        } else if (operation.type === 'delete') {
          await unlink(operation.target);
          parents.add(path.dirname(operation.target));
        } else if (operation.type === 'rename') {
          await rename(operation.source, operation.target);
          parents.add(path.dirname(operation.source));
          parents.add(path.dirname(operation.target));
        }
      }
      const syncResults = await Promise.all([...parents].map(syncParent));
      directorySyncSupported = syncResults.every(Boolean);
    } finally {
      await Promise.all(prepared.map(async (operation) => {
        if (operation.stage !== null && operation.stage !== undefined) {
          await rm(operation.stage, { force: true }).catch(() => {});
        }
      }));
    }
  }
  const elapsed = process.hrtime.bigint() - started;
  return {
    schema: ADAPTER_SCHEMA,
    ok: true,
    adapter: 'reference-self-check',
    mode,
    workload: manifest.workload,
    operations: manifest.operation_count,
    touched_paths: manifest.touched_path_count,
    workers_requested: requestedWorkers,
    workers_effective: workers,
    adapter_elapsed_ns: elapsed.toString(),
    directory_sync_supported: directorySyncSupported,
    durability_contract: 'HARNESS_SELF_CHECK_ONLY_NOT_PUBLISHABLE',
    equivalent_to_weavatrix_recoverable_batch: false,
  };
}

async function main() {
  try {
    const values = parseArgs(process.argv.slice(2));
    if (values.get('version') === true) {
      process.stdout.write('reference-self-check 2\n');
      return;
    }
    const result = await run(values);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } catch (error) {
    process.stdout.write(`${JSON.stringify({
      schema: ADAPTER_SCHEMA,
      ok: false,
      adapter: 'reference-self-check',
      error: error instanceof Error ? error.message : String(error),
      durability_contract: 'HARNESS_SELF_CHECK_ONLY_NOT_PUBLISHABLE',
      equivalent_to_weavatrix_recoverable_batch: false,
    })}\n`);
    process.exitCode = 1;
  }
}

await main();
