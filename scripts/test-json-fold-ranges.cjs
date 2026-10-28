const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { transformSync } = require('esbuild');

const source = readFileSync('apps/frontend/src/jsonFoldRanges.ts', 'utf8');
const compiled = transformSync(source, { loader: 'ts', format: 'cjs' }).code;
const foldModule = { exports: {} };
new Function('exports', 'module', compiled)(foldModule.exports, foldModule);
const { jsonFoldRanges } = foldModule.exports;

const sample = '{\n  "outer": {\n    "items": [\n      {\n        "text": "escaped \\u007b bracket } and \\\" quote"\n      }\n    ]\n  }\n}';
const lines = sample.split('\n');
const positions = lines.map((_, index) => lines.slice(0, index).join('\n').length + (index ? 1 : 0));
const ranges = jsonFoldRanges(sample);
for (const line of [0, 1, 2, 3]) {
  const range = ranges.get(positions[line]);
  assert(range, `JSON node on line ${line + 1} can fold`);
  assert.equal(sample[range.from - 1], line === 2 ? '[' : '{');
}
assert.equal(ranges.size, 4, 'brackets inside JSON strings are ignored');
assert.equal(jsonFoldRanges('{"value": 1}').size, 0, 'single-line JSON has no gutter fold');
console.log('JSON fold ranges: root and every multiline nested node can fold; quoted brackets are ignored.');
