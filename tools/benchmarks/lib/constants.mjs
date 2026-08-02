import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const SCRIPT_DIR = path.dirname(fileURLToPath(new URL('../run.mjs', import.meta.url)));
export const REPO_ROOT = path.resolve(SCRIPT_DIR, '..', '..');
export const WORK_ROOT = path.join(SCRIPT_DIR, '.work');
export const RESULT_ROOT = path.join(SCRIPT_DIR, 'results');
export const REFERENCE_ADAPTER = path.join(SCRIPT_DIR, 'adapters', 'reference.mjs');
export const ATOMWRITE_ADAPTER = path.join(SCRIPT_DIR, 'adapters', 'atomwrite.mjs');
export const WEAVATRIX_MANIFEST = path.join(SCRIPT_DIR, 'weavatrix-adapter', 'Cargo.toml');
export const LOCAL_ATOMWRITE_ROOT = path.join(SCRIPT_DIR, '.tools', 'atomwrite');
export const LOCAL_ATOMWRITE_TARGET = path.join(SCRIPT_DIR, '.tools', 'atomwrite-target');
export const LOCAL_ATOMWRITE = path.join(
  LOCAL_ATOMWRITE_ROOT,
  'bin',
  process.platform === 'win32' ? 'atomwrite.exe' : 'atomwrite',
);
export const WEAVATRIX_TARGET = path.join(SCRIPT_DIR, '.tools', 'weavatrix-target');
export const LOCAL_WEAVATRIX = path.join(
  WEAVATRIX_TARGET,
  'release',
  process.platform === 'win32'
    ? 'weavatrix-worktree-bench-adapter.exe'
    : 'weavatrix-worktree-bench-adapter',
);

export const SAMPLE_SCHEMA = 'weavatrix.worktree-benchmark-sample.v1';
export const MANIFEST_SCHEMA = 'weavatrix.worktree-benchmark-manifest.v1';
export const ADAPTER_SCHEMA = 'weavatrix.worktree-benchmark-adapter.v1';
export const ALLOWED_COUNTS = new Set([1, 5, 10, 64]);
export const ALLOWED_MODES = new Set(['dry-run', 'durable-apply']);
export const CSV_FIELDS = [
  'schema', 'run_id', 'sample_id', 'timestamp_utc', 'adapter', 'adapter_version',
  'track', 'mode', 'durability_contract', 'equivalent_to_weavatrix_recoverable_batch',
  'file_count', 'file_bytes', 'workers_requested', 'workers_effective', 'warmup',
  'iteration', 'elapsed_ns', 'elapsed_ms', 'exit_code', 'signal', 'timed_out',
  'gate_adapter_exit', 'gate_adapter_json', 'gate_adapter_report_count',
  'gate_content_hashes', 'gate_artifact_cleanup', 'gate_all',
  'unexpected_artifact_count', 'observed_file_count',
];
