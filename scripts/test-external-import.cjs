const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');

global.crypto ??= require('node:crypto').webcrypto;
const built = buildSync({ entryPoints: ['apps/frontend/src/externalCollectionImport.ts'], bundle: true, platform: 'node', format: 'cjs', write: false });
const moduleUnderTest = { exports: {} };
new Function('module', 'exports', built.outputFiles[0].text)(moduleUnderTest, moduleUnderTest.exports);
const { mergeExternalPlan, parseExternalImport } = moduleUnderTest.exports;

const draft = { method: 'POST', url: 'https://example.invalid/test', headers: [{ name: 'X-Order', value: 'first' }], bodyBase64: Buffer.from('hello').toString('base64'), tls: { preset: 'native' } };
const library = { revision: 7, collections: [{ id: 'old-collection', name: 'Existing' }], requests: [], environments: [{ id: 'old-env', name: 'Existing env', variables: { key: 'old' } }], scripts: [] };
const plan = { source: 'example', collections: [{ name: 'Imported', requests: [{ name: 'Request', draft }] }], environments: [{ name: 'Imported env', variables: { host: 'example.invalid' } }], warnings: [] };
const originalLibrary = JSON.stringify(library);
const originalPlan = JSON.stringify(plan);
const merged = mergeExternalPlan(library, plan);
assert.equal(JSON.stringify(library), originalLibrary);
assert.equal(JSON.stringify(plan), originalPlan);
assert.equal(merged.revision, library.revision);
assert.equal(merged.collections.length, 2);
assert.equal(merged.requests.length, 1);
assert.equal(merged.environments.length, 2);
assert.equal(merged.requests[0].collectionId, merged.collections[1].id);
assert.deepEqual(merged.requests[0].draft, draft);
assert.deepEqual(merged.environments[0], library.environments[0]);
assert.equal(new Set([...merged.collections, ...merged.requests, ...merged.environments].map(item => item.id)).size, 5);
assert.notEqual(merged.collections[1].id, 'old-collection');
draft.headers[0].value = 'changed after import';
assert.equal(merged.requests[0].draft.headers[0].value, 'first');

assert.throws(() => mergeExternalPlan(library, { ...plan, collections: [{ name: '测'.repeat(171), requests: [] }] }), /512/);
assert.throws(() => mergeExternalPlan(library, { ...plan, collections: [{ name: 'Bad', requests: [{ name: 'Request', draft: { ...draft, bodyBase64: '!' } }] }] }), /Base64/);
assert.throws(() => mergeExternalPlan(library, { ...plan, collections: [{ name: 'Bad', requests: [{ name: 'Request', draft: { ...draft, headers: [{ name: 1, value: 'bad' }] } }] }] }), /Invalid saved request/);
assert.throws(() => mergeExternalPlan(library, { ...plan, collections: [{ name: 'Bad', requests: [] }], environments: [{ name: 'Bad env', variables: { key: 3 } }] }), /环境变量/);
assert.throws(() => mergeExternalPlan(library, { source: 'empty', collections: [], warnings: [] }), /没有可导入/);
assert.throws(() => parseExternalImport('x'.repeat(8 * 1024 * 1024 + 1), 'postman'), /8 MiB/);
assert.throws(() => mergeExternalPlan(library, { ...plan, collections: [{ name: 'Large', requests: [{ name: 'Big', draft: { ...draft, bodyBase64: Buffer.alloc(8 * 1024 * 1024 + 1).toString('base64') } }] }] }), /8 MiB/);
const nearlyFull = structuredClone(library);
nearlyFull.environments[0].variables.padding = '';
nearlyFull.environments[0].variables.padding = 'x'.repeat(8 * 1024 * 1024 - Buffer.byteLength(JSON.stringify(nearlyFull)) - 30);
assert(Buffer.byteLength(JSON.stringify(nearlyFull)) < 8 * 1024 * 1024);
assert.throws(() => mergeExternalPlan(nearlyFull, plan), /导入后集合数据超过 8 MiB/);

const curl = parseExternalImport('curl -X POST https://example.invalid/test -H "Content-Type: text/plain" --data-raw hello', 'curl');
assert.equal(curl.collections.length, 1);
assert.equal(curl.collections[0].requests.length, 1);
assert.equal(curl.collections[0].requests[0].draft.method, 'POST');
assert.equal(Buffer.from(curl.collections[0].requests[0].draft.bodyBase64, 'base64').toString(), 'hello');
assert.equal(mergeExternalPlan(library, curl).requests.length, 1);

console.log('External collection import: isolated append, fresh IDs, validation, 8 MiB limits and cURL dispatch passed.');
