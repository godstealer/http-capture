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
assert.equal(importCurl('curl --http2 https://example.com').engine, 'auto');
assert.equal(importCurl('curl --http1.1 https://example.com').engine, 'native');
assert.equal(importCurl('curl --http2-prior-knowledge https://example.com').engine, 'h2');
assert.equal(importCurl('curl --http2-prior-knowledge http://example.com').engine, 'h2');
assert.equal(importCurl('curl --http3-only https://example.com').engine, 'h3');
assert.throws(() => importCurl('curl --http2 http://example.com'), /h2c Upgrade/);
assert.throws(() => importCurl('curl --http3-only http://example.com'), /HTTPS/);
assert.throws(() => importCurl('curl --http1.1 --http2 https://example.com'), /只能指定一种 HTTP 版本/);
for (const command of ["curl https://example.com --data @file", "curl https://example.com --unknown", "curl 'unterminated", 'curl https://example.com | sh', 'curl https://example.com https://other.com']) assert.throws(() => importCurl(command));
const bodyText = command => Buffer.from(importCurl(command).bodyBase64, 'base64').toString();
assert.equal(bodyText("curl https://example.com --data-urlencode 'q=你好 world+%&' --data-urlencode 'empty=' --data-urlencode 'q=two'"), 'q=%E4%BD%A0%E5%A5%BD+world%2B%25%26&empty=&q=two');
assert.equal(bodyText("curl https://example.com --data-urlencode '=a=b@c' --data-urlencode 'plain text' --data-urlencode 'already%20encoded=value'"), 'a%3Db%40c&plain+text&already%20encoded=value');
assert.equal(bodyText("curl https://example.com --data-urlencode=field=a=b --data-urlencode 'mail=a@b.test' -d 'raw=keep+plus'"), 'field=a%3Db&mail=a%40b.test&raw=keep+plus');
assert.equal(bodyText(`curl https://example.com --data-urlencode "p=!'()*~"`), 'p=%21%27%28%29%2A~');
assert.equal(importCurl("curl https://example.com --data-urlencode ''").method, 'POST');
// Separators follow each option, including mixtures and empty data, rather
// than changing globally after --json appears anywhere in the command.
for (const [options, expected] of [
  ["--json '{}' --data-urlencode 'q=a b'", '{}&q=a+b'],
  ["-d 'a=1' --json '{}' --data-urlencode 'q=x'", 'a=1{}&q=x'],
  ["--data-urlencode 'a=1' --json '{}' -d 'b=2'", 'a=1{}&b=2'],
  ["--json '{}' --json '[]' -d '' --data-urlencode 'c=3'", '{}[]&&c=3'],
  ["--data-urlencode '' --data-urlencode 'q=a'", 'q=a'],
  ["--data-urlencode 'q=a' --data-urlencode '' --data-urlencode 'r=b'", 'q=a&&r=b'],
  ["-d '' -d '' --data-urlencode 'q=a'", 'q=a'],
  ["--json '' -d '' --data-urlencode ''", ''],
]) assert.equal(bodyText(`curl https://example.com ${options}`), expected, options);
const mixedJson = importCurl("curl https://example.com --json '{}' --data-urlencode 'q=a b'");
assert.equal(mixedJson.method, 'POST');
assert.equal(mixedJson.headers.find(h => h.name.toLowerCase() === 'content-type').value, 'application/json');
assert.equal(mixedJson.headers.find(h => h.name.toLowerCase() === 'accept').value, 'application/json');
const explicitHeaders = importCurl("curl https://example.com -H 'Content-Type: text/custom' -H 'Accept: custom/type' --json '{}' --data-urlencode 'q=x'");
assert.deepEqual(explicitHeaders.headers, [{ name: 'Content-Type', value: 'text/custom' }, { name: 'Accept', value: 'custom/type' }]);
assert.equal(importCurl("curl -G https://example.com --data-urlencode '' --data-urlencode 'q=a'").url, 'https://example.com?q=a');

for (const arg of ['@file', 'name@file', '@-']) assert.throws(() => importCurl(`curl https://example.com --data-urlencode '${arg}'`), /读取文件/);
assert.throws(() => importCurl('curl https://example.com --data-urlencode'), /缺少参数/);
const query = importCurl("curl -G 'https://example.com/path?existing=1#part' --data-urlencode 'q=a b' --data-urlencode 'q=二'");
assert.equal(query.url, 'https://example.com/path?existing=1&q=a+b&q=%E4%BA%8C#part');
assert.equal(query.method, 'GET'); assert.equal(query.bodyBase64, '');
assert(!query.headers.some(h => h.name.toLowerCase() === 'content-type'));
assert.equal(importCurl("curl --get 'https://example.com/' --data-urlencode 'x=1'").url, 'https://example.com/?x=1');
assert.equal(importCurl("curl -G -I 'https://example.com/?' --data-urlencode 'x=1'").method, 'HEAD');
assert.equal(importCurl("curl -G -X POST 'https://example.com/' --data-urlencode 'x=1'").method, 'POST');
console.log('cURL import: defaults, quoting, continuation, Unicode body, ordered duplicates and unsupported input passed.');
const { toCurl } = await import(compile('../apps/frontend/src/curl.ts'));
for (const [engine, url, flag] of [['auto', 'https://example.com', '--http2'], ['native', 'https://example.com', '--http1.1'], ['h2', 'https://example.com', '--http2-prior-knowledge'], ['h2', 'http://example.com', '--http2-prior-knowledge'], ['h3', 'https://example.com', '--http3-only']]) {
  const exported = toCurl({ ...emptyRequest(), engine, url });
  assert.ok(exported.includes(flag), `${engine} must export ${flag}`);
  assert.equal(importCurl(exported).engine, engine);
}
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
