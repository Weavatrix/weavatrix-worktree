function splitLines(text) {
  const lines = [];
  let start = 0;
  for (let index = 0; index < text.length; index += 1) {
    if (text[index] === '\n') {
      lines.push({ text: text.slice(start, index), newline: true });
      start = index + 1;
    }
  }
  if (start < text.length) {
    lines.push({ text: text.slice(start), newline: false });
  }
  return lines;
}

function validatePath(relative) {
  if (!/^[A-Za-z0-9._/-]+$/u.test(relative)
    || relative.startsWith('/')
    || relative.split('/').some((part) => part.length === 0 || part === '..')) {
    throw new Error(`fixture path is not safe for an unquoted Git patch: ${relative}`);
  }
}

function changedLines(before, after) {
  if (before.length !== after.length) {
    throw new Error('modify benchmark Git patches require unchanged line counts');
  }
  const changed = [];
  for (let index = 0; index < before.length; index += 1) {
    if (before[index].text !== after[index].text
      || before[index].newline !== after[index].newline) {
      changed.push(index);
    }
  }
  if (changed.length === 0) {
    throw new Error('benchmark Git patch file has no changes');
  }
  return changed;
}

function mergeWindows(changed, lineCount, contextLines) {
  const windows = changed.map((line) => ({
    start: Math.max(0, line - contextLines),
    end: Math.min(lineCount, line + contextLines + 1),
  }));
  const merged = [];
  for (const window of windows) {
    const previous = merged.at(-1);
    if (previous !== undefined && window.start <= previous.end) {
      previous.end = Math.max(previous.end, window.end);
    } else {
      merged.push({ ...window });
    }
  }
  return merged;
}

function patchLine(prefix, line) {
  const marker = line.newline ? '' : '\\ No newline at end of file\n';
  return `${prefix}${line.text}\n${marker}`;
}

function modifyPatch(operation, contextLines) {
  validatePath(operation.path);
  const before = splitLines(operation.sourceBytes.toString('utf8'));
  const after = splitLines(operation.outputBytes.toString('utf8'));
  const changed = changedLines(before, after);
  const windows = mergeWindows(changed, before.length, contextLines);
  let patch = `diff --git a/${operation.path} b/${operation.path}\n`;
  patch += `--- a/${operation.path}\n+++ b/${operation.path}\n`;
  for (const window of windows) {
    const count = window.end - window.start;
    patch += `@@ -${window.start + 1},${count} +${window.start + 1},${count} @@\n`;
    for (let index = window.start; index < window.end; index += 1) {
      if (before[index].text === after[index].text
        && before[index].newline === after[index].newline) {
        patch += patchLine(' ', before[index]);
      } else {
        patch += patchLine('-', before[index]);
        patch += patchLine('+', after[index]);
      }
    }
  }
  return patch;
}

function createPatch(operation) {
  validatePath(operation.path);
  const lines = splitLines(operation.outputBytes.toString('utf8'));
  if (lines.length === 0) throw new Error('create benchmark content must be non-empty');
  let patch = `diff --git a/${operation.path} b/${operation.path}\n`;
  patch += 'new file mode 100644\n';
  patch += '--- /dev/null\n';
  patch += `+++ b/${operation.path}\n`;
  patch += `@@ -0,0 +1,${lines.length} @@\n`;
  patch += lines.map((line) => patchLine('+', line)).join('');
  return patch;
}

function deletePatch(operation) {
  validatePath(operation.path);
  const lines = splitLines(operation.sourceBytes.toString('utf8'));
  if (lines.length === 0) throw new Error('delete benchmark content must be non-empty');
  let patch = `diff --git a/${operation.path} b/${operation.path}\n`;
  patch += 'deleted file mode 100644\n';
  patch += `--- a/${operation.path}\n`;
  patch += '+++ /dev/null\n';
  patch += `@@ -1,${lines.length} +0,0 @@\n`;
  patch += lines.map((line) => patchLine('-', line)).join('');
  return patch;
}

function renamePatch(operation) {
  validatePath(operation.source);
  validatePath(operation.target);
  return [
    `diff --git a/${operation.source} b/${operation.target}`,
    'similarity index 100%',
    `rename from ${operation.source}`,
    `rename to ${operation.target}`,
    '',
  ].join('\n');
}

export function generateUnifiedPatch(operations, contextLines = 3) {
  if (!Number.isSafeInteger(contextLines) || contextLines < 1) {
    throw new Error('Git patch context must be a positive integer');
  }
  if (!Array.isArray(operations) || operations.length === 0) {
    throw new Error('Git patch requires at least one fixture operation');
  }
  return operations.map((operation) => {
    if (operation.type === 'modify') return modifyPatch(operation, contextLines);
    if (operation.type === 'create') return createPatch(operation);
    if (operation.type === 'delete') return deletePatch(operation);
    if (operation.type === 'rename') return renamePatch(operation);
    throw new Error(`unsupported Git patch operation: ${String(operation.type)}`);
  }).join('');
}
