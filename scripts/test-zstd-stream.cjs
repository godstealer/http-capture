const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');
const { Decompress } = require('fzstd');
const fixtures = require('./fixtures/zstd.json');
const output = buildSync({ entryPoints: ['apps/frontend/src/zstdStreamGuard.ts'], bundle: true, platform: 'node', format: 'cjs', write: false });
const mod = { exports: {} };
new Function('module', 'exports', output.outputFiles[0].text)(mod, mod.exports);
const { ZstdStreamGuard } = mod.exports;
function decode(input, step = 1, limit = 8 * 1024 * 1024) {
  const chunks = [];
  const decoder = new Decompress(chunk => chunks.push(Buffer.from(chunk)));
  const guard = new ZstdStreamGuard(limit);
  let forwarded = 0;
  for (let offset = 0; offset < input.length; offset += step) guard.push(input.subarray(offset, offset + step), safe => { forwarded += safe.length; decoder.push(safe, false); });
  guard.finish(); decoder.push(new Uint8Array(), true);
  return { body: Buffer.concat(chunks), forwarded };
}
for (const fixture of fixtures.filter(value => !value.encoding)) {
  const compressed = Buffer.from(fixture.compressed, 'hex');
  const expected = fixture.repeat ? Buffer.alloc(fixture.repeat, 65) : Buffer.from(fixture.input, 'hex');
  for (const step of [1, 2, 7, 64]) {
    const result = decode(compressed, step);
    assert.deepEqual(result.body, expected);
    assert.equal(result.forwarded, compressed.length);
  }
  if (compressed.length) assert.throws(() => decode(compressed.subarray(0, compressed.length - 1)), /不完整或已损坏/);
}
const compressed = Buffer.from(fixtures[1].compressed, 'hex');
const skippable = Buffer.from('502a4d1800000000', 'hex');
assert.deepEqual(decode(Buffer.concat([skippable, compressed]), 1).body, Buffer.from(fixtures[1].input, 'hex'));
const hugeWindow = Buffer.from('28b52ffd00a0010000', 'hex');
let forwarded = 0;
const guard = new ZstdStreamGuard(8 * 1024 * 1024);
assert.throws(() => { for (const byte of hugeWindow) guard.push(Uint8Array.of(byte), safe => { forwarded += safe.length; }); }, /解压窗口超过 8 MiB/);
assert.equal(forwarded, 0, 'an oversized frame header is never handed to fzstd');
const hugeSingleSegment = Buffer.from(fixtures[2].compressed, 'hex');
hugeSingleSegment.set([0, 0, 0, 4], 5);
assert.throws(() => decode(hugeSingleSegment, 1), /解压窗口超过 8 MiB/);
assert.throws(() => decode(Buffer.concat([compressed, hugeWindow]), 1), /解压窗口超过 8 MiB/);
console.log('Zstd stream guard: one-byte frames, concatenated/skippable input, truncation and pre-allocation window rejection passed.');
