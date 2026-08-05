#!/usr/bin/env node

import path from 'node:path';

import {
  buildWeavatrix,
  detectAdapters,
  installAtomwrite,
} from './lib/adapters.mjs';
import { RESULT_ROOT } from './lib/constants.mjs';
import { runMatrix } from './lib/matrix.mjs';
import {
  HELP,
  RUN_OPTION_NAMES,
  parseOptions,
  rejectUnknownOptions,
  validateRunOptions,
} from './lib/options.mjs';
import { refreshMachineFilesystem } from './lib/results.mjs';
import { timestampId } from './lib/util.mjs';

function selfCheck(options) {
  const config = validateRunOptions(options, {
    adapter: 'reference',
    workloads: ['modify', 'create', 'delete', 'rename', 'mixed'],
    counts: [1, 10],
    workers: [1],
    modes: ['dry-run', 'durable-apply'],
    fileBytes: 4_096,
    warmups: 0,
    repetitions: 1,
    timeoutMs: 30_000,
    seed: 20_260_802,
    output: options.output === undefined
      ? path.join(RESULT_ROOT, `${timestampId()}-self-check`)
      : path.resolve(options.output),
  });
  const result = runMatrix(config);
  const expectedSamples = 18;
  const ok = result.samples === expectedSamples && result.all_gates_pass;
  process.stdout.write(`${JSON.stringify({
    self_check: ok ? 'pass' : 'FAIL',
    expected_samples: expectedSamples,
    ...result,
  }, null, 2)}\n`);
  if (!ok) {
    process.exitCode = 1;
  }
}

function main() {
  const [command = 'help', ...rest] = process.argv.slice(2);
  const options = parseOptions(rest);
  if (command === 'help' || command === '--help' || command === '-h') {
    rejectUnknownOptions(options, new Set(), 'help');
    process.stdout.write(HELP);
  } else if (command === 'detect') {
    rejectUnknownOptions(
      options,
      new Set(['weavatrix-bin', 'atomwrite-bin', 'git-bin']),
      'detect',
    );
    process.stdout.write(`${JSON.stringify(detectAdapters(options), null, 2)}\n`);
  } else if (command === 'build-weavatrix') {
    rejectUnknownOptions(options, new Set(), 'build-weavatrix');
    process.stdout.write(`${JSON.stringify({ built: buildWeavatrix() })}\n`);
  } else if (command === 'install-atomwrite') {
    rejectUnknownOptions(options, new Set(), 'install-atomwrite');
    process.stdout.write(`${JSON.stringify({ installed: installAtomwrite() })}\n`);
  } else if (command === 'refresh-machine-filesystem') {
    rejectUnknownOptions(options, new Set(['result']), 'refresh-machine-filesystem');
    if (options.result === undefined) {
      throw new Error('refresh-machine-filesystem requires --result DIR');
    }
    process.stdout.write(`${JSON.stringify(
      refreshMachineFilesystem(options.result), null, 2,
    )}\n`);
  } else if (command === 'self-check') {
    rejectUnknownOptions(options, new Set(['output']), 'self-check');
    selfCheck(options);
  } else if (command === 'run') {
    rejectUnknownOptions(options, RUN_OPTION_NAMES, 'run');
    const result = runMatrix(validateRunOptions(options));
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
    if (!result.all_gates_pass) {
      process.exitCode = 2;
    }
  } else {
    throw new Error(`unknown command: ${command}\n\n${HELP}`);
  }
}

try {
  main();
} catch (error) {
  process.stderr.write(
    `benchmark harness error: ${error instanceof Error ? error.stack : String(error)}\n`,
  );
  process.exitCode = 1;
}
