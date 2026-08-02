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

export function resetOwnedDirectory(directory, ownerRoot) {
  if (!isWithin(directory, ownerRoot)) {
    throw new Error(`refusing to reset path outside benchmark work root: ${directory}`);
  }
  rmSync(directory, { recursive: true, force: true });
  mkdirSync(directory, { recursive: true });
}

export function generateScenario(sampleRoot, fileCount, fileBytes) {
  const workspace = path.join(sampleRoot, 'workspace');
  const sourceDirectory = path.join(workspace, 'src');
  mkdirSync(sourceDirectory, { recursive: true });
  const files = [];
  const expected = new Map();
  for (let fileIndex = 0; fileIndex < fileCount; fileIndex += 1) {
    const basename = `file-${String(fileIndex).padStart(4, '0')}.rs`;
    const relative = `src/${basename}`;
    const fixture = generateFile(fileIndex, fileBytes);
    writeFileSync(path.join(sourceDirectory, basename), fixture.sourceBytes);
    const sourceHash = sha256(fixture.sourceBytes);
    const expectedHash = sha256(fixture.outputBytes);
    files.push({
      path: relative,
      sha256: sourceHash,
      expected_sha256: expectedHash,
      bytes_before: fixture.sourceBytes.length,
      bytes_after: fixture.outputBytes.length,
      edits: fixture.edits,
    });
    expected.set(relative, { sourceHash, expectedHash });
  }
  const manifest = {
    schema: MANIFEST_SCHEMA,
    operation: 'benchmark_exact_replace',
    file_count: fileCount,
    file_bytes: fileBytes,
    files,
  };
  const manifestPath = path.join(sampleRoot, 'manifest.json');
  writeFileSync(manifestPath, `${JSON.stringify(manifest)}\n`, 'utf8');
  return { workspace, manifestPath, manifest, expected };
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

function allowedPersistentArtifact(adapter, relative) {
  return adapter === 'weavatrix' && relative === '.weavatrix/worktree/lock';
}

export function correctnessGates(adapter, mode, scenario, processResult) {
  const observed = scanTree(scenario.workspace);
  const observedByPath = new Map(observed.map((entry) => [entry.path, entry]));
  const hashFailures = [];
  for (const [relative, hashes] of scenario.expected.entries()) {
    const entry = observedByPath.get(relative);
    const expectedHash = mode === 'dry-run' ? hashes.sourceHash : hashes.expectedHash;
    if (entry?.type !== 'file' || entry.sha256 !== expectedHash) {
      hashFailures.push({
        path: relative,
        expected_sha256: expectedHash,
        actual_sha256: entry?.sha256 ?? null,
        actual_type: entry?.type ?? 'missing',
      });
    }
  }
  const extras = observed.filter((entry) => !scenario.expected.has(entry.path));
  const allowedArtifacts = extras
    .filter((entry) => allowedPersistentArtifact(adapter, entry.path));
  const unexpectedArtifacts = extras
    .filter((entry) => !allowedPersistentArtifact(adapter, entry.path));
  const adapterJson = processResult.adapterResult;
  const adapterJsonGate = adapterJson !== null
    && adapterJson.schema === ADAPTER_SCHEMA
    && adapterJson.ok === true;
  const gates = {
    adapter_exit: processResult.exitCode === 0 && processResult.error === null,
    adapter_json: adapterJsonGate,
    adapter_report_count: adapterJsonGate
      && Number(adapterJson.files) === scenario.manifest.file_count,
    content_hashes: hashFailures.length === 0,
    artifact_cleanup: unexpectedArtifacts.length === 0,
  };
  gates.all = Object.values(gates).every(Boolean);
  return { gates, hashFailures, observed, allowedArtifacts, unexpectedArtifacts };
}
