const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');
const { createGzip, createDeflate, createBrotliCompress, createZstdCompress, gzipSync, brotliCompressSync, zstdCompressSync, constants } = require('node:zlib');

const output = buildSync({ entryPoints: ['apps/frontend/src/sseLive.worker.ts'], bundle: true, platform: 'node', format: 'cjs', write: false, packages: 'external' });
const encodingOutput = buildSync({ entryPoints: ['apps/frontend/src/sseEncoding.ts'], bundle: true, platform: 'node', format: 'cjs', write: false });
const encodingModule = { exports: {} };
new Function('module', 'exports', encodingOutput.outputFiles[0].text)(encodingModule, encodingModule.exports);
const { parseSseEncodings } = encodingModule.exports;
assert.deepEqual(parseSseEncodings('GZip, identity, Br'), ['br', 'gzip']);
assert.equal(parseSseEncodings('identity'), null);
assert.equal(parseSseEncodings('gzip,,br'), null);
assert.equal(parseSseEncodings('gzip, unknown'), null);
assert.equal(parseSseEncodings('gzip,br,zstd,deflate,gzip'), null);
const messages = [];
global.self = { postMessage(message) { messages.push(message); } };
new Function('require', output.outputFiles[0].text)(require);

const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(predicate) {
  for (let i = 0; i < 200; i++) {
    const value = messages.find(predicate);
    if (value) return value;
    await delay(10);
  }
  throw new Error('live SSE view did not update');
}
async function flush(stream, encoding = 'gzip') {
  const operation = encoding === 'br' ? constants.BROTLI_OPERATION_FLUSH : encoding === 'zstd' ? constants.ZSTD_e_flush : constants.Z_SYNC_FLUSH;
  await new Promise((resolve, reject) => stream.flush(operation, error => error ? reject(error) : resolve()));
}
async function exercise(encoding) {
  messages.length = 0;
  const stream = encoding === 'gzip' ? createGzip() : encoding === 'deflate' ? createDeflate() : encoding === 'br' ? createBrotliCompress() : createZstdCompress();
  const chunks = [];
  stream.on('data', chunk => chunks.push(chunk));
  stream.write('id: 1\ndata: first\n\n');
  await flush(stream, encoding);
  const first = Buffer.concat(chunks);
  // Snapshots can end between Base64 quartets. Each update must append only
  // the genuinely new compressed bytes to the same decompressor.
  for (const length of [1, 2, first.length]) self.onmessage({ data: { body: first.subarray(0, length).toString('base64'), encoding } });
  const live = await until(message => message.raw === 'id: 1\ndata: first\n\n');
  assert.equal(live.error, '');
  stream.write('data: second\n\n');
  await flush(stream, encoding);
  const both = Buffer.concat(chunks);
  self.onmessage({ data: { body: both.toString('base64'), encoding } });
  const next = await until(message => message.raw === 'id: 1\ndata: first\n\ndata: second\n\n');
  assert.equal(next.error, '');
  stream.end();
}

async function exerciseRanges(encoding) {
  messages.length = 0;
  const stream = encoding === 'br' ? createBrotliCompress() : encoding === 'zstd' ? createZstdCompress() : createGzip();
  const chunks = [];
  stream.on('data', chunk => chunks.push(chunk));
  stream.write('data: range one\n\n');
  await flush(stream, encoding);
  const first = Buffer.concat(chunks);
  for (const [index, part] of [first.subarray(0, 1), first.subarray(1, 5), first.subarray(5)].entries()) {
    self.onmessage({ data: { chunk: new Uint8Array(part), encoding, reset: index === 0 } });
  }
  await until(message => message.raw === 'data: range one\n\n');
  stream.write('data: range two\n\n');
  await flush(stream, encoding);
  const second = Buffer.concat(chunks).subarray(first.length);
  self.onmessage({ data: { chunk: new Uint8Array(second), encoding } });
  await until(message => message.raw === 'data: range one\n\ndata: range two\n\n');
  const beforeEnd = Buffer.concat(chunks).length;
  const ended = new Promise(resolve => stream.once('end', resolve));
  stream.end(); await ended;
  const trailer = Buffer.concat(chunks).subarray(beforeEnd);
  const beforeFinal = messages.length;
  self.onmessage({ data: { chunk: new Uint8Array(trailer), encoding, finish: true } });
  await until(message => messages.indexOf(message) >= beforeFinal && message.raw === 'data: range one\n\ndata: range two\n\n' && message.error === '');
}

async function exerciseStacked(encoding, bytes) {
  messages.length = 0;
  const parts = [bytes.subarray(0, 1), bytes.subarray(1, 7), bytes.subarray(7, Math.floor(bytes.length / 2)), bytes.subarray(Math.floor(bytes.length / 2))];
  for (const [index, part] of parts.entries())
    self.onmessage({ data: { chunk: new Uint8Array(part), encoding, reset: index === 0, finish: index === parts.length - 1 } });
  const final = await until(message => message.raw === 'id: layered\ndata: 完整事件\n\n' && message.error === '');
  assert.equal(final.binary, false);
  assert.equal(messages.some(message => message.error), false);
}

async function exerciseLiveStacked() {
  messages.length = 0;
  const inner = createGzip();
  const outer = createBrotliCompress();
  inner.pipe(outer);
  const chunks = [];
  outer.on('data', chunk => chunks.push(chunk));
  inner.write('data: first layer\n\n');
  await flush(inner, 'gzip');
  await flush(outer, 'br');
  const first = Buffer.concat(chunks);
  self.onmessage({ data: { body: first.toString('base64'), encoding: 'gzip, br' } });
  await until(message => message.raw === 'data: first layer\n\n');
  inner.write('data: second layer\n\n');
  await flush(inner, 'gzip');
  await flush(outer, 'br');
  self.onmessage({ data: { body: Buffer.concat(chunks).toString('base64'), encoding: 'gzip, br' } });
  await until(message => message.raw === 'data: first layer\n\ndata: second layer\n\n');
  inner.end();
}

(async () => {
  await exercise('gzip');
  await exercise('deflate');
  await exercise('br');
  await exercise('zstd');
  await exerciseRanges('gzip');
  await exerciseRanges('br');
  await exerciseRanges('zstd');
  const layered = Buffer.from('id: layered\ndata: 完整事件\n\n');
  await exerciseStacked('gzip, br', brotliCompressSync(gzipSync(layered)));
  await exerciseStacked('zstd, gzip', gzipSync(zstdCompressSync(layered)));
  await exerciseStacked('br, zstd', zstdCompressSync(brotliCompressSync(layered)));
  await exerciseLiveStacked();
  messages.length = 0;
  const trailing = Buffer.concat([brotliCompressSync(Buffer.from('data: ok\n\n')), Buffer.from([0])]);
  self.onmessage({ data: { chunk: new Uint8Array(trailing), encoding: 'br', reset: true, finish: true } });
  await until(message => /多余数据/.test(message.error ?? ''));
  messages.length = 0;
  const bomb = brotliCompressSync(Buffer.alloc(8 * 1024 * 1024 + 1, 65));
  self.onmessage({ data: { chunk: new Uint8Array(bomb), encoding: 'br', reset: true, finish: true } });
  await until(message => /解压后正文超过 8 MiB/.test(message.error ?? ''));
  messages.length = 0;
  const frame = zstdCompressSync(Buffer.alloc(5 * 1024 * 1024, 65));
  self.onmessage({ data: { chunk: new Uint8Array(Buffer.concat([frame, frame])), encoding: 'zstd', reset: true, finish: true } });
  await until(message => /解压后正文超过 8 MiB/.test(message.error ?? ''));
  console.log('Live SSE: four codecs and stacked encodings preserve incremental events across byte ranges.');
})().catch(error => { console.error(error); process.exitCode = 1; });
