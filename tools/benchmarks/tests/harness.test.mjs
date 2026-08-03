import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { generateUnifiedPatch } from '../lib/git-patch.mjs';
import {
  applyComparisonPublicationPolicy,
  atomwriteDurableAuditSummary,
  comparisonModes,
  conservativeTwoTimesGates,
  strongerContractPerformanceFloor,
} from '../lib/comparison.mjs';
import { configurationsFor } from '../lib/matrix.mjs';
import {
  RUN_OPTION_NAMES,
  rejectUnknownOptions,
  validateRunOptions,
} from '../lib/options.mjs';
import { markdownComparisonReport } from '../lib/report.mjs';
import { summarize } from '../lib/results.mjs';
import {
  analyzeRenameGraph,
  correctnessGates,
  generateScenario,
} from '../lib/scenario.mjs';

test('Git patch generation is deterministic and carries exact context', () => {
  const patch = generateUnifiedPatch([{
    type: 'modify',
    path: 'src/example.rs',
    sourceBytes: Buffer.from('zero\nold\nlast\n'),
    outputBytes: Buffer.from('zero\nnew\nlast\n'),
  }], 1);
  assert.equal(patch, [
    'diff --git a/src/example.rs b/src/example.rs',
    '--- a/src/example.rs',
    '+++ b/src/example.rs',
    '@@ -1,3 +1,3 @@',
    ' zero',
    '-old',
    '+new',
    ' last',
    '',
  ].join('\n'));
  assert.throws(
    () => generateUnifiedPatch([{
      type: 'modify',
      path: '../escape.rs',
      sourceBytes: Buffer.from('old\n'),
      outputBytes: Buffer.from('new\n'),
    }]),
    /not safe/u,
  );
});

test('Git patches encode create, delete, and true rename operations', () => {
  const patch = generateUnifiedPatch([
    { type: 'create', path: 'src/new.rs', outputBytes: Buffer.from('new\n') },
    { type: 'delete', path: 'src/old.rs', sourceBytes: Buffer.from('old\n') },
    { type: 'rename', source: 'src/from.rs', target: 'src/to.rs' },
  ]);
  assert.match(patch, /new file mode 100644[\s\S]*--- \/dev\/null[\s\S]*\+\+\+ b\/src\/new\.rs/u);
  assert.match(patch, /deleted file mode 100644[\s\S]*\+\+\+ \/dev\/null/u);
  assert.match(patch, /similarity index 100%\nrename from src\/from\.rs\nrename to src\/to\.rs/u);
});

test('git-apply options select only its honest modes and no worker axis', () => {
  const config = validateRunOptions({ adapter: 'git-apply' });
  assert.deepEqual(config.modes, ['dry-run', 'non-durable-apply']);
  assert.deepEqual(config.requested_workers, []);
  assert.deepEqual(config.workers, [null]);
  assert.throws(
    () => validateRunOptions({ adapter: 'git-apply', workers: '1' }),
    /not supported/u,
  );
  assert.throws(
    () => validateRunOptions({ adapter: 'git-apply', modes: 'durable-apply' }),
    /does not support/u,
  );
  assert.throws(
    () => validateRunOptions({ adapter: 'weavatrix', modes: 'non-durable-apply' }),
    /does not use/u,
  );
});

test('git-apply matrix collapses workers to one N/A configuration', () => {
  const config = validateRunOptions({
    adapter: 'git-apply', workloads: 'modify,rename', counts: '1,5',
  });
  const matrix = configurationsFor(config);
  assert.equal(matrix.length, 8);
  assert.deepEqual(
    new Set(matrix.map((configuration) => configuration.workers)),
    new Set([null]),
  );
  assert.deepEqual(
    new Set(matrix.map((configuration) => configuration.mode)),
    new Set(['dry-run', 'non-durable-apply']),
  );
});

test('mixed is one ten-operation headline workload', () => {
  const config = validateRunOptions({
    adapter: 'weavatrix',
    workloads: 'mixed',
    counts: '1,5,10,64',
    workers: '1',
    modes: 'dry-run',
  });
  assert.deepEqual(configurationsFor(config), [{
    workload: 'mixed', operationCount: 10, fileBytes: 65_536,
    mode: 'dry-run', workers: 1,
  }]);
  assert.throws(
    () => validateRunOptions({ workloads: 'modify,unknown' }),
    /unsupported --workloads/u,
  );
});

test('mixed-only runs cannot silently produce an empty matrix', () => {
  assert.throws(
    () => validateRunOptions({
      adapter: 'reference', workloads: 'mixed', counts: '1,5',
      workers: '1', modes: 'dry-run',
    }),
    /mixed workload requires --counts to include 10/u,
  );
});

test('mixed and resource bounds fail before fixture allocation', () => {
  assert.throws(
    () => validateRunOptions({
      adapter: 'reference', workloads: 'modify,mixed', counts: '1,5',
      workers: '1', modes: 'dry-run',
    }),
    /mixed workload requires --counts to include 10/u,
  );
  assert.throws(
    () => validateRunOptions({ workers: '65' }),
    /must not exceed 64/u,
  );
  assert.throws(
    () => validateRunOptions({ 'file-bytes': '1048577' }),
    /must not exceed 1048576/u,
  );
});

test('unknown CLI options cannot silently fall back to benchmark defaults', () => {
  assert.throws(
    () => rejectUnknownOptions({ repetition: '30' }, RUN_OPTION_NAMES, 'run'),
    /--repetition/u,
  );
  assert.doesNotThrow(
    () => rejectUnknownOptions({ repetitions: '30' }, RUN_OPTION_NAMES, 'run'),
  );
});

test('rename chains and cycles are classified as correctness cases, not latency axes', () => {
  const chain = analyzeRenameGraph([
    { type: 'rename', source: 'a.rs', target: 'b.rs' },
    { type: 'rename', source: 'b.rs', target: 'c.rs' },
  ]);
  assert.deepEqual(chain.chains, [{ source: 'a.rs', target: 'b.rs', next: 'c.rs' }]);
  assert.deepEqual(chain.cycles, []);

  const cycle = analyzeRenameGraph([
    { type: 'rename', source: 'a.rs', target: 'b.rs' },
    { type: 'rename', source: 'b.rs', target: 'a.rs' },
  ]);
  assert.equal(cycle.chains.length, 2);
  assert.deepEqual(cycle.cycles, [['a.rs', 'b.rs']]);
});

test('functional smoke rows are never marked publication quality', () => {
  const row = {
    adapter: 'weavatrix', adapter_version: '0.2.0', track: 'SUBPROCESS_END_TO_END',
    mode: 'dry-run', durability_contract: 'READ_ONLY', workload: 'modify',
    operation_count: 1, touched_path_count: 1, file_bytes: 4096,
    workers_requested: 1, workers_effective: 1, warmup: false,
    elapsed_ms: 1, sample_id: 'sample-1', gates: { all: true },
    equivalent_to_weavatrix_recoverable_batch: true,
  };
  const [result] = summarize([row]).configurations;
  assert.equal(result.publishable, false);
  assert.equal(result.publication_sample_profile, 'FUNCTIONAL_OR_INCOMPLETE_NOT_PUBLICATION_QUALITY');
});

test('one failed sample poisons the requested configuration instead of splitting it', () => {
  const base = {
    adapter: 'weavatrix', adapter_version: '0.2.0', track: 'SUBPROCESS_END_TO_END',
    mode: 'dry-run', durability_contract: 'READ_ONLY', workload: 'modify',
    operation_count: 1, touched_path_count: 1, file_bytes: 4096,
    workers_requested: 1, workers_effective: 1, elapsed_ms: 1,
    gates: { all: true }, equivalent_to_weavatrix_recoverable_batch: true,
  };
  const rows = Array.from({ length: 35 }, (_, index) => ({
    ...base, sample_id: `ok-${index}`, warmup: index < 5,
  }));
  rows.push({
    ...base,
    sample_id: 'failed',
    warmup: false,
    durability_contract: 'UNKNOWN_BECAUSE_ADAPTER_FAILED',
    workers_effective: null,
    gates: { all: false },
    equivalent_to_weavatrix_recoverable_batch: false,
  });
  const summary = summarize(rows);
  assert.equal(summary.configurations.length, 1);
  assert.equal(summary.configurations[0].publishable, false);
  assert.deepEqual(summary.configurations[0].failed_sample_ids, ['failed']);
  assert.equal(summary.configurations[0].adapter_metadata_consistent, false);
  assert.deepEqual(summary.configurations[0].median_ci95_ms.lower_ms, 1);
  assert.deepEqual(summary.configurations[0].median_ci95_ms.upper_ms, 1);
});

test('two-times gate excludes faster but contract-incomparable baselines', () => {
  const common = {
    publishable: true, track: 'SUBPROCESS_END_TO_END', mode: 'durable-apply',
    workload: 'modify', operation_count: 10, touched_path_count: 10,
    file_bytes: 65_536, workers_effective: 4, p25_ms: 20, p75_ms: 10,
  };
  const gate = conservativeTwoTimesGates([
    {
      ...common, adapter: 'weavatrix', durability_contract: 'RECOVERABLE',
      equivalent_comparison_eligible: true,
    },
    {
      ...common, adapter: 'atomwrite', durability_contract: 'COMPENSATING',
      equivalent_comparison_eligible: false,
    },
  ]);
  assert.deepEqual(gate.eligible_rows, []);
  assert.equal(gate.all_eligible_rows_pass, false);

  const floor = strongerContractPerformanceFloor([
    {
      ...common, adapter: 'weavatrix', mode: 'dry-run',
      durability_contract: 'FULL_SHA256_CAS', equivalent_comparison_eligible: false,
      p75_ms: 10,
    },
    {
      ...common, adapter: 'atomwrite', mode: 'dry-run',
      durability_contract: 'EXACT_REPLACE_NO_FULL_CAS',
      equivalent_comparison_eligible: false, p25_ms: 25,
    },
  ]);
  assert.equal(floor.eligible_rows.length, 1);
  assert.equal(floor.eligible_rows[0].ratio, 2.5);
  assert.equal(floor.eligible_rows[0].passes_two_times_floor, true);
  assert.equal(floor.eligible_rows[0].equivalent_contracts, false);
  assert.equal(floor.universal_ranking, false);
});

test('atomwrite durable comparison is a separate non-publishable audit profile', () => {
  assert.deepEqual(comparisonModes('publication', 'atomwrite'), ['dry-run']);
  assert.deepEqual(
    comparisonModes('atomwrite-durable-audit', 'atomwrite'),
    ['durable-apply'],
  );
  assert.throws(() => comparisonModes('unknown', 'atomwrite'), /comparison-profile/u);

  const [configuration] = applyComparisonPublicationPolicy(
    'atomwrite-durable-audit',
    [{
      adapter: 'atomwrite', mode: 'durable-apply', publishable: true,
      equivalent_comparison_eligible: true,
    }],
  );
  assert.equal(configuration.publishable, false);
  assert.equal(configuration.equivalent_comparison_eligible, false);
  assert.equal(configuration.latency_statistics_class, 'DIAGNOSTIC_ONLY_NON_PUBLISHABLE');

  const audit = atomwriteDurableAuditSummary([{
    adapter: 'atomwrite', mode: 'durable-apply', warmup: false,
    gates: { tree_state: true, artifact_cleanup: false },
    unexpected_artifacts: [{ path: 'src/example.rs.bak.20260802' }],
  }]);
  assert.equal(audit.contract_defect_reproduced, true);
  assert.equal(audit.rollback_backup_artifact_samples, 1);
  assert.equal(audit.external_cleanup_performed, false);
});

test('comparison report keeps strict and stronger-contract claims separate', () => {
  const report = markdownComparisonReport({
    interleaved_tool_order: true,
    universal_ranking: false,
    configurations: [{
      adapter: 'weavatrix', adapter_version: '0.2.0', mode: 'dry-run',
      workload: 'modify', operation_count: 1, touched_path_count: 1,
      workers_requested: 1, workers_effective: 1, recorded_samples: 30,
      valid_samples: 30, p50_ms: 1.25, p95_ms: 1.75,
      all_correctness_gates_pass: true, publishable: true,
    }],
    two_times_gate: { eligible_rows: [], all_eligible_rows_pass: false },
    stronger_contract_performance_floor: {
      eligible_rows: [], all_eligible_rows_pass: false,
    },
  }, {
    run_id: 'test', comparison_profile: 'publication', warmups: 5,
    repetitions: 30, file_bytes: 65_536,
  });
  assert.match(report, /no strict 2× claim is eligible/u);
  assert.match(report, /Contracts equivalent: false/u);
  assert.match(report, /weavatrix 0\.2\.0/u);
});

test('artifact cleanup gate sees unexpected empty directories', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'weavatrix-bench-test-'));
  try {
    const scenario = generateScenario(root, 'create', 1, 1024);
    mkdirSync(path.join(scenario.workspace, 'leftover-stage'));
    const checked = correctnessGates('git-apply', 'dry-run', scenario, {
      exitCode: 0,
      error: null,
      adapterResult: {
        schema: 'weavatrix.worktree-benchmark-adapter.v2',
        ok: true,
        operations: 1,
        touched_paths: 1,
      },
    });
    assert.equal(checked.gates.artifact_cleanup, false);
    assert.deepEqual(
      checked.unexpectedArtifacts.map((entry) => entry.path),
      ['leftover-stage'],
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('the Weavatrix persistent lock allowance is type-strict', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'weavatrix-bench-lock-type-'));
  try {
    const scenario = generateScenario(root, 'create', 1, 1024);
    mkdirSync(path.join(scenario.workspace, '.weavatrix', 'worktree', 'lock'), {
      recursive: true,
    });
    const checked = correctnessGates('weavatrix', 'dry-run', scenario, {
      exitCode: 0,
      error: null,
      adapterResult: {
        schema: 'weavatrix.worktree-benchmark-adapter.v2',
        ok: true,
        operations: 1,
        touched_paths: 1,
        effective_max_files: 64,
        effective_max_paths: 64,
      },
    });
    assert.equal(checked.gates.artifact_cleanup, false);
    assert.deepEqual(
      checked.unexpectedArtifacts.map((entry) => [entry.path, entry.type]),
      [['.weavatrix/worktree/lock', 'directory']],
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('rename64 is 64 operations and an explicit 128-path budget', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'weavatrix-bench-rename64-'));
  try {
    const scenario = generateScenario(root, 'rename', 64, 1024);
    assert.equal(scenario.manifest.operation_count, 64);
    assert.equal(scenario.manifest.touched_path_count, 128);
    assert.equal(scenario.expected.size, 129);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
