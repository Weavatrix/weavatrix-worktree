#!/usr/bin/env node

import path from 'node:path';

import {
  buildWeavatrix,
  detectAdapters,
  installAtomwrite,
} from './lib/adapters.mjs';
import { RESULT_ROOT } from './lib/constants.mjs';
import { runMatrix } from './lib/matrix.mjs';
import { HELP, parseOptions, validateRunOptions } from './lib/options.mjs';
import { refreshMachineFilesystem } from './lib/results.mjs';
import { timestampId } from './lib/util.mjs';

function selfCheck(options) {
  const config = validateRunOptions(options, {
    adapter: 'reference',
    counts: [1, 5],
    workers: [1, 2],
    modes: ['dry-run', 'durable-apply'],
    fileBytes: 4_096,
    warmups: 1,
    repetitions: 2,
    timeoutMs: 30_000,
    seed: 20_260_802,
    output: options.output === undefined
      ? path.join(RESULT_ROOT, `${timestampId()}-self-check`)
      : path.resolve(options.output),
  });
  const result = runMatrix(config);
  const expectedSamples = 2 * 2 * 2 * (1 + 2);
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
    process.stdout.write(HELP);
  } else if (command === 'detect') {
    process.stdout.write(`${JSON.stringify(detectAdapters(options), null, 2)}\n`);
  } else if (command === 'build-weavatrix') {
    process.stdout.write(`${JSON.stringify({ built: buildWeavatrix() })}\n`);
  } else if (command === 'install-atomwrite') {
    process.stdout.write(`${JSON.stringify({ installed: installAtomwrite() })}\n`);
  } else if (command === 'refresh-machine-filesystem') {
    if (options.result === undefined) {
      throw new Error('refresh-machine-filesystem requires --result DIR');
    }
    process.stdout.write(`${JSON.stringify(
      refreshMachineFilesystem(options.result), null, 2,
    )}\n`);
  } else if (command === 'self-check') {
    selfCheck(options);
  } else if (command === 'run') {
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
