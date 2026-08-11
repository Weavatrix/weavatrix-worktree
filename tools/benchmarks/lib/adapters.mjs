import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

import {
  ATOMWRITE_ADAPTER,
  GIT_APPLY_ADAPTER,
  LOCAL_ATOMWRITE,
  LOCAL_ATOMWRITE_ROOT,
  LOCAL_ATOMWRITE_TARGET,
  LOCAL_WEAVATRIX,
  REFERENCE_ADAPTER,
  REPO_ROOT,
  WEAVATRIX_ADAPTER,
  WEAVATRIX_MANIFEST,
  WEAVATRIX_TARGET,
} from './constants.mjs';
import { commandOutput, sha256 } from './util.mjs';

function findOnPath(command) {
  const locator = process.platform === 'win32' ? 'where.exe' : 'which';
  const output = commandOutput(locator, [command]);
  return output?.split(/\r?\n/u).find((line) => line.trim().length > 0)?.trim() ?? null;
}

function resolveBinary(explicit, local, pathCommand) {
  if (explicit !== undefined) {
    const resolved = path.resolve(explicit);
    return existsSync(resolved) ? resolved : null;
  }
  return existsSync(local) ? local : findOnPath(pathCommand);
}

function fileSha256(file) {
  try {
    return file === null ? null : sha256(readFileSync(file));
  } catch {
    return null;
  }
}

function probeBinary(binary, args = ['--version']) {
  if (binary === null) {
    return { available: false, binary: null, version: null, error: 'not found' };
  }
  const result = spawnSync(binary, args, {
    encoding: 'utf8',
    timeout: 30_000,
    windowsHide: true,
  });
  return {
    available: !result.error && result.status === 0,
    binary,
    binary_sha256: fileSha256(binary),
    version: result.status === 0 ? result.stdout.trim() : null,
    error: result.error?.message ?? (result.status === 0 ? null : result.stderr.trim()),
  };
}

function rootCrateVersion() {
  const manifest = readFileSync(path.join(REPO_ROOT, 'Cargo.toml'), 'utf8');
  const version = /^version = "([^"]+)"$/mu.exec(manifest)?.[1];
  if (version === undefined) {
    throw new Error('cannot read the crate version from Cargo.toml');
  }
  return version;
}

function requireVersion(probe, label, pattern, expected) {
  if (!probe.available || probe.version === null) return probe;
  if (pattern.test(probe.version)) return probe;
  return {
    ...probe,
    available: false,
    error: `${label} version mismatch: expected ${expected}, found ${probe.version}`,
  };
}

export function detectAdapters(options = {}) {
  const weavatrixBinary = resolveBinary(
    options['weavatrix-bin'], LOCAL_WEAVATRIX, 'weavatrix-worktree-bench-adapter',
  );
  const atomwriteBinary = resolveBinary(options['atomwrite-bin'], LOCAL_ATOMWRITE, 'atomwrite');
  const gitBinary = resolveBinary(options['git-bin'], '', 'git');
  const crateVersion = rootCrateVersion();
  const weavatrix = requireVersion(
    probeBinary(weavatrixBinary),
    'weavatrix-worktree adapter',
    new RegExp(`\\(weavatrix-worktree ${crateVersion.replaceAll('.', '\\.')}\\)$`, 'u'),
    `weavatrix-worktree ${crateVersion}`,
  );
  const atomwrite = requireVersion(
    probeBinary(atomwriteBinary),
    'atomwrite',
    /^atomwrite 0\.1\.36(?:\s|$)/u,
    'atomwrite 0.1.36',
  );
  return {
    reference: {
      available: existsSync(REFERENCE_ADAPTER),
      binary: process.execPath,
      script: REFERENCE_ADAPTER,
      script_sha256: fileSha256(REFERENCE_ADAPTER),
      version: commandOutput(process.execPath, [REFERENCE_ADAPTER, '--version']),
      publishable: false,
    },
    weavatrix: {
      ...weavatrix,
      wrapper_sha256: fileSha256(WEAVATRIX_ADAPTER),
      build_command: `cargo build --release --locked --manifest-path "${WEAVATRIX_MANIFEST}" --target-dir "${WEAVATRIX_TARGET}"`,
      worker_control: true,
    },
    atomwrite: {
      ...atomwrite,
      wrapper_sha256: fileSha256(ATOMWRITE_ADAPTER),
      install_command: `cargo install atomwrite --version 0.1.36 --locked --root "${LOCAL_ATOMWRITE_ROOT}"`,
      worker_control: true,
      equivalent_to_weavatrix_recoverable_batch: false,
    },
    'git-apply': {
      ...probeBinary(gitBinary),
      wrapper_sha256: fileSha256(GIT_APPLY_ADAPTER),
      worker_control: false,
      durability: false,
      equivalent_to_weavatrix_recoverable_batch: false,
    },
  };
}

export function adapterInvocation(adapter, detection, scenario, mode, workers, timeoutMs) {
  const common = [
    '--workspace', scenario.workspace,
    '--manifest', scenario.manifestPath,
    '--mode', mode,
    '--workers', String(workers ?? 1),
  ];
  if (adapter === 'reference') {
    return { command: process.execPath, args: [REFERENCE_ADAPTER, ...common], timeoutMs };
  }
  if (adapter === 'weavatrix') {
    return {
      command: process.execPath,
      args: [
        WEAVATRIX_ADAPTER,
        '--weavatrix-bin', detection.weavatrix.binary,
        '--timeout-ms', String(timeoutMs),
        ...common,
      ],
      timeoutMs,
    };
  }
  if (adapter === 'atomwrite') {
    return {
      command: process.execPath,
      args: [
        ATOMWRITE_ADAPTER,
        '--atomwrite-bin', detection.atomwrite.binary,
        '--atomwrite-version', detection.atomwrite.version,
        '--timeout-ms', String(timeoutMs),
        ...common,
      ],
      timeoutMs,
    };
  }
  if (adapter === 'git-apply') {
    const args = [
      GIT_APPLY_ADAPTER,
      '--git-bin', detection['git-apply'].binary,
      '--workspace', scenario.workspace,
      '--repository-root', REPO_ROOT,
      '--manifest', scenario.manifestPath,
      '--patch', scenario.patchPath,
      '--mode', mode,
      '--timeout-ms', String(timeoutMs),
    ];
    return { command: process.execPath, args, timeoutMs };
  }
  throw new Error(`unsupported adapter: ${adapter}`);
}

export function executeAdapter(invocation) {
  const started = process.hrtime.bigint();
  const result = spawnSync(invocation.command, invocation.args, {
    encoding: 'utf8',
    timeout: invocation.timeoutMs,
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  const elapsed = process.hrtime.bigint() - started;
  let adapterResult = null;
  let parseError = null;
  try {
    adapterResult = JSON.parse(result.stdout.trim());
  } catch (error) {
    parseError = error instanceof Error ? error.message : String(error);
  }
  return {
    elapsedNs: elapsed.toString(),
    elapsedMs: Number(elapsed) / 1_000_000,
    exitCode: result.status,
    signal: result.signal,
    timedOut: result.error?.code === 'ETIMEDOUT',
    error: result.error?.message ?? parseError,
    stdoutSha256: sha256(result.stdout ?? ''),
    stderr: (result.stderr ?? '').slice(0, 16_384),
    adapterResult,
  };
}

export function adapterVersion(adapter, detection) {
  return adapter === 'reference' ? detection.reference.version : detection[adapter].version;
}

export function buildWeavatrix() {
  mkdirSync(path.dirname(WEAVATRIX_TARGET), { recursive: true });
  const result = spawnSync('cargo', [
    'build', '--release', '--locked',
    '--manifest-path', WEAVATRIX_MANIFEST,
    '--target-dir', WEAVATRIX_TARGET,
  ], { stdio: 'inherit', windowsHide: true });
  if (result.error || result.status !== 0) {
    throw new Error(result.error?.message ?? `cargo build exited ${String(result.status)}`);
  }
  return LOCAL_WEAVATRIX;
}

export function installAtomwrite() {
  mkdirSync(path.dirname(LOCAL_ATOMWRITE_ROOT), { recursive: true });
  const result = spawnSync('cargo', [
    'install', 'atomwrite', '--version', '0.1.36', '--locked',
    '--root', LOCAL_ATOMWRITE_ROOT,
  ], {
    stdio: 'inherit',
    windowsHide: true,
    env: { ...process.env, CARGO_TARGET_DIR: LOCAL_ATOMWRITE_TARGET },
  });
  if (result.error || result.status !== 0) {
    throw new Error(result.error?.message ?? `cargo install exited ${String(result.status)}`);
  }
  return LOCAL_ATOMWRITE;
}
