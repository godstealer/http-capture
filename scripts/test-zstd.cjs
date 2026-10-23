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
const hugeSingleSegment = Buffer.from(fixtures[2].compressed, 'hex');
hugeSingleSegment.set([0, 0, 0, 4], 5); // Advertise 64 MiB from a tiny compressed frame.
assert.throws(() => decodeZstd(hugeSingleSegment), /解压窗口超过 8 MiB/);
const hugeWindow = Buffer.from('28b52ffd00a0010000', 'hex'); // Non-single-segment frame advertises a 1 GiB window.
assert.throws(() => decodeZstd(hugeWindow), /解压窗口超过 8 MiB/);
const normal = Buffer.from(a.compressed, 'hex');
assert.throws(() => decodeZstd(Buffer.concat([normal, hugeWindow])), /解压窗口超过 8 MiB/);
const skippable = Buffer.from('502a4d1800000000', 'hex');
assert.deepEqual(Buffer.from(decodeZstd(Buffer.concat([skippable, normal]))), Buffer.from(a.input, 'hex'));
assert.throws(() => decodeZstd(Buffer.concat([skippable.subarray(0, 7), normal])), /不完整或已损坏/);
console.log('Zstd: empty, Unicode, compressed blocks, concatenated/skippable frames, window preflight, limit, truncation and corrupt input passed.');
