import path from 'node:path';

import { ALLOWED_COUNTS, ALLOWED_MODES, RESULT_ROOT } from './constants.mjs';
import { timestampId } from './util.mjs';

export const HELP = `Usage:
  node tools/benchmarks/run.mjs self-check [--output DIR]
  node tools/benchmarks/run.mjs detect [--weavatrix-bin PATH] [--atomwrite-bin PATH]
  node tools/benchmarks/run.mjs build-weavatrix
  node tools/benchmarks/run.mjs install-atomwrite
  node tools/benchmarks/run.mjs refresh-machine-filesystem --result DIR
  node tools/benchmarks/run.mjs run [options]

Run options:
  --adapter NAME          reference, weavatrix, or atomwrite (default weavatrix)
  --counts LIST           comma list from 1,5,10,64 (default all)
  --workers LIST          comma list (default 1,2,4,8)
  --modes LIST            dry-run,durable-apply (default both)
  --file-bytes N          exact bytes per UTF-8 fixture file (default 65536)
  --warmups N             retained warmups per configuration (default 5)
  --repetitions N         recorded samples per configuration (default 30)
  --timeout-ms N          subprocess timeout (default 120000)
  --seed N                deterministic matrix shuffle seed (default 20260802)
  --output DIR            result directory; must be absent or empty
  --weavatrix-bin PATH    explicit Rust adapter binary
  --atomwrite-bin PATH    explicit atomwrite binary
`;

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
  if (!['reference', 'weavatrix', 'atomwrite'].includes(adapter)) {
    throw new Error('--adapter must be reference, weavatrix, or atomwrite');
  }
  const counts = overrides.counts ?? integerListOption(options, 'counts', '1,5,10,64');
  for (const count of counts) {
    if (!ALLOWED_COUNTS.has(count)) {
      throw new Error(`unsupported --counts value: ${count}`);
    }
  }
  const requestedWorkers = overrides.workers
    ?? integerListOption(options, 'workers', '1,2,4,8');
  const modes = overrides.modes ?? listOption(options, 'modes', 'dry-run,durable-apply');
  for (const mode of modes) {
    if (!ALLOWED_MODES.has(mode)) {
      throw new Error(`unsupported --modes value: ${mode}`);
    }
  }
  const output = path.resolve(overrides.output ?? options.output
    ?? path.join(RESULT_ROOT, `${timestampId()}-${adapter}`));
  return {
    adapter,
    counts,
    requested_workers: requestedWorkers,
    workers: requestedWorkers,
    modes,
    file_bytes: overrides.fileBytes ?? integerOption(options, 'file-bytes', 65_536, 1_024),
    warmups: overrides.warmups ?? integerOption(options, 'warmups', 5, 0),
    repetitions: overrides.repetitions ?? integerOption(options, 'repetitions', 30, 1),
    timeout_ms: overrides.timeoutMs ?? integerOption(options, 'timeout-ms', 120_000, 1),
    seed: overrides.seed ?? integerOption(options, 'seed', 20_260_802, 0),
    output,
    weavatrix_bin: options['weavatrix-bin'],
    atomwrite_bin: options['atomwrite-bin'],
  };
}
