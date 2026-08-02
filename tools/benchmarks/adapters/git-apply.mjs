#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import path from 'node:path';

const SCHEMA = 'weavatrix.worktree-benchmark-adapter.v1';

function parseArgs(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 1) {
    const item = argv[index];
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

async function run(values) {
  const binary = path.resolve(required(values, 'git-bin'));
  const workspace = path.resolve(required(values, 'workspace'));
  const repositoryRoot = path.resolve(required(values, 'repository-root'));
  const manifestPath = path.resolve(required(values, 'manifest'));
  const patchPath = path.resolve(required(values, 'patch'));
  const mode = required(values, 'mode');
  if (mode !== 'dry-run' && mode !== 'non-durable-apply') {
    throw new Error(`unsupported mode: ${mode}`);
  }
  const [manifestText, patch] = await Promise.all([
    readFile(manifestPath, 'utf8'),
    readFile(patchPath),
  ]);
  const manifest = JSON.parse(manifestText);
  if (manifest.schema !== 'weavatrix.worktree-benchmark-manifest.v1') {
    throw new Error(`unsupported manifest schema: ${String(manifest.schema)}`);
  }
  if (!Array.isArray(manifest.files)
    || manifest.files.length !== Number(manifest.file_count)) {
    throw new Error('manifest file count mismatch');
  }
  const relativeWorkspace = path.relative(repositoryRoot, workspace);
  if (relativeWorkspace.length === 0
    || relativeWorkspace.startsWith('..')
    || path.isAbsolute(relativeWorkspace)) {
    throw new Error('Git fixture workspace must be inside the harness repository root');
  }
  const directory = relativeWorkspace.split(path.sep).join('/');
  const args = [
    'apply',
    '--no-index',
    `--directory=${directory}`,
    '--whitespace=nowarn',
  ];
  if (mode === 'dry-run') {
    args.push('--check');
  }
  args.push('-');
  const gitEnvironment = { ...process.env };
  delete gitEnvironment.GIT_DIR;
  delete gitEnvironment.GIT_WORK_TREE;
  delete gitEnvironment.GIT_INDEX_FILE;
  delete gitEnvironment.GIT_PREFIX;
  const started = process.hrtime.bigint();
  const result = spawnSync(binary, args, {
    cwd: repositoryRoot,
    input: patch,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    timeout: 120_000,
    windowsHide: true,
    env: gitEnvironment,
  });
  const elapsed = process.hrtime.bigint() - started;
  if (result.error) {
    throw result.error;
  }
  const ok = result.status === 0;
  return {
    schema: SCHEMA,
    ok,
    adapter: 'git-apply',
    mode,
    files: manifest.files.length,
    workers_requested: null,
    workers_effective: null,
    worker_control: 'NOT_SUPPORTED_BY_GIT_APPLY',
    adapter_elapsed_ns: elapsed.toString(),
    underlying_exit_code: result.status,
    underlying_signal: result.signal,
    underlying_stdout_sha256: createHash('sha256').update(result.stdout).digest('hex'),
    underlying_stderr: result.stderr.slice(0, 16_384),
    patch_sha256: createHash('sha256').update(patch).digest('hex'),
    stale_guard: 'UNIFIED_DIFF_CONTEXT_WITHOUT_FULL_FILE_HASH_CAS',
    repository_discovery: 'EXPLICIT_HARNESS_ROOT_WITH_NO_INDEX_AND_DIRECTORY_PREFIX',
    partial_apply_policy: 'DEFAULT_WHOLE_PATCH_ON_HUNK_APPLICABILITY_ERROR_NO_REJECT',
    durability_contract: mode === 'dry-run'
      ? 'NO_WRITE_UNIFIED_DIFF_APPLICABILITY_CHECK'
      : 'WHOLE_PATCH_PREFLIGHT_WITH_NON_DURABLE_WORKTREE_WRITES',
    crash_recovery: 'NONE_DOCUMENTED',
    equivalent_to_weavatrix_recoverable_batch: false,
    error: ok ? null : 'git apply rejected or failed to write the patch',
  };
}

async function main() {
  try {
    const result = await run(parseArgs(process.argv.slice(2)));
    process.stdout.write(`${JSON.stringify(result)}\n`);
    if (!result.ok) {
      process.exitCode = 1;
    }
  } catch (error) {
    process.stdout.write(`${JSON.stringify({
      schema: SCHEMA,
      ok: false,
      adapter: 'git-apply',
      error: error instanceof Error ? error.message : String(error),
      durability_contract: 'UNKNOWN_BECAUSE_ADAPTER_FAILED',
      equivalent_to_weavatrix_recoverable_batch: false,
    })}\n`);
    process.exitCode = 1;
  }
}

await main();
