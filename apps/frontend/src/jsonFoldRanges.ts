// CodeMirror parses large documents incrementally. Compute multiline JSON
// object/array ranges so the root and nested nodes can fold immediately.
export function jsonFoldRanges(text: string): Map<number, { from: number; to: number }> {
  const maxRanges = 100_000;
  const ranges = new Map<number, { from: number; to: number }>();
  const stack: { at: number; lineStart: number; bracket: string }[] = [];
  let lineStart = 0, quoted = false, escaped = false;
  for (let at = 0; at < text.length; at++) {
    const char = text[at];
    if (quoted) {
      if (escaped) escaped = false;
      else if (char === '\\') escaped = true;
      else if (char === '"') quoted = false;
    } else if (char === '"') quoted = true;
    else if (char === '{' || char === '[') stack.push({ at, lineStart, bracket: char });
    else if (char === '}' || char === ']') {
      const open = stack.pop();
      if (!open || (open.bracket === '{' ? char !== '}' : char !== ']')) { stack.length = 0; continue; }
      // Keep the root even if a very large document fills the range budget.
      if (lineStart > open.lineStart && at > open.at + 1 && (ranges.size < maxRanges || stack.length === 0)) {
        const previous = ranges.get(open.lineStart);
        if (!previous || open.at < previous.from) ranges.set(open.lineStart, { from: open.at + 1, to: at });
      }
    }
    if (char === '\n') lineStart = at + 1;
  }
  return ranges;
}
