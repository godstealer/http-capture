const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');

const built = buildSync({ entryPoints: ['apps/frontend/src/externalImportOther.ts'], bundle: true, platform: 'node', format: 'cjs', write: false });
const imported = { exports: {} };
new Function('module', 'exports', built.outputFiles[0].text)(imported, imported.exports);
const { parseOpenApi, parseInsomnia, parseHarCollection } = imported.exports;
const decoded = draft => Buffer.from(draft.bodyBase64, 'base64').toString('utf8');

const openapi = parseOpenApi(JSON.stringify({
  openapi: '3.1.0', info: { title: 'Example API' },
  servers: [{ url: 'https://{host}/api', variables: { host: { default: 'example.test' } } }],
  components: { parameters: { id: { name: 'id', in: 'path', schema: { type: 'string' }, example: 7 } } },
  paths: { '/items/{id}': { post: {
    summary: 'Create item', parameters: [{ $ref: '#/components/parameters/id' }, { name: 'q', in: 'query', schema: { type: 'string' } },
      { name: 'session', in: 'cookie', example: 'abc' }, { name: 'mode', in: 'cookie', example: 'fast' }],
    requestBody: { content: { 'application/json': { example: { title: 'ok' } } } },
  } } },
}));
assert.equal(openapi.collections[0].name, 'Example API');
assert.equal(openapi.collections[0].requests.length, 1);
const apiRequest = openapi.collections[0].requests[0];
assert.equal(apiRequest.name, 'Create item');
assert.equal(apiRequest.draft.url, 'https://example.test/api/items/7?q={{q}}');
assert.equal(apiRequest.draft.headers.find(h => h.name === 'Cookie').value, 'session=abc; mode=fast');
assert.equal(apiRequest.draft.headers.find(h => h.name === 'Content-Type').value, 'application/json');
assert.deepEqual(JSON.parse(decoded(apiRequest.draft)), { title: 'ok' });
assert(openapi.warnings.some(w => w.includes('示例值')));
assert.throws(() => parseOpenApi('{"swagger":"2.0"}'), /Swagger 2.0/);
const incompleteSpec = parseOpenApi(JSON.stringify({ openapi: '3.0.3', info: { title: 'Incomplete' }, paths: {
  '/files/{fileId}': { post: { requestBody: { content: { 'multipart/form-data': { schema: { type: 'object' } } } } } },
} }));
assert.equal(incompleteSpec.collections[0].requests[0].draft.url, '{{baseUrl}}/files/{{fileId}}');
assert(incompleteSpec.warnings.some(w => w.includes('文件或二进制')));
assert(incompleteSpec.warnings.some(w => w.includes('路径包含')));

const insomnia = parseInsomnia(JSON.stringify({ __export_format: 4, resources: [
  { _id: 'workspace', _type: 'workspace', name: 'Workspace' },
  { _id: 'folder', _type: 'request_group', parentId: 'workspace', name: 'Users' },
  { _id: 'request', _type: 'request', parentId: 'folder', name: 'Create', method: 'POST', url: 'https://example.test/users',
    headers: [{ name: 'X-Enabled', value: 'yes' }, { name: 'X-Disabled', value: 'no', disabled: true }],
    parameters: [{ name: 'dryRun', value: '1' }], body: { mimeType: 'application/json', text: '{"id":7}' } },
] }));
assert.equal(insomnia.collections[0].name, 'Workspace');
assert.equal(insomnia.collections[0].requests[0].name, 'Users / Create');
assert.equal(insomnia.collections[0].requests[0].draft.url, 'https://example.test/users?dryRun=1');
assert.equal(insomnia.collections[0].requests[0].draft.headers[1].disabled, true);
assert.equal(decoded(insomnia.collections[0].requests[0].draft), '{"id":7}');
assert.throws(() => parseInsomnia('{"type":"collection.insomnia.rest/5.0"}'), /v5 YAML/);
assert.throws(() => parseInsomnia('type: collection.insomnia.rest/5.0\nname: Requests'), /v5 YAML/);

const har = parseHarCollection(JSON.stringify({ log: { version: '1.2', entries: [{
  request: { method: 'POST', url: 'https://example.test/form', headers: [{ name: 'Accept', value: '*/*' }],
    cookies: [{ name: 'sid', value: 'abc' }], postData: { mimeType: 'application/x-www-form-urlencoded', params: [{ name: 'a', value: 'two words' }] } },
  response: { status: 200 },
}] } }));
assert.equal(har.collections[0].requests.length, 1);
assert.equal(har.collections[0].requests[0].draft.method, 'POST');
assert.equal(decoded(har.collections[0].requests[0].draft), 'a=two%20words');
assert.equal(har.collections[0].requests[0].draft.headers.find(h => h.name === 'Cookie').value, 'sid=abc');
assert(har.warnings.some(w => w.includes('响应')));
assert.throws(() => parseHarCollection('{"log":{}}'), /log.entries/);
const binaryHar = parseHarCollection(JSON.stringify({ log: { entries: [{ request: {
  method: 'POST', url: 'https://example.test/upload',
  headers: [{ name: 'X-Order', value: 'first' }, { name: 'X-Order', value: 'second' }],
  postData: { text: 'AAEC', encoding: 'base64' },
} }] } }));
assert.deepEqual(binaryHar.collections[0].requests[0].draft.headers.map(h => h.value), ['first', 'second']);
assert.equal(binaryHar.collections[0].requests[0].draft.bodyBase64, 'AAEC');

console.log('OpenAPI, Insomnia v4 JSON and HAR 1.2 request import passed.');
