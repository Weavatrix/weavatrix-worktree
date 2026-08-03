import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import path from 'node:path';

import { adapterInvocation, detectAdapters, executeAdapter } from './adapters.mjs';
import { CSV_FIELDS, RESULT_ROOT, WORK_ROOT } from './constants.mjs';
import { csvRow, machineMetadata, rowForSample, summarize } from './results.mjs';
import { correctnessGates, generateScenario, resetOwnedDirectory } from './scenario.mjs';
import { isWithin, mulberry32, shuffled, timestampId } from './util.mjs';

function ensureOutputDirectory(directory) {
  if (existsSync(directory)) {
    if (readdirSync(directory).length !== 0) {
      throw new Error(`--output directory must be absent or empty: ${directory}`);
    }
  } else {
    mkdirSync(directory, { recursive: true });
  }
}

export function configurationsFor(config) {
  const configurations = [];
  for (const workload of config.workloads) {
    for (const operationCount of config.counts) {
      // The mixed contract is deliberately one fixed ten-operation agent workload:
      // six modifications, two creations, and two deletions.
      if (workload === 'mixed' && operationCount !== 10) {
        continue;
      }
      for (const mode of config.modes) {
        for (const workers of config.workers) {
          configurations.push({
            workload,
            operationCount,
            fileBytes: config.file_bytes,
            mode,
            workers,
          });
        }
      }
    }
  }
  return configurations;
}

function outputPaths(output) {
  return {
    config: path.join(output, 'config.json'),
    machine: path.join(output, 'machine.json'),
    jsonl: path.join(output, 'samples.jsonl'),
    csv: path.join(output, 'samples.csv'),
    summary: path.join(output, 'summary.json'),
  };
}

function prepareOutput(config, runId, detection, preparedMachine) {
  const paths = outputPaths(config.output);
  writeFileSync(paths.config, `${JSON.stringify({
    schema: 'weavatrix.worktree-benchmark-config.v2',
    run_id: runId,
    ...config,
    output: config.output,
    track: 'SUBPROCESS_END_TO_END',
    atomwrite_worker_axis_collapsed: false,
    worker_axis_collapsed: config.workers.length === 1 && config.workers[0] === null,
  }, null, 2)}\n`);
  writeFileSync(paths.machine, `${JSON.stringify(
    preparedMachine ?? machineMetadata(detection, config.adapter), null, 2,
  )}\n`);
  writeFileSync(paths.jsonl, '');
  writeFileSync(paths.csv, `${CSV_FIELDS.join(',')}\n`);
  return paths;
}

function executeSample(context, configuration, warmup, iteration) {
  const sampleId = `${warmup ? 'warmup' : 'sample'}-${String(context.sequence).padStart(6, '0')}`;
  context.sequence += 1;
  const sampleRoot = path.join(context.runWorkRoot, sampleId);
  resetOwnedDirectory(sampleRoot, context.runWorkRoot);
  const scenario = generateScenario(
    sampleRoot,
    configuration.workload,
    configuration.operationCount,
    configuration.fileBytes,
  );
  const invocation = adapterInvocation(
    context.config.adapter,
    context.detection,
    scenario,
    configuration.mode,
    configuration.workers,
    context.config.timeout_ms,
  );
  const processResult = executeAdapter(invocation);
  const checked = correctnessGates(
    context.config.adapter, configuration.mode, scenario, processResult,
  );
  const row = rowForSample(
    context.runId,
    sampleId,
    context.config.adapter,
    configuration,
    warmup,
    iteration,
    processResult,
    checked,
    context.detection,
  );
  context.rows.push(row);
  appendFileSync(context.paths.jsonl, `${JSON.stringify(row)}\n`);
  appendFileSync(context.paths.csv, csvRow(row));
  if (context.config.quiet !== true) {
    process.stderr.write(
      `[${sampleId}] ${context.config.adapter} ${configuration.mode} workload=${configuration.workload} operations=${configuration.operationCount} workers=${String(configuration.workers ?? 'default')} ${row.elapsed_ms.toFixed(3)}ms gates=${row.gates.all ? 'pass' : 'FAIL'}\n`,
    );
  }
  rmSync(sampleRoot, { recursive: true, force: true });
}

export function runMatrix(config, prepared = {}) {
  mkdirSync(WORK_ROOT, { recursive: true });
  mkdirSync(RESULT_ROOT, { recursive: true });
  ensureOutputDirectory(config.output);
  const detection = prepared.detection ?? detectAdapters({
    'weavatrix-bin': config.weavatrix_bin,
    'atomwrite-bin': config.atomwrite_bin,
    'git-bin': config.git_bin,
  });
  if (!detection[config.adapter].available) {
    const hint = config.adapter === 'weavatrix'
      ? detection.weavatrix.build_command
      : config.adapter === 'atomwrite'
        ? detection.atomwrite.install_command
        : 'install Git or pass --git-bin PATH';
    throw new Error(`${config.adapter} adapter is unavailable. Prepare it with: ${hint}`);
  }
  const runId = `${timestampId()}-${config.adapter}-${process.pid}`;
  const runWorkRoot = path.join(WORK_ROOT, runId);
  mkdirSync(runWorkRoot, { recursive: true });
  const context = {
    config,
    detection,
    runId,
    runWorkRoot,
    paths: prepareOutput(config, runId, detection, prepared.machine),
    rows: [],
    sequence: 0,
  };
  const configurations = configurationsFor(config);
  if (configurations.length === 0) {
    throw new Error(
      'benchmark matrix is empty; the mixed workload requires --counts to include 10',
    );
  }
  const random = mulberry32(config.seed);
  try {
    for (let warmup = 0; warmup < config.warmups; warmup += 1) {
      for (const configuration of shuffled(configurations, random)) {
        executeSample(context, configuration, true, warmup);
      }
    }
    for (let repetition = 0; repetition < config.repetitions; repetition += 1) {
      for (const configuration of shuffled(configurations, random)) {
        executeSample(context, configuration, false, repetition);
      }
    }
    const summary = summarize(context.rows);
    writeFileSync(context.paths.summary, `${JSON.stringify(summary, null, 2)}\n`);
    return {
      run_id: runId,
      output: config.output,
      samples: context.rows.length,
      recorded_samples: context.rows.filter((row) => !row.warmup).length,
      all_gates_pass: context.rows.every((row) => row.gates.all),
      summary,
    };
  } finally {
    if (isWithin(runWorkRoot, WORK_ROOT)) {
      rmSync(runWorkRoot, { recursive: true, force: true });
    }
  }
}
