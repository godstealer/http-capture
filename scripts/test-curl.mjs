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

const disabledRequest={...request,bodyBase64:'',headers:[
 {name:'X-Dup',value:'first'},
 {name:'Connection',value:'X-Keep, X-Dup',disabled:true},
 {name:'X-Dup',value:'disabled duplicate',disabled:true},
 {name:'x-dup',value:'second',disabled:false},
 {name:'X-Keep',value:'preserved'},
 {name:'X-Off',value:'disabled value',disabled:true},
]};
const disabledBefore=JSON.stringify(disabledRequest);
const disabledCommand=toCurl(disabledRequest);
assert.ok(disabledCommand.includes("--header 'X-Dup: first' --header 'x-dup: second' --header 'X-Keep: preserved'"));
assert.ok(!disabledCommand.includes('disabled duplicate'));
assert.ok(!disabledCommand.includes('disabled value'));
assert.ok(!disabledCommand.includes("--header 'Connection:"));
assert.equal(JSON.stringify(disabledRequest),disabledBefore);
const disabledContentType=toCurl({...request,headers:[{name:'Content-Type',value:'application/json',disabled:true}]});
assert.ok(!disabledContentType.includes('application/json'));
assert.ok(disabledContentType.includes("--header 'Content-Type:'"));
console.log('cURL disabled headers: disabled Connection tokens do not hide enabled fields, duplicate order/case survive and disabled Content-Type is not sent.');
