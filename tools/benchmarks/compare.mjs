#!/usr/bin/env node

import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from 'node:fs';
import path from 'node:path';

import {
  applyComparisonPublicationPolicy,
  atomwriteDurableAuditSummary,
  comparisonModes,
  conservativeTwoTimesGates,
  strongerContractPerformanceFloor,
} from './lib/comparison.mjs';
import { CSV_FIELDS, RESULT_ROOT } from './lib/constants.mjs';
import { detectAdapters } from './lib/adapters.mjs';
import { runMatrix } from './lib/matrix.mjs';
import {
  RUN_OPTION_NAMES,
  parseOptions,
  rejectUnknownOptions,
  validateRunOptions,
} from './lib/options.mjs';
import { csvRow, machineMetadata, summarize } from './lib/results.mjs';
import { markdownComparisonReport } from './lib/report.mjs';
import { mulberry32, shuffled, timestampId } from './lib/util.mjs';

const ADAPTERS = ['weavatrix', 'atomwrite', 'git-apply'];
const COMPARISON_OPTIONS = new Set([
  ...RUN_OPTION_NAMES,
  'adapters',
  'comparison-profile',
]);
COMPARISON_OPTIONS.delete('adapter');
COMPARISON_OPTIONS.delete('modes');

const HELP = `Usage:
  node tools/benchmarks/compare.mjs [options]

Options:
  --comparison-profile NAME  publication (default) or atomwrite-durable-audit
  --adapters LIST            at least two from weavatrix,atomwrite,git-apply
  --workloads LIST           modify,create,delete,rename,mixed
  --counts LIST              comma list from 1,5,10,64
  --workers LIST             comma list up to 64 for worker-aware adapters
  --file-bytes N             1024..1048576
  --warmups N                default 5
  --repetitions N            default 30
  --timeout-ms N             default 120000
  --seed N                   deterministic schedule seed
  --output DIR               absent or empty result directory
  --weavatrix-bin PATH       explicit Rust adapter binary
  --atomwrite-bin PATH       explicit atomwrite 0.1.36 binary
  --git-bin PATH             explicit Git binary
`;

function requireEmptyOutput(output) {
  if (existsSync(output)) {
    if (readdirSync(output).length !== 0) {
      throw new Error(`--output directory must be absent or empty: ${output}`);
    }
  } else {
    mkdirSync(output, { recursive: true });
  }
}

function selectedAdapters(options) {
  const values = String(options.adapters ?? ADAPTERS.join(','))
    .split(',').map((value) => value.trim()).filter(Boolean);
  if (values.length < 2 || new Set(values).size !== values.length
    || values.some((value) => !ADAPTERS.includes(value))) {
    throw new Error('--adapters requires at least two unique values from weavatrix,atomwrite,git-apply');
  }
  return values;
}

function configFor(adapter, profile, options, base, output, roundSeed) {
  const adapterOptions = { ...options, adapter };
  delete adapterOptions.adapters;
  delete adapterOptions['comparison-profile'];
  delete adapterOptions.output;
  delete adapterOptions.warmups;
  delete adapterOptions.repetitions;
  if (adapter === 'git-apply') delete adapterOptions.workers;
  delete adapterOptions.modes;
  const modes = comparisonModes(profile, adapter);
  return {
    ...validateRunOptions(adapterOptions, {
    adapter,
    workloads: base.workloads,
    counts: base.counts,
    workers: adapter === 'git-apply' ? undefined : base.workers,
    modes,
    fileBytes: base.file_bytes,
    warmups: 0,
    repetitions: 1,
    timeoutMs: base.timeout_ms,
    seed: roundSeed,
      output,
    }),
    quiet: true,
  };
}

function loadRows(resultDirectory, prefix, warmup, iteration, comparisonRunId) {
  return readFileSync(path.join(resultDirectory, 'samples.jsonl'), 'utf8')
    .split(/\r?\n/u).filter(Boolean).map((line, index) => {
      const row = JSON.parse(line);
      return {
        ...row,
        run_id: comparisonRunId,
        sample_id: `${prefix}-${String(index).padStart(4, '0')}`,
        warmup,
        iteration,
      };
    });
}

function run(options) {
  const adapters = selectedAdapters(options);
  const profile = String(options['comparison-profile'] ?? 'publication');
  comparisonModes(profile, adapters[0]);
  if (profile === 'atomwrite-durable-audit'
    && (!adapters.includes('weavatrix') || !adapters.includes('atomwrite'))) {
    throw new Error(
      'atomwrite-durable-audit requires both weavatrix and atomwrite adapters',
    );
  }
  const output = path.resolve(options.output
    ?? path.join(RESULT_ROOT, `${timestampId()}-interleaved-comparison`));
  requireEmptyOutput(output);
  const base = validateRunOptions({ ...options, adapter: 'weavatrix' });
  const comparisonRunId = `${timestampId()}-interleaved-${process.pid}`;
  const config = {
    schema: 'weavatrix.worktree-benchmark-comparison.v2',
    run_id: comparisonRunId,
    comparison_profile: profile,
    adapters,
    warmups: base.warmups,
    repetitions: base.repetitions,
    workloads: base.workloads,
    counts: base.counts,
    workers: base.workers,
    file_bytes: base.file_bytes,
    timeout_ms: base.timeout_ms,
    seed: base.seed,
    modes_by_adapter: Object.fromEntries(
      adapters.map((adapter) => [adapter, comparisonModes(profile, adapter)]),
    ),
  };
  writeFileSync(path.join(output, 'config.json'), `${JSON.stringify(config, null, 2)}\n`);
  const detection = detectAdapters({
    'weavatrix-bin': base.weavatrix_bin,
    'atomwrite-bin': base.atomwrite_bin,
    'git-bin': base.git_bin,
  });
  const captured = machineMetadata(detection, adapters[0]);
  const { adapter: _capturedAdapter, ...commonMachine } = captured;
  const machineByAdapter = Object.fromEntries(adapters.map((adapter) => [
    adapter,
    {
      ...commonMachine,
      adapter: detection[adapter],
      comparison_snapshot_reused: true,
    },
  ]));
  writeFileSync(path.join(output, 'machine.json'), `${JSON.stringify({
    ...commonMachine,
    adapters: Object.fromEntries(
      adapters.map((adapter) => [adapter, detection[adapter]]),
    ),
    comparison_snapshot_reused_by_component_runs: true,
  }, null, 2)}\n`);
  const rounds = base.warmups + base.repetitions;
  const random = mulberry32(base.seed);
  const schedule = [];
  const rows = [];
  const jsonl = path.join(output, 'samples.jsonl');
  const csv = path.join(output, 'samples.csv');
  writeFileSync(jsonl, '');
  writeFileSync(csv, `${CSV_FIELDS.join(',')}\n`);
  for (let round = 0; round < rounds; round += 1) {
    const warmup = round < base.warmups;
    const iteration = warmup ? round : round - base.warmups;
    const order = shuffled(adapters, random);
    const scheduleEntry = {
      round,
      warmup,
      iteration,
      adapter_order: order,
      adapters_completed: [],
      status: 'running',
    };
    schedule.push(scheduleEntry);
    writeFileSync(
      path.join(output, 'schedule.json'),
      `${JSON.stringify(schedule, null, 2)}\n`,
    );
    process.stderr.write(
      `[comparison round ${round + 1}/${rounds}] ${warmup ? 'warmup' : 'recorded'} order=${order.join(',')}\n`,
    );
    for (let orderIndex = 0; orderIndex < order.length; orderIndex += 1) {
      const adapter = order[orderIndex];
      const roundOutput = path.join(
        output, 'rounds', String(round).padStart(2, '0'), adapter,
      );
      const result = runMatrix(
        configFor(
          adapter,
          profile,
          options,
          base,
          roundOutput,
          base.seed + round * 101 + orderIndex,
        ),
        { detection, machine: machineByAdapter[adapter] },
      );
      const prefix = `${warmup ? 'warmup' : 'sample'}-${String(round).padStart(2, '0')}-${adapter}`;
      const roundRows = loadRows(
        result.output, prefix, warmup, iteration, comparisonRunId,
      );
      rows.push(...roundRows);
      for (const row of roundRows) {
        appendFileSync(jsonl, `${JSON.stringify(row)}\n`);
        appendFileSync(csv, csvRow(row));
      }
      scheduleEntry.adapters_completed.push(adapter);
      writeFileSync(
        path.join(output, 'schedule.json'),
        `${JSON.stringify(schedule, null, 2)}\n`,
      );
    }
    scheduleEntry.status = 'complete';
    writeFileSync(
      path.join(output, 'schedule.json'),
      `${JSON.stringify(schedule, null, 2)}\n`,
    );
  }
  const summary = summarize(rows);
  summary.configurations = applyComparisonPublicationPolicy(
    profile,
    summary.configurations,
  );
  summary.comparison_profile = profile;
  summary.interleaved_tool_order = true;
  summary.two_times_gate = conservativeTwoTimesGates(summary.configurations);
  summary.stronger_contract_performance_floor = strongerContractPerformanceFloor(
    summary.configurations,
  );
  if (profile === 'atomwrite-durable-audit') {
    summary.atomwrite_durable_audit = atomwriteDurableAuditSummary(rows);
  }
  writeFileSync(path.join(output, 'summary.json'), `${JSON.stringify(summary, null, 2)}\n`);
  writeFileSync(path.join(output, 'report.md'), markdownComparisonReport(summary, config));
  return {
    run_id: comparisonRunId,
    output,
    comparison_profile: profile,
    samples: rows.length,
    all_gates_pass: rows.every((row) => row.gates.all),
    publishable_configurations: summary.configurations.filter((row) => row.publishable).length,
    two_times_eligible_rows: summary.two_times_gate.eligible_rows.length,
    stronger_contract_floor_rows:
      summary.stronger_contract_performance_floor.eligible_rows.length,
  };
}

try {
  const argv = process.argv.slice(2);
  if (argv.length === 1 && ['--help', '-h'].includes(argv[0])) {
    process.stdout.write(HELP);
  } else {
    const options = parseOptions(argv);
    rejectUnknownOptions(options, COMPARISON_OPTIONS, 'comparison');
    const result = run(options);
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
    if (!result.all_gates_pass) process.exitCode = 2;
  }
} catch (error) {
  process.stderr.write(`comparison harness error: ${error instanceof Error ? error.stack : String(error)}\n`);
  process.exitCode = 1;
}
