#!/usr/bin/env node

import { createHash } from 'node:crypto';
import {
  open,
  readFile,
  rename,
  rm,
} from 'node:fs/promises';
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

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function safeTarget(workspace, relative) {
  if (typeof relative !== 'string' || relative.length === 0 || path.isAbsolute(relative)) {
    throw new Error(`unsafe benchmark path: ${String(relative)}`);
  }
  const normalizedParts = relative.split('/');
  if (normalizedParts.some((part) => part === '' || part === '.' || part === '..')) {
    throw new Error(`unsafe benchmark path: ${relative}`);
  }
  const root = path.resolve(workspace);
  const target = path.resolve(root, ...normalizedParts);
  if (target !== root && !target.startsWith(`${root}${path.sep}`)) {
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
  let currentLine = 1;
  while (currentLine < position.line) {
    const newline = source.indexOf('\n', offset);
    if (newline === -1) {
      throw new Error(`line ${position.line} is outside source`);
    }
    offset = newline + 1;
    currentLine += 1;
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
  return output;
}

async function mapLimit(items, limit, operation) {
  const results = new Array(items.length);
  let next = 0;
  async function worker() {
    while (true) {
      const index = next;
      next += 1;
      if (index >= items.length) {
        return;
      }
      results[index] = await operation(items[index], index);
    }
  }
  const active = Math.max(1, Math.min(limit, items.length));
  await Promise.all(Array.from({ length: active }, () => worker()));
  return results;
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
  if (mode !== 'dry-run' && mode !== 'durable-apply') {
    throw new Error(`unsupported mode: ${mode}`);
  }
  const requestedWorkers = Number(required(values, 'workers'));
  if (!Number.isInteger(requestedWorkers) || requestedWorkers < 1) {
    throw new Error('--workers must be a positive integer');
  }
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  if (manifest.schema !== 'weavatrix.worktree-benchmark-manifest.v1') {
    throw new Error(`unsupported manifest schema: ${String(manifest.schema)}`);
  }
  if (!Array.isArray(manifest.files) || manifest.files.length === 0) {
    throw new Error('manifest has no files');
  }
  const workers = Math.min(requestedWorkers, manifest.files.length);
  const started = process.hrtime.bigint();
  const projected = await mapLimit(manifest.files, workers, async (file, index) => {
    const target = safeTarget(workspace, file.path);
    const sourceBytes = await readFile(target);
    const sourceHash = sha256(sourceBytes);
    if (sourceHash !== file.sha256) {
      throw new Error(`source SHA-256 mismatch for ${file.path}`);
    }
    const source = sourceBytes.toString('utf8');
    if (Buffer.byteLength(source, 'utf8') !== sourceBytes.length) {
      throw new Error(`invalid UTF-8 for ${file.path}`);
    }
    const output = render(source, file.edits);
    const outputBytes = Buffer.from(output, 'utf8');
    if (sha256(outputBytes) !== file.expected_sha256) {
      throw new Error(`output SHA-256 mismatch for ${file.path}`);
    }
    return { index, target, outputBytes, sourceHash, stage: null };
  });

  let directorySyncSupported = null;
  if (mode === 'durable-apply') {
    await mapLimit(projected, workers, async (file) => {
      const stage = `${file.target}.wvx-bench-${process.pid}-${file.index}.stage`;
      const handle = await open(stage, 'wx');
      try {
        await handle.writeFile(file.outputBytes);
        await handle.sync();
      } finally {
        await handle.close();
      }
      file.stage = stage;
    });
    try {
      for (const file of projected) {
        const current = await readFile(file.target);
        if (sha256(current) !== file.sourceHash) {
          throw new Error(`source changed before commit: ${file.target}`);
        }
        await rename(file.stage, file.target);
        file.stage = null;
      }
      const parents = [...new Set(projected.map((file) => path.dirname(file.target)))];
      const syncResults = await Promise.all(parents.map((parent) => syncParent(parent)));
      directorySyncSupported = syncResults.every(Boolean);
    } finally {
      await Promise.all(projected.map(async (file) => {
        if (file.stage !== null) {
          await rm(file.stage, { force: true }).catch(() => {});
        }
      }));
    }
  }
  const elapsed = process.hrtime.bigint() - started;
  return {
    schema: SCHEMA,
    ok: true,
    adapter: 'reference-self-check',
    mode,
    files: manifest.files.length,
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
      process.stdout.write('reference-self-check 1\n');
      return;
    }
    const result = await run(values);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } catch (error) {
    process.stdout.write(`${JSON.stringify({
      schema: SCHEMA,
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
