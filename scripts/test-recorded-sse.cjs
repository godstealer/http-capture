const assert = require('node:assert/strict');
const { build } = require('esbuild');
const SIZE = 64 * 1024;
const initial = Buffer.alloc(SIZE + 10, 7);
const extended = Buffer.concat([initial, Buffer.from([1, 2, 3, 4, 5])]);
const flow = (revision, length) => ({ id: 'sse-1', bodyRevision: revision, bodyReferences: { response: { byteLength: length } } });
function serve(bytes, calls) {
  return async (name, request) => {
    assert.equal(name, 'read_flow_body');
    assert.equal(request.length, SIZE);
    calls.push(request);
    const part = bytes.subarray(request.offset, request.offset + request.length);
    return { offset: request.offset, totalBytes: bytes.length, bodyBase64: part.toString('base64'), nextOffset: request.offset + part.length < bytes.length ? request.offset + part.length : null };
  };
}
(async () => {
  const output = await build({ entryPoints: ['apps/frontend/src/recordedSseModel.ts'], bundle: true, platform: 'node', format: 'cjs', write: false, plugins: [{ name: 'api-stub', setup(builder) {
    builder.onLoad({ filter: /[/\\]api\.ts$/ }, () => ({ contents: 'export const invoke=()=>{throw Error("unexpected live API")}' }));
  } }] });
  const model = { exports: {} };
  new Function('module', 'exports', 'require', output.outputFiles[0].text)(model, model.exports, require);
  const { readRecordedSseGrowth } = model.exports;
  const calls = [], chunks = [];
  let offset = await readRecordedSseGrowth(flow('v1', initial.length), 0, { isCurrent: () => true, onChunk: (chunk, next) => chunks.push([Buffer.from(chunk), next]) }, serve(initial, calls));
  assert.equal(offset, initial.length);
  assert.deepEqual(calls.map(call => call.offset), [0, SIZE]);
  assert.deepEqual(Buffer.concat(chunks.map(([chunk]) => chunk)), initial);
  assert.deepEqual(chunks.map(([, next]) => next), [SIZE, initial.length]);
  calls.length = 0; chunks.length = 0;
  offset = await readRecordedSseGrowth(flow('v2', extended.length), offset, { isCurrent: () => true, onChunk: (chunk, next) => chunks.push([Buffer.from(chunk), next]) }, serve(extended, calls));
  assert.equal(offset, extended.length);
  assert.deepEqual(calls.map(call => [call.offset, call.expectedVersion]), [[SIZE, 'v2']]);
  assert.deepEqual(chunks.map(([chunk]) => [...chunk]), [[1, 2, 3, 4, 5]]);
  let current = true, delivered = 0;
  await assert.rejects(readRecordedSseGrowth(flow('v3', extended.length), SIZE, { isCurrent: () => current, onChunk: () => delivered++ }, async (name, request) => { current = false; return serve(extended, [])(name, request) }), /cancelled/);
  assert.equal(delivered, 0);
  await assert.rejects(readRecordedSseGrowth(flow('v3', extended.length), SIZE, { isCurrent: () => true, onChunk: () => delivered++ }, async () => ({ offset: SIZE, totalBytes: extended.length, bodyBase64: 'AA==', nextOffset: null })), /Incomplete recorded body range/);
  assert.equal(delivered, 0);
  await assert.rejects(readRecordedSseGrowth(flow('v4', 8 * 1024 * 1024 + 1), 0, { isCurrent: () => true, onChunk: () => delivered++ }, async () => { throw Error('must not read'); }), /范围不可用/);
  console.log('Recorded live SSE: version-pinned 64 KiB reads, exact growth, stale/cancelled rejection and budget passed.');
})().catch(error => { console.error(error); process.exitCode = 1; });
