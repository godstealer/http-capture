import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
const source = fs.readFileSync(new URL('../apps/frontend/src/curl.ts', import.meta.url), 'utf8');
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
const { toCurl } = await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`);
const request = { method: 'POST', url: "https://example.test/a?x='$(whoami)&y=[1]", engine: 'native', tls: { preset: 'native' },
  headers: [{ name: 'X-A', value: 'one' }, { name: 'X-B', value: "a'b" }, { name: 'x-a', value: 'three' },
    { name: 'Connection', value: 'X-Hop' }, { name: 'X-Hop', value: 'remove-me' },
    { name: 'Content-Length', value: '99' }, { name: 'X-Empty', value: '' }, { name: 'X-Byte', value: '\xff' }],
  bodyBase64: Buffer.from([0, 255, 10, 39, 36]).toString('base64') };
const before = JSON.stringify(request);
const command = toCurl(request);
assert.ok(command.includes("printf '%b' '\\000\\377\\012\\047\\044' | curl"));
assert.ok(command.includes("'https://example.test/a?x='\"'\"'$(whoami)&y=[1]'"));
assert.ok(command.includes("--header 'X-A: one' --header 'X-B: a'\"'\"'b' --header 'x-a: three'"));
assert.ok(command.includes("--header 'X-Empty;'"));
assert.ok(command.includes('\\377')); // Binary header value remains a byte.
assert.ok(!command.includes('remove-me') && !command.includes('Content-Length:'));
assert.ok(command.includes('--data-binary @-'));
assert.equal(JSON.stringify(request), before);
assert.ok(!toCurl({ ...request, bodyBase64: '' }).includes('--data-binary'));
console.log('cURL export: shell quoting, ordered duplicates, binary body, framing and immutable input passed.');
