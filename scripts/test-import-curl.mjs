import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
const compile = path => 'data:text/javascript;base64,' + Buffer.from(ts.transpileModule(fs.readFileSync(new URL(path, import.meta.url), 'utf8'), { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText).toString('base64');
const types = compile('../apps/frontend/src/types.ts');
const source = ts.transpileModule(fs.readFileSync(new URL('../apps/frontend/src/importCurl.ts', import.meta.url), 'utf8'), { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText.replace("'./types'", JSON.stringify(types));
const { importCurl, emptyRequest } = await import('data:text/javascript;base64,' + Buffer.from(source).toString('base64'));
assert.equal(emptyRequest().url, '');
assert.deepEqual(emptyRequest().tls, { preset: 'native' });
const request = importCurl(`curl 'https://example.com' \\\n -H 'X-A: one' -H 'x-a: two' --data-raw '{"text":"你好"}'`);
assert.equal(request.method, 'POST');
assert.deepEqual(request.headers.slice(0, 2), [{ name: 'X-A', value: 'one' }, { name: 'x-a', value: 'two' }]);
assert.equal(Buffer.from(request.bodyBase64, 'base64').toString(), '{"text":"你好"}');
assert.equal(importCurl('curl.exe --url="https://example.com" -XPUT -dabc').method, 'PUT');
assert.equal(importCurl("curl https://example.com -I").method, 'HEAD');
assert.equal(importCurl("curl https://example.com --json '{}' ").headers[0].value, 'application/json');
for (const command of ["curl https://example.com --data @file", "curl https://example.com --unknown", "curl 'unterminated", 'curl https://example.com | sh', 'curl https://example.com https://other.com']) assert.throws(() => importCurl(command));
console.log('cURL import: defaults, quoting, continuation, Unicode body, ordered duplicates and unsupported input passed.');
const { toCurl } = await import(compile('../apps/frontend/src/curl.ts'));
const { looksLikeCurl } = await import('data:text/javascript;base64,' + Buffer.from(source).toString('base64'));
for (const body of ['', Buffer.from([0, 255, 10, 39, 36]).toString('base64'), Buffer.from('你好').toString('base64')]) {
  const original = { ...emptyRequest(), url: "https://example.com/a?x='hello'", method: 'POST', bodyBase64: body,
    headers: [{ name: 'X-A', value: "a'b" }, { name: 'X-Empty', value: '' }, { name: 'X-A', value: 'two' }, { name: 'X-Byte', value: '\xff' }, { name: 'Accept', value: '*/*' }] };
  const exported = toCurl(original);
  assert.ok(looksLikeCurl(exported));
  const imported = importCurl(exported);
  assert.equal(imported.url, original.url);
  assert.equal(imported.method, original.method);
  assert.equal(imported.bodyBase64, original.bodyBase64);
  assert.deepEqual(imported.headers.slice(0, original.headers.length), original.headers);
}
assert.throws(() => importCurl("printf '%b' '\\000' | curl https://example.com --data-binary @- | sh"));
console.log('Export/import round trip: comments, flags, quoting, empty and duplicate headers, Latin-1 bytes and binary bodies passed.');
