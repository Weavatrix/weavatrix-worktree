import {
  existsSync,
  readFileSync,
  readdirSync,
  statfsSync,
  writeFileSync,
} from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import { adapterVersion } from './adapters.mjs';
import { CSV_FIELDS, SAMPLE_SCHEMA, WORK_ROOT } from './constants.mjs';
import { commandOutput, sha256 } from './util.mjs';

function csvEscape(value) {
  if (value === null || value === undefined) {
    return '';
  }
  const text = String(value);
  return /[",\r\n]/u.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
}

export function csvRow(row) {
  return `${CSV_FIELDS.map((field) => csvEscape(row[field])).join(',')}\n`;
}

export function rowForSample(
  runId,
  sampleId,
  adapter,
  configuration,
  warmup,
  iteration,
  processResult,
  checked,
  detection,
) {
  const adapterResult = processResult.adapterResult;
  return {
    schema: SAMPLE_SCHEMA,
    run_id: runId,
    sample_id: sampleId,
    timestamp_utc: new Date().toISOString(),
    adapter,
    adapter_version: adapterVersion(adapter, detection),
    track: 'SUBPROCESS_END_TO_END',
    mode: configuration.mode,
    durability_contract: adapterResult?.durability_contract ?? 'UNKNOWN_BECAUSE_ADAPTER_FAILED',
    equivalent_to_weavatrix_recoverable_batch:
      adapterResult?.equivalent_to_weavatrix_recoverable_batch === true,
    workload: configuration.workload,
    operation_count: configuration.operationCount,
    touched_path_count: checked.touchedPathCount,
    file_bytes: configuration.fileBytes,
    workers_requested: configuration.workers,
    workers_effective: adapterResult?.workers_effective ?? null,
    effective_max_files: adapterResult?.effective_max_files ?? null,
    effective_max_paths: adapterResult?.effective_max_paths ?? null,
    warmup,
    iteration,
    elapsed_ns: processResult.elapsedNs,
    elapsed_ms: processResult.elapsedMs,
    exit_code: processResult.exitCode,
    signal: processResult.signal,
    timed_out: processResult.timedOut,
    process_error: processResult.error,
    stdout_sha256: processResult.stdoutSha256,
    stderr: processResult.stderr,
    adapter_result: adapterResult,
    gates: checked.gates,
    gate_adapter_exit: checked.gates.adapter_exit,
    gate_adapter_json: checked.gates.adapter_json,
    gate_adapter_report_count: checked.gates.adapter_report_count,
    gate_adapter_resource_budget: checked.gates.adapter_resource_budget,
    gate_tree_state: checked.gates.tree_state,
    gate_artifact_cleanup: checked.gates.artifact_cleanup,
    gate_all: checked.gates.all,
    state_failures: checked.stateFailures,
    expected_tree: checked.expectedTree,
    observed_entries: checked.observed,
    observed_entry_count: checked.observed.length,
    allowed_persistent_artifacts: checked.allowedArtifacts,
    unexpected_artifacts: checked.unexpectedArtifacts,
    unexpected_artifact_count: checked.unexpectedArtifacts.length,
  };
}

function median(values) {
  if (values.length === 0) {
    return null;
  }
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[middle - 1] + sorted[middle]) / 2
    : sorted[middle];
}

function nearestRank(values, percentile) {
  if (values.length === 0) {
    return null;
  }
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.max(0, Math.ceil(percentile * sorted.length) - 1)];
}

function deterministicRandom(label) {
  let state = Number.parseInt(sha256(label).slice(0, 8), 16) || 1;
  return () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return (state >>> 0) / 4_294_967_296;
  };
}

function bootstrapMedianCi(values, label, resamples = 2_000) {
  if (values.length < 20) return null;
  const random = deterministicRandom(label);
  const medians = [];
  for (let sample = 0; sample < resamples; sample += 1) {
    const resampled = Array.from(
      { length: values.length },
      () => values[Math.floor(random() * values.length)],
    );
    medians.push(median(resampled));
  }
  return {
    lower_ms: nearestRank(medians, 0.025),
    upper_ms: nearestRank(medians, 0.975),
    confidence: 0.95,
    method: 'DETERMINISTIC_NONPARAMETRIC_BOOTSTRAP_MEDIAN_PERCENTILE',
    resamples,
  };
}

export function summarize(rows) {
  const groups = new Map();
  for (const row of rows) {
    const key = JSON.stringify([
      row.adapter, row.adapter_version, row.track, row.mode,
      row.workload, row.operation_count, row.file_bytes, row.workers_requested,
    ]);
    if (!groups.has(key)) {
      groups.set(key, []);
    }
    groups.get(key).push(row);
  }
  const configurations = [];
  for (const samples of groups.values()) {
    const warmups = samples.filter((sample) => sample.warmup);
    const recorded = samples.filter((sample) => !sample.warmup);
    const valid = recorded.filter((sample) => sample.gates.all);
    const elapsed = valid.map((sample) => sample.elapsed_ms);
    const p50 = median(elapsed);
    const deviations = p50 === null ? [] : elapsed.map((value) => Math.abs(value - p50));
    const allPass = samples.every((sample) => sample.gates.all);
    const standardPublicationProfile = warmups.length >= 5 && recorded.length >= 30;
    const first = recorded[0] ?? warmups[0];
    const durabilityContracts = [...new Set(
      samples.map((sample) => sample.durability_contract),
    )].sort();
    const effectiveWorkers = [...new Set(
      samples.map((sample) => sample.workers_effective),
    )].sort((left, right) => Number(left ?? -1) - Number(right ?? -1));
    const effectiveMaxFiles = [...new Set(
      samples.map((sample) => sample.effective_max_files),
    )].sort((left, right) => Number(left ?? -1) - Number(right ?? -1));
    const effectiveMaxPaths = [...new Set(
      samples.map((sample) => sample.effective_max_paths),
    )].sort((left, right) => Number(left ?? -1) - Number(right ?? -1));
    const metadataConsistent = durabilityContracts.length === 1
      && effectiveWorkers.length === 1
      && effectiveMaxFiles.length === 1
      && effectiveMaxPaths.length === 1;
    const medianCi95 = bootstrapMedianCi(elapsed, JSON.stringify([
      first.adapter, first.adapter_version, first.track, first.mode,
      first.workload, first.operation_count, first.file_bytes, first.workers_requested,
    ]));
    configurations.push({
      adapter: first.adapter,
      adapter_version: first.adapter_version,
      track: first.track,
      mode: first.mode,
      durability_contract: metadataConsistent ? durabilityContracts[0] : null,
      durability_contracts_observed: durabilityContracts,
      workload: first.workload,
      operation_count: first.operation_count,
      touched_path_count: first.touched_path_count,
      file_bytes: first.file_bytes,
      workers_requested: first.workers_requested,
      workers_effective: metadataConsistent ? effectiveWorkers[0] : null,
      workers_effective_observed: effectiveWorkers,
      effective_max_files: effectiveMaxFiles.length === 1 ? effectiveMaxFiles[0] : null,
      effective_max_files_observed: effectiveMaxFiles,
      effective_max_paths: effectiveMaxPaths.length === 1 ? effectiveMaxPaths[0] : null,
      effective_max_paths_observed: effectiveMaxPaths,
      adapter_metadata_consistent: metadataConsistent,
      warmup_samples: warmups.length,
      recorded_samples: recorded.length,
      valid_samples: valid.length,
      failed_sample_ids: samples
        .filter((sample) => !sample.gates.all).map((sample) => sample.sample_id),
      all_correctness_gates_pass: allPass,
      publication_sample_profile: standardPublicationProfile
        ? 'STANDARD_5_WARMUPS_30_RECORDED'
        : 'FUNCTIONAL_OR_INCOMPLETE_NOT_PUBLICATION_QUALITY',
      publishable: first.adapter !== 'reference'
        && allPass && standardPublicationProfile && metadataConsistent && medianCi95 !== null,
      equivalent_comparison_eligible: first.adapter !== 'reference'
        && allPass
        && standardPublicationProfile && metadataConsistent && medianCi95 !== null
        && recorded.every((sample) => sample.equivalent_to_weavatrix_recoverable_batch),
      p25_ms: valid.length >= 4 ? nearestRank(elapsed, 0.25) : null,
      p50_ms: p50,
      p75_ms: valid.length >= 4 ? nearestRank(elapsed, 0.75) : null,
      p95_ms: valid.length >= 20 ? nearestRank(elapsed, 0.95) : null,
      median_ci95_ms: medianCi95,
      mad_ms: median(deviations),
      min_ms: elapsed.length > 0 ? Math.min(...elapsed) : null,
      max_ms: elapsed.length > 0 ? Math.max(...elapsed) : null,
    });
  }
  configurations.sort((left, right) => JSON.stringify(left).localeCompare(JSON.stringify(right)));
  return {
    schema: 'weavatrix.worktree-benchmark-summary.v2',
    generated_at_utc: new Date().toISOString(),
    claim_scope: 'RECORDED_MACHINE_FILESYSTEM_TOOL_VERSIONS_AND_EXACT_MODES_ONLY',
    universal_ranking: false,
    configurations,
  };
}

function windowsVolume(root) {
  if (process.platform !== 'win32') {
    return null;
  }
  const match = path.parse(path.resolve(root)).root.match(/^([A-Za-z]):\\$/u);
  if (match === null) {
    return null;
  }
  const script = [
    `$volume = Get-Volume -DriveLetter '${match[1]}' -ErrorAction Stop`,
    '$volume | Select-Object DriveLetter,FileSystem,FileSystemLabel,DriveType,HealthStatus,OperationalStatus,Size,SizeRemaining | ConvertTo-Json -Compress',
  ].join('; ');
  const output = commandOutput(
    'powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], 30_000,
  );
  if (output === null) {
    return null;
  }
  try {
    return JSON.parse(output);
  } catch {
    return null;
  }
}

function filesystemMetadata(root) {
  const probeRoot = path.resolve(root);
  try {
    const value = statfsSync(probeRoot, { bigint: true });
    const raw = Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, item.toString()]),
    );
    const volume = windowsVolume(probeRoot);
    return {
      probe_root: probeRoot,
      detected_name: volume?.FileSystem ?? null,
      detection_source: volume === null
        ? 'node:fs.statfsSync numeric type only'
        : 'node:fs.statfsSync + PowerShell Get-Volume',
      numeric_type: raw.type,
      block_size_bytes: raw.bsize,
      blocks_total: raw.blocks,
      blocks_free: raw.bfree,
      blocks_available: raw.bavail,
      file_nodes_total: raw.files,
      file_nodes_free: raw.ffree,
      volume,
    };
  } catch (error) {
    return {
      probe_root: probeRoot,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

function harnessSourceFingerprint() {
  const root = path.dirname(WORK_ROOT);
  const excluded = new Set(['.tools', '.work', 'results']);
  const extensions = new Set(['.js', '.json', '.lock', '.md', '.mjs', '.rs', '.toml']);
  const files = [];
  function visit(directory, relativeDirectory = '') {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (entry.isDirectory() && excluded.has(entry.name)) continue;
      const relative = path.posix.join(relativeDirectory, entry.name);
      const absolute = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        visit(absolute, relative);
      } else if (entry.isFile() && extensions.has(path.extname(entry.name))) {
        files.push({ relative, bytes: readFileSync(absolute) });
      }
    }
  }
  visit(root);
  files.sort((left, right) => left.relative.localeCompare(right.relative));
  return sha256(Buffer.concat(files.flatMap(({ relative, bytes }) => [
    Buffer.from(`${relative}\0`, 'utf8'), bytes, Buffer.from('\0', 'utf8'),
  ])));
}

export function machineMetadata(detection, adapter) {
  const cpu = os.cpus();
  const gitStatus = commandOutput('git', ['status', '--short'], 30_000);
  return {
    schema: 'weavatrix.worktree-benchmark-machine.v1',
    captured_at_utc: new Date().toISOString(),
    platform: os.platform(),
    release: os.release(),
    os_version: os.version(),
    architecture: os.arch(),
    hostname: os.hostname(),
    endianness: os.endianness(),
    cpu_model: cpu[0]?.model ?? null,
    logical_cpu_count: cpu.length,
    total_memory_bytes: os.totalmem(),
    free_memory_bytes_at_capture: os.freemem(),
    node_version: process.version,
    v8_version: process.versions.v8,
    rustc: commandOutput('rustc', ['-vV'], 30_000),
    cargo: commandOutput('cargo', ['--version'], 30_000),
    git: commandOutput('git', ['--version'], 30_000),
    git_head: commandOutput('git', ['rev-parse', 'HEAD'], 30_000),
    git_dirty: gitStatus !== null && gitStatus.length > 0,
    git_status_sha256: gitStatus === null ? null : sha256(gitStatus),
    benchmark_harness_source_sha256: harnessSourceFingerprint(),
    active_power_scheme: process.platform === 'win32'
      ? commandOutput('powercfg.exe', ['/getactivescheme'], 30_000)
      : null,
    filesystem: filesystemMetadata(WORK_ROOT),
    adapter: detection[adapter],
    environment_disclosure: 'NO_GENERAL_ENVIRONMENT_VARIABLE_DUMP',
  };
}

export function refreshMachineFilesystem(resultDirectory) {
  const machinePath = path.join(path.resolve(resultDirectory), 'machine.json');
  if (!existsSync(machinePath)) {
    throw new Error(`machine metadata does not exist: ${machinePath}`);
  }
  const metadata = JSON.parse(readFileSync(machinePath, 'utf8'));
  const recordedRoot = metadata.filesystem?.probe_root;
  if (metadata.platform !== os.platform()
    || metadata.architecture !== os.arch()
    || metadata.hostname !== os.hostname()
    || typeof recordedRoot !== 'string'
    || recordedRoot.length === 0) {
    throw new Error('refusing filesystem refresh on a different or unproven machine');
  }
  metadata.filesystem = filesystemMetadata(recordedRoot);
  metadata.filesystem_refreshed_at_utc = new Date().toISOString();
  writeFileSync(machinePath, `${JSON.stringify(metadata, null, 2)}\n`, 'utf8');
  return { machine: machinePath, filesystem: metadata.filesystem };
}
