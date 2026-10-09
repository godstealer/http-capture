const { buildSync } = require('esbuild');
const assert = require('node:assert/strict');
const fixtures = require('./fixtures/zstd.json');
const output = buildSync({ entryPoints: ['apps/frontend/src/response.worker.ts'], bundle: true, platform: 'node', format: 'cjs', write: false, packages: 'external' });
let result;
global.self = { postMessage(value) { result = value; } };
new Function('require', output.outputFiles[0].text)(require);
(async () => {
  for (const fixture of fixtures.filter(f => f.input && f.input !== '')) {
    await self.onmessage({ data: { body: Buffer.from(fixture.compressed, 'hex').toString('base64'), contentType: 'application/json', encoding: fixture.encoding || 'zstd', parser: 'auto' } });
    assert.equal(result.error, '');
    assert.deepEqual(Buffer.from(await result.decodedBlob.arrayBuffer()), Buffer.from(fixture.input, 'hex'));
    assert.deepEqual(JSON.parse(result.formatted), JSON.parse(Buffer.from(fixture.input, 'hex').toString()));
  }
  await self.onmessage({ data: { body: 'AAECAw==', contentType: 'application/octet-stream', encoding: '', parser: 'auto' } });
  assert.equal(result.binary, true);
  assert.deepEqual(Buffer.from(await result.blob.arrayBuffer()), Buffer.from([0, 1, 2, 3]));
  console.log('Response worker: zstd and stacked gzip/zstd, formatted JSON, exact decoded download bytes and binary blobs passed.');
})().catch(error => { console.error(error); process.exitCode = 1; });
