const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');
const fixtures = require('./fixtures/zstd.json');
const result = buildSync({ entryPoints: ['apps/frontend/src/decodeZstd.ts'], bundle: true, platform: 'node', format: 'cjs', write: false });
const m = { exports: {} }; new Function('module', 'exports', result.outputFiles[0].text)(m, m.exports);
const { decodeZstd } = m.exports;
for (const fixture of fixtures.filter(f => !f.encoding)) {
  const compressed = Buffer.from(fixture.compressed, 'hex');
  const expected = fixture.repeat ? Buffer.alloc(fixture.repeat, 65) : Buffer.from(fixture.input, 'hex');
  assert.deepEqual(Buffer.from(decodeZstd(compressed)), expected);
  if (expected.length) assert.throws(() => decodeZstd(compressed, expected.length - 1), /8 MiB/);
  assert.throws(() => decodeZstd(compressed.subarray(0, compressed.length - 1)));
}
assert.throws(() => decodeZstd(Buffer.from([1, 2, 3])));
const a = fixtures[1];
assert.deepEqual(Buffer.from(decodeZstd(Buffer.from(a.compressed.repeat(2), 'hex'))), Buffer.from(a.input.repeat(2), 'hex'));
console.log('Zstd: empty, Unicode, compressed blocks, concatenated frames, limit, truncation and corrupt input passed.');
