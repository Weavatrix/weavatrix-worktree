import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync } from 'node:fs';
import path from 'node:path';

import {
  ATOMWRITE_ADAPTER,
  LOCAL_ATOMWRITE,
  LOCAL_ATOMWRITE_ROOT,
  LOCAL_ATOMWRITE_TARGET,
  LOCAL_WEAVATRIX,
  REFERENCE_ADAPTER,
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
    version: result.status === 0 ? result.stdout.trim() : null,
    error: result.error?.message ?? (result.status === 0 ? null : result.stderr.trim()),
  };
}

export function detectAdapters(options = {}) {
  const weavatrixBinary = resolveBinary(
    options['weavatrix-bin'], LOCAL_WEAVATRIX, 'weavatrix-worktree-bench-adapter',
  );
  const atomwriteBinary = resolveBinary(options['atomwrite-bin'], LOCAL_ATOMWRITE, 'atomwrite');
  return {
    reference: {
      available: existsSync(REFERENCE_ADAPTER),
      binary: process.execPath,
      script: REFERENCE_ADAPTER,
      version: commandOutput(process.execPath, [REFERENCE_ADAPTER, '--version']),
      publishable: false,
    },
    weavatrix: {
      ...probeBinary(weavatrixBinary),
      build_command: `cargo build --release --locked --manifest-path "${WEAVATRIX_MANIFEST}" --target-dir "${WEAVATRIX_TARGET}"`,
      worker_control: true,
    },
    atomwrite: {
      ...probeBinary(atomwriteBinary),
      install_command: `cargo install atomwrite --version 0.1.35 --locked --root "${LOCAL_ATOMWRITE_ROOT}"`,
      worker_control: true,
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
    return { command: detection.weavatrix.binary, args: common, timeoutMs };
  }
  if (adapter === 'atomwrite') {
    return {
      command: process.execPath,
      args: [ATOMWRITE_ADAPTER, '--atomwrite-bin', detection.atomwrite.binary, ...common],
      timeoutMs,
    };
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
    'install', 'atomwrite', '--version', '0.1.35', '--locked',
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
