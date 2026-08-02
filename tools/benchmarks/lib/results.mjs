import {
  existsSync,
  readFileSync,
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
    file_count: configuration.fileCount,
    file_bytes: configuration.fileBytes,
    workers_requested: configuration.workers,
    workers_effective: adapterResult?.workers_effective ?? null,
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
    gate_content_hashes: checked.gates.content_hashes,
    gate_artifact_cleanup: checked.gates.artifact_cleanup,
    gate_all: checked.gates.all,
    hash_failures: checked.hashFailures,
    observed_files: checked.observed,
    observed_file_count: checked.observed.length,
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

export function summarize(rows) {
  const groups = new Map();
  for (const row of rows.filter((sample) => !sample.warmup)) {
    const key = JSON.stringify([
      row.adapter, row.mode, row.file_count, row.file_bytes, row.workers_requested,
    ]);
    if (!groups.has(key)) {
      groups.set(key, []);
    }
    groups.get(key).push(row);
  }
  const configurations = [];
  for (const samples of groups.values()) {
    const valid = samples.filter((sample) => sample.gates.all);
    const elapsed = valid.map((sample) => sample.elapsed_ms);
    const p50 = median(elapsed);
    const deviations = p50 === null ? [] : elapsed.map((value) => Math.abs(value - p50));
    const allPass = valid.length === samples.length;
    const first = samples[0];
    configurations.push({
      adapter: first.adapter,
      mode: first.mode,
      file_count: first.file_count,
      file_bytes: first.file_bytes,
      workers_requested: first.workers_requested,
      recorded_samples: samples.length,
      valid_samples: valid.length,
      failed_sample_ids: samples
        .filter((sample) => !sample.gates.all).map((sample) => sample.sample_id),
      all_correctness_gates_pass: allPass,
      publishable: first.adapter !== 'reference' && allPass,
      equivalent_comparison_eligible: first.adapter !== 'reference'
        && allPass
        && samples.every((sample) => sample.equivalent_to_weavatrix_recoverable_batch),
      p50_ms: p50,
      p95_ms: valid.length >= 20 ? nearestRank(elapsed, 0.95) : null,
      mad_ms: median(deviations),
      min_ms: elapsed.length > 0 ? Math.min(...elapsed) : null,
      max_ms: elapsed.length > 0 ? Math.max(...elapsed) : null,
    });
  }
  configurations.sort((left, right) => JSON.stringify(left).localeCompare(JSON.stringify(right)));
  return {
    schema: 'weavatrix.worktree-benchmark-summary.v1',
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
  try {
    const value = statfsSync(root, { bigint: true });
    const raw = Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, item.toString()]),
    );
    const volume = windowsVolume(root);
    return {
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
    return { error: error instanceof Error ? error.message : String(error) };
  }
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
  metadata.filesystem = filesystemMetadata(WORK_ROOT);
  metadata.filesystem_refreshed_at_utc = new Date().toISOString();
  writeFileSync(machinePath, `${JSON.stringify(metadata, null, 2)}\n`, 'utf8');
  return { machine: machinePath, filesystem: metadata.filesystem };
}
