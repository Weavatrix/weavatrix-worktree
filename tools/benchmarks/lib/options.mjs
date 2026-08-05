import path from 'node:path';

import {
  ALLOWED_COUNTS,
  ALLOWED_MODES,
  ALLOWED_WORKLOADS,
  MAX_FILE_BYTES,
  MAX_WORKERS,
  RESULT_ROOT,
} from './constants.mjs';
import { timestampId } from './util.mjs';

export const HELP = `Usage:
  node tools/benchmarks/run.mjs self-check [--output DIR]
  node tools/benchmarks/run.mjs detect [--weavatrix-bin PATH] [--atomwrite-bin PATH] [--git-bin PATH]
  node tools/benchmarks/run.mjs build-weavatrix
  node tools/benchmarks/run.mjs install-atomwrite
  node tools/benchmarks/run.mjs refresh-machine-filesystem --result DIR
  node tools/benchmarks/run.mjs run [options]

Run options:
  --adapter NAME          reference, weavatrix, atomwrite, or git-apply
  --workloads LIST        modify,create,delete,rename,mixed (default all)
  --counts LIST           comma list from 1,5,10,64 (default all)
  --workers LIST          comma list <=64 (default 1,2,4,8; invalid for git-apply)
  --modes LIST            adapter-specific modes (default both supported modes)
  --file-bytes N          exact bytes, 1024..1048576, per UTF-8 file (default 65536)
  --warmups N             retained warmups per configuration (default 5)
  --repetitions N         recorded samples per configuration (default 30)
  --timeout-ms N          subprocess timeout (default 120000)
  --seed N                deterministic matrix shuffle seed (default 20260802)
  --output DIR            result directory; must be absent or empty
  --weavatrix-bin PATH    explicit Rust adapter binary
  --atomwrite-bin PATH    explicit atomwrite binary
  --git-bin PATH          explicit Git binary
`;

export const RUN_OPTION_NAMES = new Set([
  'adapter', 'workloads', 'counts', 'workers', 'modes', 'file-bytes',
  'warmups', 'repetitions', 'timeout-ms', 'seed', 'output',
  'weavatrix-bin', 'atomwrite-bin', 'git-bin',
]);

export function rejectUnknownOptions(options, allowed, command) {
  const unknown = Object.keys(options).filter((name) => !allowed.has(name));
  if (unknown.length > 0) {
    throw new Error(`${command} does not support option(s): ${unknown.map((name) => `--${name}`).join(', ')}`);
  }
}

export function parseOptions(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const item = argv[index];
    if (!item.startsWith('--')) {
      throw new Error(`unexpected positional argument: ${item}`);
    }
    const equals = item.indexOf('=');
    if (equals !== -1) {
      options[item.slice(2, equals)] = item.slice(equals + 1);
      continue;
    }
    const next = argv[index + 1];
    if (next === undefined || next.startsWith('--')) {
      throw new Error(`missing value for ${item}`);
    }
    options[item.slice(2)] = next;
    index += 1;
  }
  return options;
}

function integerOption(options, name, fallback, minimum = 0) {
  const value = Number(options[name] ?? String(fallback));
  if (!Number.isSafeInteger(value) || value < minimum) {
    throw new Error(`--${name} must be an integer >= ${minimum}`);
  }
  return value;
}

function listOption(options, name, fallback) {
  const values = String(options[name] ?? fallback)
    .split(',').map((value) => value.trim()).filter(Boolean);
  if (values.length === 0 || new Set(values).size !== values.length) {
    throw new Error(`--${name} must contain unique comma-separated values`);
  }
  return values;
}

function integerListOption(options, name, fallback) {
  return listOption(options, name, fallback).map((raw) => {
    const value = Number(raw);
    if (!Number.isSafeInteger(value) || value < 1) {
      throw new Error(`--${name} contains an invalid positive integer: ${raw}`);
    }
    return value;
  });
}

export function validateRunOptions(options, overrides = {}) {
  const adapter = overrides.adapter ?? options.adapter ?? 'weavatrix';
  if (!['reference', 'weavatrix', 'atomwrite', 'git-apply'].includes(adapter)) {
    throw new Error('--adapter must be reference, weavatrix, atomwrite, or git-apply');
  }
  const counts = overrides.counts ?? integerListOption(options, 'counts', '1,5,10,64');
  for (const count of counts) {
    if (!ALLOWED_COUNTS.has(count)) {
      throw new Error(`unsupported --counts value: ${count}`);
    }
  }
  const workloads = overrides.workloads
    ?? listOption(options, 'workloads', 'modify,create,delete,rename,mixed');
  for (const workload of workloads) {
    if (!ALLOWED_WORKLOADS.has(workload)) {
      throw new Error(`unsupported --workloads value: ${workload}`);
    }
  }
  if (workloads.includes('mixed') && !counts.includes(10)) {
    throw new Error('the mixed workload requires --counts to include 10');
  }
  const workerless = adapter === 'git-apply';
  if (workerless && (options.workers !== undefined || overrides.workers !== undefined)) {
    throw new Error('--workers is not supported by git-apply');
  }
  const requestedWorkers = workerless
    ? []
    : (overrides.workers ?? integerListOption(options, 'workers', '1,2,4,8'));
  if (requestedWorkers.some((workers) => workers > MAX_WORKERS)) {
    throw new Error(`--workers values must not exceed ${MAX_WORKERS}`);
  }
  const defaultModes = workerless
    ? 'dry-run,non-durable-apply'
    : 'dry-run,durable-apply';
  const modes = overrides.modes ?? listOption(options, 'modes', defaultModes);
  for (const mode of modes) {
    if (!ALLOWED_MODES.has(mode)) {
      throw new Error(`unsupported --modes value: ${mode}`);
    }
    if (workerless && mode === 'durable-apply') {
      throw new Error('git-apply does not support durable-apply');
    }
    if (!workerless && mode === 'non-durable-apply') {
      throw new Error(`${adapter} does not use the non-durable-apply benchmark mode`);
    }
  }
  const output = path.resolve(overrides.output ?? options.output
    ?? path.join(RESULT_ROOT, `${timestampId()}-${adapter}`));
  const fileBytes = overrides.fileBytes ?? integerOption(options, 'file-bytes', 65_536, 1_024);
  if (fileBytes > MAX_FILE_BYTES) {
    throw new Error(`--file-bytes must not exceed ${MAX_FILE_BYTES}`);
  }
  return {
    adapter,
    workloads,
    counts,
    requested_workers: requestedWorkers,
    workers: workerless ? [null] : requestedWorkers,
    modes,
    file_bytes: fileBytes,
    warmups: overrides.warmups ?? integerOption(options, 'warmups', 5, 0),
    repetitions: overrides.repetitions ?? integerOption(options, 'repetitions', 30, 1),
    timeout_ms: overrides.timeoutMs ?? integerOption(options, 'timeout-ms', 120_000, 1),
    seed: overrides.seed ?? integerOption(options, 'seed', 20_260_802, 0),
    output,
    weavatrix_bin: options['weavatrix-bin'],
    atomwrite_bin: options['atomwrite-bin'],
    git_bin: options['git-bin'],
  };
}
