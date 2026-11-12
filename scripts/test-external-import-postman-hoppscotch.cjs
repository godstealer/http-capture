const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');

global.crypto ??= require('node:crypto').webcrypto;
const built = buildSync({
  entryPoints: ['apps/frontend/src/externalImportPostmanHoppscotch.ts'],
  bundle: true, platform: 'node', format: 'cjs', write: false,
});
const moduleUnderTest = { exports: {} };
new Function('module', 'exports', built.outputFiles[0].text)(moduleUnderTest, moduleUnderTest.exports);
const { parsePostman, parseHoppscotch } = moduleUnderTest.exports;
const body = request => Buffer.from(request.draft.bodyBase64, 'base64').toString('utf8');

const postman = {
  info: { name: 'API', schema: 'https://schema.getpostman.com/json/collection/v2.1.0/collection.json' },
  auth: { type: 'bearer', bearer: [{ key: 'token', value: '{{token}}' }] },
  variable: [{ key: 'base', value: 'https://api.example.test' }, { key: 'off', value: 'ignored', disabled: true }],
  event: [{ listen: 'prerequest', script: { exec: ['console.log(1)'] } }],
  item: [{ name: 'Users', event: [{ listen: 'test' }], item: [
    { name: 'Create', request: {
      method: 'POST',
      url: { raw: '{{base}}/users?active=1&debug=1', query: [
        { key: 'active', value: '1' }, { key: 'debug', value: '1', disabled: true },
      ] },
      header: [{ key: 'X-Test', value: 'a' }, { key: 'X-Disabled', value: 'b', disabled: true }],
      body: { mode: 'raw', raw: '{"name":"张三"}', options: { raw: { language: 'json' } } },
    } },
    { name: 'Submit form', request: { method: 'POST', url: '{{base}}/form',
      body: { mode: 'urlencoded', urlencoded: [
        { key: 'name', value: 'a b' }, { key: 'unused', value: 'x', disabled: true },
      ] } } },
    { name: 'Upload', request: { method: 'POST', url: '{{base}}/upload',
      body: { mode: 'formdata', formdata: [{ key: 'attachment', type: 'file', src: 'local.bin' }] } } },
  ] }],
};
const importedPostman = parsePostman(JSON.stringify(postman));
assert.equal(importedPostman.source, 'Postman Collection');
assert.deepEqual(importedPostman.environments[0].variables, { base: 'https://api.example.test' });
assert.equal(importedPostman.collections.length, 1);
assert.equal(importedPostman.collections[0].name, 'API / Users');
assert.equal(importedPostman.collections[0].requests.length, 2);
const create = importedPostman.collections[0].requests[0];
assert.equal(create.draft.url, '{{base}}/users?active=1');
assert.equal(create.draft.method, 'POST');
assert.equal(body(create), '{"name":"张三"}');
assert(create.draft.headers.some(header => header.name === 'X-Disabled' && header.disabled));
assert(create.draft.headers.some(header => header.name === 'Authorization' && header.value === 'Bearer {{token}}'));
assert(create.draft.headers.some(header => header.name === 'Content-Type' && header.value === 'application/json'));
assert.equal(body(importedPostman.collections[0].requests[1]), 'name=a%20b');
assert(importedPostman.warnings.some(message => message.includes('Upload') && message.includes('跳过')));
assert(importedPostman.warnings.some(message => message.includes('文件夹级')));
assert(importedPostman.warnings.some(message => message.includes('集合级')));
assert.throws(() => parsePostman('{"info":{"name":"wrong"},"item":[]}'), /不是支持的 Postman/);

const hopp = [{
  v: 12, name: 'Shop', folders: [{ v: 12, name: 'Orders', folders: [],
    requests: [{ v: '17', name: 'Create', method: 'POST',
      endpoint: 'https://example.test/orders?existing=1&disabled=2',
      params: [{ key: 'existing', value: '1', active: true },
        { key: 'disabled', value: '2', active: false },
        { key: 'extra', value: '{{value}}', active: true }],
      headers: [{ key: 'X-Root', value: 'override', active: false }, { key: 'X-Local', value: 'yes', active: true }],
      auth: { authType: 'inherit', authActive: true },
      body: { contentType: 'application/json', body: '{"ok":true}' },
    }, { v: '17', name: 'File', method: 'POST', endpoint: 'https://example.test/upload',
      body: { contentType: 'multipart/form-data', body: [{ key: 'file', active: true, isFile: true, value: [] }] },
    }],
  }], requests: [], headers: [{ key: 'X-Root', value: 'root', active: true }],
  auth: { authType: 'bearer', authActive: true, token: 'secret' },
  variables: [{ key: 'value', initialValue: 'initial', currentValue: 'current', secret: false }],
}];
const importedHopp = parseHoppscotch(JSON.stringify(hopp));
assert.equal(importedHopp.source, 'Hoppscotch');
assert.equal(importedHopp.collections.length, 1);
assert.equal(importedHopp.collections[0].name, 'Shop / Orders');
assert.equal(importedHopp.collections[0].requests.length, 1);
const order = importedHopp.collections[0].requests[0];
assert.equal(order.draft.url, 'https://example.test/orders?existing=1&extra={{value}}');
assert.equal(body(order), '{"ok":true}');
assert(order.draft.headers.some(header => header.name === 'X-Root' && header.disabled));
assert(order.draft.headers.some(header => header.name === 'Authorization' && header.value === 'Bearer secret'));
assert.deepEqual(importedHopp.environments[0].variables, { value: 'current' });
assert(importedHopp.warnings.some(message => message.includes('File') && message.includes('跳过')));
assert.throws(() => parseHoppscotch('{"name":"wrong"}'), /不是 Hoppscotch/);

console.log('Postman and Hoppscotch collection parsing: nested folders, headers, bodies, auth, variables and lossy-skip warnings passed.');
