import {
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import path from 'node:path';

import { ADAPTER_SCHEMA, MANIFEST_SCHEMA } from './constants.mjs';
import { generateUnifiedPatch } from './git-patch.mjs';
import { isWithin, sha256 } from './util.mjs';

function fixedToken(prefix, width = 32) {
  if (!/^[A-Z0-9_]+$/u.test(prefix) || prefix.length > width) {
    throw new Error(`invalid fixture token prefix: ${prefix}`);
  }
  return prefix.padEnd(width, '_');
}

function fixtureLine(fileIndex, lineIndex, width) {
  const prefix = `// file=${String(fileIndex).padStart(4, '0')} line=${String(lineIndex + 1).padStart(6, '0')} `;
  const line = `${prefix}${'x'.repeat(width - 1 - prefix.length)}\n`;
  if (Buffer.byteLength(line) !== width) {
    throw new Error('fixture line width invariant failed');
  }
  return line;
}

function generateFile(fileIndex, fileBytes) {
  const lineWidth = 128;
  const markerColumn = 64;
  const markerWidth = 32;
  const fullLines = Math.floor(fileBytes / lineWidth);
  if (fullLines < 8) {
    throw new Error('--file-bytes must provide at least eight 128-byte lines (>= 1024)');
  }
  const lines = Array.from(
    { length: fullLines },
    (_, line) => fixtureLine(fileIndex, line, lineWidth),
  );
  const selected = [0.1, 0.5, 0.9]
    .map((fraction) => Math.floor((fullLines - 1) * fraction));
  if (new Set(selected).size !== selected.length) {
    throw new Error('fixture marker lines are not unique');
  }
  const edits = [];
  for (let slot = 0; slot < selected.length; slot += 1) {
    const lineIndex = selected[slot];
    const expected = fixedToken(`SRC_F${String(fileIndex).padStart(4, '0')}_S${slot}`);
    const replacement = fixedToken(`DST_F${String(fileIndex).padStart(4, '0')}_S${slot}`);
    lines[lineIndex] = `${lines[lineIndex].slice(0, markerColumn)}${expected}${lines[lineIndex].slice(markerColumn + markerWidth)}`;
    edits.push({
      start: { line: lineIndex + 1, character: markerColumn },
      end: { line: lineIndex + 1, character: markerColumn + markerWidth },
      expected,
      replacement,
    });
  }
  const tail = 'z'.repeat(fileBytes - fullLines * lineWidth);
  const source = `${lines.join('')}${tail}`;
  let expectedOutput = source;
  for (const edit of edits) {
    const occurrences = expectedOutput.split(edit.expected).length - 1;
    if (occurrences !== 1) {
      throw new Error(`fixture token occurrence invariant failed: ${edit.expected}`);
    }
    expectedOutput = expectedOutput.replace(edit.expected, edit.replacement);
  }
  const sourceBytes = Buffer.from(source, 'utf8');
  const outputBytes = Buffer.from(expectedOutput, 'utf8');
  if (sourceBytes.length !== fileBytes || outputBytes.length !== fileBytes) {
    throw new Error('fixture byte-size invariant failed');
  }
  return { sourceBytes, outputBytes, edits };
}

function fileState(bytes) {
  return { state: 'file', bytes: bytes.length, sha256: sha256(bytes) };
}

const MISSING = Object.freeze({ state: 'missing' });

function ensureParent(workspace, relative) {
  mkdirSync(path.dirname(path.join(workspace, ...relative.split('/'))), { recursive: true });
}

function writeFixture(workspace, relative, bytes) {
  ensureParent(workspace, relative);
  writeFileSync(path.join(workspace, ...relative.split('/')), bytes);
}

function addExpected(expected, relative, before, after) {
  if (expected.has(relative)) {
    throw new Error(`benchmark operation graph touches a path twice: ${relative}`);
  }
  expected.set(relative, { before, after });
}

function modifyOperation(workspace, expected, patchOperations, index, fileBytes) {
  const fixture = generateFile(index, fileBytes);
  const relative = `modify/file-${String(index).padStart(4, '0')}.rs`;
  writeFixture(workspace, relative, fixture.sourceBytes);
  const before = fileState(fixture.sourceBytes);
  const after = fileState(fixture.outputBytes);
  addExpected(expected, relative, before, after);
  patchOperations.push({
    type: 'modify', path: relative,
    sourceBytes: fixture.sourceBytes, outputBytes: fixture.outputBytes,
  });
  return {
    type: 'modify',
    path: relative,
    source_sha256: before.sha256,
    output_sha256: after.sha256,
    bytes_before: before.bytes,
    bytes_after: after.bytes,
    edits: fixture.edits,
  };
}

function createOperation(workspace, expected, patchOperations, index, fileBytes) {
  const fixture = generateFile(index, fileBytes);
  const relative = `create/file-${String(index).padStart(4, '0')}.rs`;
  ensureParent(workspace, relative);
  const after = fileState(fixture.sourceBytes);
  addExpected(expected, relative, MISSING, after);
  patchOperations.push({ type: 'create', path: relative, outputBytes: fixture.sourceBytes });
  return {
    type: 'create',
    path: relative,
    content: fixture.sourceBytes.toString('utf8'),
    output_sha256: after.sha256,
    bytes_after: after.bytes,
  };
}

function deleteOperation(workspace, expected, patchOperations, index, fileBytes) {
  const fixture = generateFile(index, fileBytes);
  const relative = `delete/file-${String(index).padStart(4, '0')}.rs`;
  writeFixture(workspace, relative, fixture.sourceBytes);
  const before = fileState(fixture.sourceBytes);
  addExpected(expected, relative, before, MISSING);
  patchOperations.push({ type: 'delete', path: relative, sourceBytes: fixture.sourceBytes });
  return {
    type: 'delete',
    path: relative,
    source_sha256: before.sha256,
    bytes_before: before.bytes,
  };
}

function renameOperation(workspace, expected, patchOperations, index, fileBytes) {
  const fixture = generateFile(index, fileBytes);
  const suffix = `file-${String(index).padStart(4, '0')}.rs`;
  const source = `rename-source/${suffix}`;
  const target = `rename-target/${suffix}`;
  writeFixture(workspace, source, fixture.sourceBytes);
  ensureParent(workspace, target);
  const state = fileState(fixture.sourceBytes);
  addExpected(expected, source, state, MISSING);
  addExpected(expected, target, MISSING, state);
  patchOperations.push({ type: 'rename', source, target });
  return {
    type: 'rename',
    source,
    target,
    source_sha256: state.sha256,
    bytes_before: state.bytes,
  };
}

const FACTORIES = {
  modify: modifyOperation,
  create: createOperation,
  delete: deleteOperation,
  rename: renameOperation,
};

export function resetOwnedDirectory(directory, ownerRoot) {
  if (!isWithin(directory, ownerRoot)) {
    throw new Error(`refusing to reset path outside benchmark work root: ${directory}`);
  }
  rmSync(directory, { recursive: true, force: true });
  mkdirSync(directory, { recursive: true });
}

export function analyzeRenameGraph(operations) {
  const edges = new Map();
  for (const operation of operations.filter((item) => item.type === 'rename')) {
    if (edges.has(operation.source)) {
      throw new Error(`duplicate rename source: ${operation.source}`);
    }
    edges.set(operation.source, operation.target);
  }
  const chains = [...edges]
    .filter(([, target]) => edges.has(target))
    .map(([source, target]) => ({ source, target, next: edges.get(target) }));
  const cycles = [];
  const completed = new Set();
  for (const start of edges.keys()) {
    if (completed.has(start)) continue;
    const order = [];
    const positions = new Map();
    let current = start;
    while (edges.has(current) && !completed.has(current)) {
      if (positions.has(current)) {
        cycles.push(order.slice(positions.get(current)));
        break;
      }
      positions.set(current, order.length);
      order.push(current);
      current = edges.get(current);
    }
    order.forEach((pathValue) => completed.add(pathValue));
  }
  return { chains, cycles };
}

export function generateScenario(sampleRoot, workload, operationCount, fileBytes) {
  if (!(workload in FACTORIES) && workload !== 'mixed') {
    throw new Error(`unsupported benchmark workload: ${workload}`);
  }
  if (workload === 'mixed' && operationCount !== 10) {
    throw new Error('mixed workload is exactly ten operations');
  }
  const workspace = path.join(sampleRoot, 'workspace');
  mkdirSync(workspace, { recursive: true });
  const expected = new Map();
  const patchOperations = [];
  const operations = [];
  // An unrelated control file proves that tools neither replace the workspace root nor
  // damage paths outside the operation graph. It also keeps the fixture root observable
  // when a delete-only Git patch removes every requested target and prunes empty parents.
  const controlPath = 'control/untouched.txt';
  const controlBytes = Buffer.from('benchmark control: must remain byte-identical\n', 'utf8');
  writeFixture(workspace, controlPath, controlBytes);
  const controlState = fileState(controlBytes);
  addExpected(expected, controlPath, controlState, controlState);
  if (workload === 'mixed') {
    for (let index = 0; index < 6; index += 1) {
      operations.push(modifyOperation(workspace, expected, patchOperations, index, fileBytes));
    }
    for (let index = 6; index < 8; index += 1) {
      operations.push(createOperation(workspace, expected, patchOperations, index, fileBytes));
    }
    for (let index = 8; index < 10; index += 1) {
      operations.push(deleteOperation(workspace, expected, patchOperations, index, fileBytes));
    }
  } else {
    const factory = FACTORIES[workload];
    for (let index = 0; index < operationCount; index += 1) {
      operations.push(factory(workspace, expected, patchOperations, index, fileBytes));
    }
  }
  const manifest = {
    schema: MANIFEST_SCHEMA,
    fixture_generator: 'weavatrix-fixed-markers-v2',
    fixture_seed: null,
    operation: `benchmark_${workload}`,
    workload,
    operation_count: operations.length,
    touched_path_count: expected.size - 1,
    file_bytes: fileBytes,
    operations,
  };
  const manifestPath = path.join(sampleRoot, 'manifest.json');
  const patchPath = path.join(sampleRoot, 'changes.patch');
  writeFileSync(manifestPath, `${JSON.stringify(manifest)}\n`, 'utf8');
  writeFileSync(patchPath, generateUnifiedPatch(patchOperations), 'utf8');
  const expectedDirectories = new Set();
  for (const relative of expected.keys()) {
    let parent = path.posix.dirname(relative);
    while (parent !== '.') {
      expectedDirectories.add(parent);
      parent = path.posix.dirname(parent);
    }
  }
  return {
    workspace, manifestPath, patchPath, manifest, expected, expectedDirectories,
  };
}

function scanTree(root) {
  const observed = [];
  function visit(directory) {
    const entries = readdirSync(directory, { withFileTypes: true })
      .sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      const absolute = path.join(directory, entry.name);
      const relative = path.relative(root, absolute).split(path.sep).join('/');
      if (entry.isDirectory()) {
        observed.push({ path: relative, type: 'directory', bytes: null, sha256: null });
        visit(absolute);
      } else if (entry.isFile()) {
        const bytes = readFileSync(absolute);
        observed.push({ path: relative, type: 'file', bytes: bytes.length, sha256: sha256(bytes) });
      } else if (entry.isSymbolicLink()) {
        observed.push({ path: relative, type: 'symlink', bytes: null, sha256: null });
      } else {
        observed.push({
          path: relative,
          type: 'other',
          bytes: lstatSync(absolute).size,
          sha256: null,
        });
      }
    }
  }
  visit(root);
  return observed;
}

function allowedPersistentArtifact(adapter, entry) {
  return adapter === 'weavatrix' && (
    (entry.type === 'file' && entry.path === '.weavatrix/worktree/lock')
    || (entry.type === 'directory'
      && (entry.path === '.weavatrix' || entry.path === '.weavatrix/worktree'))
  );
}

function stateMatches(expected, actual) {
  if (expected.state === 'missing') {
    return actual === undefined;
  }
  return actual?.type === 'file'
    && actual.sha256 === expected.sha256
    && actual.bytes === expected.bytes;
}

export function correctnessGates(adapter, mode, scenario, processResult) {
  const observed = scanTree(scenario.workspace);
  const observedByPath = new Map(observed.map((entry) => [entry.path, entry]));
  const stateFailures = [];
  const phase = mode === 'dry-run' ? 'before' : 'after';
  for (const [relative, states] of scenario.expected.entries()) {
    const expectedState = states[phase];
    const actual = observedByPath.get(relative);
    if (!stateMatches(expectedState, actual)) {
      stateFailures.push({
        path: relative,
        expected_state: expectedState.state,
        expected_sha256: expectedState.sha256 ?? null,
        expected_bytes: expectedState.bytes ?? null,
        actual_state: actual === undefined ? 'missing' : actual.type,
        actual_sha256: actual?.sha256 ?? null,
        actual_bytes: actual?.bytes ?? null,
      });
    }
  }
  const extras = observed.filter((entry) => !scenario.expected.has(entry.path)
    && !scenario.expectedDirectories.has(entry.path));
  const allowedArtifacts = extras
    .filter((entry) => allowedPersistentArtifact(adapter, entry));
  const unexpectedArtifacts = extras
    .filter((entry) => !allowedPersistentArtifact(adapter, entry));
  const adapterJson = processResult.adapterResult;
  const adapterJsonGate = adapterJson !== null
    && adapterJson.schema === ADAPTER_SCHEMA
    && adapterJson.ok === true;
  const gates = {
    adapter_exit: processResult.exitCode === 0 && processResult.error === null,
    adapter_json: adapterJsonGate,
    adapter_report_count: adapterJsonGate
      && Number(adapterJson.operations) === scenario.manifest.operation_count
      && Number(adapterJson.touched_paths) === scenario.manifest.touched_path_count,
    adapter_resource_budget: adapter !== 'weavatrix' || (adapterJsonGate
      && Number(adapterJson.effective_max_files) >= scenario.manifest.touched_path_count
      && Number(adapterJson.effective_max_paths) >= scenario.manifest.touched_path_count),
    tree_state: stateFailures.length === 0,
    artifact_cleanup: unexpectedArtifacts.length === 0,
  };
  gates.all = Object.values(gates).every(Boolean);
  const expectedTree = [...scenario.expected.entries()].map(([relative, states]) => ({
    path: relative,
    before: states.before,
    after: states.after,
  }));
  return {
    gates,
    stateFailures,
    observed,
    allowedArtifacts,
    unexpectedArtifacts,
    expectedTree,
    touchedPathCount: scenario.manifest.touched_path_count,
  };
}
