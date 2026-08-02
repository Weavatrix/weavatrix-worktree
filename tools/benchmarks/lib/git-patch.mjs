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
    throw new Error('benchmark Git patches require unchanged line counts');
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

function patchForFile(file, contextLines) {
  validatePath(file.path);
  const before = splitLines(file.sourceBytes.toString('utf8'));
  const after = splitLines(file.outputBytes.toString('utf8'));
  const changed = changedLines(before, after);
  const windows = mergeWindows(changed, before.length, contextLines);
  let patch = `diff --git a/${file.path} b/${file.path}\n`;
  patch += `--- a/${file.path}\n+++ b/${file.path}\n`;
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

export function generateUnifiedPatch(files, contextLines = 3) {
  if (!Number.isSafeInteger(contextLines) || contextLines < 1) {
    throw new Error('Git patch context must be a positive integer');
  }
  if (!Array.isArray(files) || files.length === 0) {
    throw new Error('Git patch requires at least one fixture file');
  }
  return files.map((file) => patchForFile(file, contextLines)).join('');
}
