const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');
const loaded = { exports: {} };
new Function('module', 'exports', buildSync({
  entryPoints: ['apps/frontend/src/requestBody.ts'], bundle: true,
  platform: 'node', format: 'cjs', write: false,
}).outputFiles[0].text)(loaded, loaded.exports);
const {
  AUTO_EDIT_LIMIT, bodyBytes, bodyByteLength, bytesBase64, utf8Body, textBody, requestContentType, rawBodyParser,
  inferRequestBodyMode, rawContentType, withRequestBody, readFormBody,
  encodeFormBody, fileRequestBody, requestBodyWorkMatches,
} = loaded.exports;

const watchdog = setTimeout(() => { console.error('Request body tests did not settle'); process.exit(1); }, 10000);
const draft = (overrides = {}) => ({
  method: 'POST', url: 'https://example.test/old', bodyBase64: 'AAE=', tls: { preset: 'native' },
  headers: [
    { name: 'X-Before', value: 'one' },
    { name: 'Content-Type', value: 'old/type' },
    { name: 'X-Between', value: 'two' },
    { name: 'CONTENT-TYPE', value: 'duplicate/type' },
    { name: 'content-length', value: '999' },
    { name: 'Transfer-Encoding', value: 'chunked' },
    { name: 'Content-Encoding', value: 'gzip' },
    { name: 'Content-Encoding', value: 'br', disabled: true },
    { name: 'Content-Type', value: 'disabled/type', disabled: true },
    { name: 'Content-Length', value: 'disabled length', disabled: true },
    { name: 'X-After', value: 'three' },
  ], ...overrides,
});

(async () => {
  const binary = Uint8Array.from([0, 255, 128, 195, 40, 1, 13, 10]);
  const encoded = bytesBase64(binary);
  assert.equal(encoded, Buffer.from(binary).toString('base64'));
  assert.deepEqual(bodyBytes(encoded), binary);
  assert.equal(bodyByteLength(encoded), binary.length);
  for (const sample of ['', 'YQ==', 'YQ', 'YWI=', 'YWI', 'YWJj', ' Y W J j\r\n', 'AA==', 'AAA=', 'AAAA', 'A', 'AA=', 'AAA==', 'A===', 'AA==A', 'AA-A', 'YQ===', 'YWJj\u00a0']) {
    let expected = null;
    try { expected = atob(sample).length; } catch { /* Invalid Base64 is shown as an error. */ }
    assert.equal(bodyByteLength(sample), expected, `Base64 length must match atob for ${JSON.stringify(sample)}`);
  }
  let seed = 17;
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=\t\r\n !_-';
  for (let trial = 0; trial < 1000; trial++) {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    const length = seed % 32;
    let sample = '';
    for (let index = 0; index < length; index++) {
      seed = (seed * 1664525 + 1013904223) >>> 0;
      sample += alphabet[seed % alphabet.length];
    }
    let expected = null;
    try { expected = atob(sample).length; } catch { /* invalid */ }
    assert.equal(bodyByteLength(sample), expected, `Differential Base64 case ${trial}`);
  }
  assert.equal(bodyByteLength('A'.repeat(8 * 1024 * 1024)), 6 * 1024 * 1024);
  assert.equal(utf8Body(encoded), null, 'binary must never become an editable UTF-8 placeholder');
  assert.equal(utf8Body('!'), null);
  assert.equal(utf8Body(textBody('中文 + 😀\n')), '中文 + 😀\n');
  const larger = Uint8Array.from({ length: 128 * 1024 + 2 }, (_, index) => index % 251);
  assert.equal(bytesBase64(larger), Buffer.from(larger).toString('base64'));

  assert.equal(inferRequestBodyMode(encoded, 'application/octet-stream'), 'file');
  assert.equal(inferRequestBodyMode(textBody('a=one&a=two'), 'Application/X-Www-Form-Urlencoded; charset=UTF-8'), 'form');
  assert.equal(inferRequestBodyMode('aA==', 'multipart/form-data; boundary=test'), 'multipart');
  assert.equal(inferRequestBodyMode(textBody('{"a":1}'), 'application/vnd.test+json'), 'raw');
  assert.equal(inferRequestBodyMode('', ''), 'raw');
  const overBudget = Buffer.alloc(AUTO_EDIT_LIMIT + 1, 65).toString('base64');
  assert.equal(inferRequestBodyMode(overBudget, 'application/json'), 'file', 'large JSON must not decode automatically');
  assert.equal(bodyByteLength(overBudget), AUTO_EDIT_LIMIT + 1);
  assert.equal(inferRequestBodyMode('invalid!', 'text/plain'), 'file', 'invalid Base64 stays available for correction');
  assert.equal(rawBodyParser('Application/Problem+JSON; charset=utf-8'), 'json');
  assert.equal(rawBodyParser('text/html'), 'html');
  assert.equal(rawBodyParser('text/javascript'), 'babel');
  assert.equal(rawBodyParser('text/css'), 'css');
  assert.equal(rawContentType('json', 'application/vnd.test+json; charset=UTF-8'), 'application/vnd.test+json; charset=UTF-8');
  assert.equal(rawContentType('json', 'application/vnd.test+json; profile="schema; charset=preserve"; charset="UTF-16"; version=2'),
    'application/vnd.test+json; profile="schema; charset=preserve"; charset=utf-8; version=2');
  assert.equal(rawContentType('text', 'text/plain; charset=iso-8859-1; format=flowed'), 'text/plain; charset=utf-8; format=flowed');
  assert.equal(rawContentType('html', 'text/html; CHARSET = gbk; charset=latin1'), 'text/html; CHARSET = utf-8');
  assert.equal(rawContentType('json', 'application/json; profile="schema\\\"; charset=preserve"; charset=utf-16'),
    'application/json; profile="schema\\\"; charset=preserve"; charset=utf-8');
  assert.equal(rawContentType('json', 'text/plain'), 'application/json');
  assert.equal(rawContentType('text', 'application/octet-stream'), 'text/plain; charset=utf-8');
  assert.equal(requestContentType([{ name: 'Content-Type', value: 'ignored', disabled: true }, { name: 'content-type', value: 'text/plain' }]), 'text/plain');

  const original = draft();
  const originalSnapshot = structuredClone(original);
  const updated = withRequestBody(original, encoded, 'application/octet-stream');
  assert.deepEqual(original, originalSnapshot, 'body commits must not mutate the parent draft');
  assert.equal(updated.url, original.url);
  assert.equal(updated.bodyBase64, encoded);
  assert.deepEqual(updated.headers.map(header => [header.name, header.value, !!header.disabled]), [
    ['X-Before', 'one', false], ['Content-Type', 'application/octet-stream', false],
    ['X-Between', 'two', false], ['Content-Encoding', 'br', true], ['Content-Type', 'disabled/type', true],
    ['Content-Length', 'disabled length', true], ['X-After', 'three', false],
  ]);
  const advanced = withRequestBody(original, 'unfinished base64 input', undefined, { preserveContentEncoding: true });
  assert.equal(advanced.bodyBase64, 'unfinished base64 input');
  assert.deepEqual(advanced.headers.filter(header => header.name.toLowerCase() === 'content-type'), original.headers.filter(header => header.name.toLowerCase() === 'content-type'));
  assert.deepEqual(advanced.headers.filter(header => header.name.toLowerCase() === 'content-encoding'), original.headers.filter(header => header.name.toLowerCase() === 'content-encoding'));
  assert.equal(updated.headers.some(header => !header.disabled && header.name.toLowerCase() === 'content-encoding'), false,
    'new raw/form/file bytes must not retain stale gzip transport encoding');
  const mimeOnly = withRequestBody(original, original.bodyBase64, 'new/type', { preserveContentEncoding: true });
  assert.equal(mimeOnly.headers.find(header => !header.disabled && header.name.toLowerCase() === 'content-encoding').value, 'gzip');
  assert.throws(() => withRequestBody(original, '', 'text/plain\r\nInjected: yes'), /Content-Type/);

  const fields = [
    { name: 'a', value: 'one' }, { name: 'a', value: 'two' },
    { name: 'space', value: 'a b' }, { name: 'special', value: '+&=%' },
    { name: 'empty', value: '' }, { name: '', value: '中文' },
  ];
  const form = await encodeFormBody(fields, 'form');
  assert.equal(form.contentType, 'application/x-www-form-urlencoded');
  assert.equal(utf8Body(form.bodyBase64), 'a=one&a=two&space=a+b&special=%2B%26%3D%25&empty=&=%E4%B8%AD%E6%96%87');
  assert.deepEqual(await readFormBody(form.bodyBase64, form.contentType, 'form'), fields);
  const captured = textBody('dup=first&middle=a%2Bb+%26&dup=second&empty=&=unnamed');
  assert.deepEqual(await readFormBody(captured, form.contentType, 'form'), [
    { name: 'dup', value: 'first' }, { name: 'middle', value: 'a+b &' },
    { name: 'dup', value: 'second' }, { name: 'empty', value: '' }, { name: '', value: 'unnamed' },
  ]);
  await assert.rejects(readFormBody(textBody('do not silently clear'), 'application/json', 'form'), /not this form/);
  await assert.rejects(readFormBody(encoded, form.contentType, 'form'), /UTF-8/);
  for (const text of ['field=%FF', '%FF=value', 'field=%C0%AF', 'field=%ED%A0%80', 'field=%E4%B8']) {
    await assert.rejects(readFormBody(textBody(text), form.contentType, 'form'), /UTF-8/, text);
  }
  assert.deepEqual(await readFormBody(textBody('utf8=%E4%B8%AD%E6%96%87&literal=%EF%BF%BD&badEscape=%zz+100%25&empty&nul=%00'), form.contentType, 'form'), [
    {name:'utf8',value:'中文'}, {name:'literal',value:'\ufffd'}, {name:'badEscape',value:'%zz 100%'},
    {name:'empty',value:''}, {name:'nul',value:'\0'},
  ]);

  // Captured multipart fixture is constructed independently, including non-UTF8
  // file bytes. Loading must retain repeated fields and the original file.
  const boundary = 'CaptureFixtureBoundary';
  const fixture = Buffer.concat([
    Buffer.from('--' + boundary + '\r\nContent-Disposition: form-data; name="tag"\r\n\r\nfirst\r\n'),
    Buffer.from('--' + boundary + '\r\nContent-Disposition: form-data; name="upload"; filename="original.bin"\r\nContent-Type: application/octet-stream\r\n\r\n'),
    Buffer.from(binary),
    Buffer.from('\r\n--' + boundary + '\r\nContent-Disposition: form-data; name="tag"\r\n\r\nsecond\r\n--' + boundary + '--\r\n'),
  ]).toString('base64');
  const multipart = await readFormBody(fixture, 'multipart/form-data; boundary=' + boundary, 'multipart');
  assert.deepEqual(multipart.map(part => part.name), ['tag', 'upload', 'tag']);
  assert.equal(multipart[0].value, 'first');
  assert.equal(multipart[2].value, 'second');
  assert.equal(multipart[1].file.name, 'original.bin');
  assert.equal(multipart[1].file.type, 'application/octet-stream');
  assert.deepEqual(new Uint8Array(await multipart[1].file.arrayBuffer()), binary);
  const originalRequestArrayBuffer = Request.prototype.arrayBuffer;
  Request.prototype.arrayBuffer = () => { throw new Error('Multipart must use the body stream'); };
  let rebuilt;
  try { rebuilt = await encodeFormBody(multipart, 'multipart'); }
  finally { Request.prototype.arrayBuffer = originalRequestArrayBuffer; }
  assert.match(rebuilt.contentType, /^multipart\/form-data; boundary=/);
  const parsed = await new Response(bodyBytes(rebuilt.bodyBase64), { headers: { 'Content-Type': rebuilt.contentType } }).formData();
  assert.deepEqual([...parsed.keys()], ['tag', 'upload', 'tag']);
  assert.deepEqual(parsed.getAll('tag'), ['first', 'second']);
  assert.deepEqual(new Uint8Array(await parsed.get('upload').arrayBuffer()), binary);
  await assert.rejects(readFormBody(fixture, 'multipart/form-data', 'multipart'));
  await assert.rejects(encodeFormBody(multipart, 'form'), /cannot contain files/);
  assert.deepEqual(await readFormBody('', '', 'multipart'), []);

  const file = new File([binary], 'opaque.bin', { type: 'application/octet-stream' });
  assert.deepEqual(await fileRequestBody(file), { bodyBase64: encoded, contentType: 'application/octet-stream' });
  const streamedBytes = Uint8Array.from({ length: 128 * 1024 + 5 }, (_, index) => index % 251);
  const streamedFile = new Blob([], { type: 'application/octet-stream' });
  streamedFile.arrayBuffer = () => { throw new Error('File must use the body stream'); };
  streamedFile.stream = () => new ReadableStream({
    start(controller) {
      let offset = 0;
      for (const size of [1, 2, 4, 32767, 3, 65537, streamedBytes.length]) {
        if (offset >= streamedBytes.length) break;
        const end = Math.min(offset + size, streamedBytes.length);
        controller.enqueue(streamedBytes.subarray(offset, end));
        offset = end;
      }
      controller.close();
    },
  });
  assert.deepEqual(await fileRequestBody(streamedFile), {
    bodyBase64: Buffer.from(streamedBytes).toString('base64'), contentType: 'application/octet-stream',
  }, 'stream chunks split across Base64 triplets must round-trip exactly');
  const emptyFile = new File([], 'empty.bin');
  assert.deepEqual(await fileRequestBody(emptyFile), { bodyBase64: '', contentType: 'application/octet-stream' });
  const emptyMultipart = await encodeFormBody([{ name: 'empty', value: '', file: emptyFile }], 'multipart');
  const loadedEmpty = await readFormBody(emptyMultipart.bodyBase64, emptyMultipart.contentType, 'multipart');
  assert.equal(loadedEmpty[0].file.name, 'empty.bin');
  assert.equal(loadedEmpty[0].file.size, 0);

  // A slow file result merges into newer request metadata, but must be discarded
  // when a newer body, MIME or operation (including a mode switch) supersedes it.
  const before = { body: original.bodyBase64, mime: requestContentType(original.headers), token: 1 };
  const newerMetadata = { ...original, url: 'https://example.test/new', method: 'PUT',
    headers: [...original.headers, { name: 'X-Late', value: 'preserve' }] };
  assert.equal(requestBodyWorkMatches(before, { body: newerMetadata.bodyBase64, mime: requestContentType(newerMetadata.headers), token: 1 }), true);
  const merged = withRequestBody(newerMetadata, encoded, 'application/octet-stream');
  assert.equal(merged.url, 'https://example.test/new'); assert.equal(merged.method, 'PUT');
  assert.equal(merged.headers.at(-1).name, 'X-Late');
  for (const replacement of [{ body: 'new body' }, { mime: 'text/plain' }, { token: 2 }]) {
    assert.equal(requestBodyWorkMatches(before, { ...before, ...replacement }), false);
  }
  console.log('Request body helpers: binary files, captured forms, repeated fields, MIME and stale-result safety passed');
})().then(() => clearTimeout(watchdog), error => { clearTimeout(watchdog); console.error(error); process.exitCode = 1; });
