import assert from 'node:assert/strict';
import test from 'node:test';

import { generateUnifiedPatch } from '../lib/git-patch.mjs';
import { configurationsFor } from '../lib/matrix.mjs';
import { validateRunOptions } from '../lib/options.mjs';

test('Git patch generation is deterministic and carries exact context', () => {
  const patch = generateUnifiedPatch([{
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
      path: '../escape.rs',
      sourceBytes: Buffer.from('old\n'),
      outputBytes: Buffer.from('new\n'),
    }]),
    /not safe/u,
  );
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
  const config = validateRunOptions({ adapter: 'git-apply' });
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
