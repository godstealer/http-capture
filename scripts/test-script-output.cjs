const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');

// Exercise the actual JavaScript template embedded in the Rust script runner.
const source = readFileSync('crates/capture-core/src/scripts.rs', 'utf8');
const match = source.match(/let source=format!\(r#"([\s\S]*?)"#\);/);
assert(match, 'script result template exists');
function execute(code, request, response) {
  const template = match[1].replaceAll('{{', '{').replaceAll('}}', '}').replace('{code}', code);
  const withoutBody = value => value == null ? null : Object.fromEntries(Object.entries(value).filter(([key]) => key !== 'bodyBase64').sort(([a], [b]) => a.localeCompare(b)));
  const input = JSON.stringify({ request: withoutBody(request), response: withoutBody(response), variables: {} });
  assert(!input.includes(request.bodyBase64), 'input metadata excludes the complete body');
  const result = new Function('__input', '__requestBody', '__responseBody', `return ${template};`)(input, request.bodyBase64, response?.bodyBase64 ?? '');
  return JSON.parse(result);
}

const body = 'QUJD'.repeat(256 * 1024);
const request = { bodyBase64: body, headers: [], method: 'POST', url: 'https://example.invalid/' };
const response = { bodyBase64: body, headers: [], status: 200, version: 'HTTP/2' };
const unchanged = execute("assert(request.bodyBase64.length===1048576);assert(Object.keys(request).join(',')==='bodyBase64,headers,method,url');assert(Object.keys(response).join(',')==='bodyBase64,headers,status,version');request.headers.push({name:'X-Test',value:'yes'});", request, response);
assert.equal(unchanged.__requestBodyUnchanged, true);
assert.equal(unchanged.__responseBodyUnchanged, true);
assert.equal(unchanged.request.bodyBase64, '');
assert.equal(unchanged.response.bodyBase64, '');
assert.equal(unchanged.request.headers[0].name, 'X-Test');
assert(Buffer.byteLength(JSON.stringify(unchanged)) < 1024, 'unchanged megabyte bodies do not enter the result JSON');

const changed = execute("request.bodyBase64='YQ==';response.bodyBase64='Yg==';", request, response);
assert.equal(changed.__requestBodyUnchanged, false);
assert.equal(changed.__responseBodyUnchanged, false);
assert.equal(changed.request.bodyBase64, 'YQ==');
assert.equal(changed.response.bodyBase64, 'Yg==');
const deleted = execute('delete request.bodyBase64;', request, null);
assert.equal(deleted.__requestBodyUnchanged, false);
assert.equal(Object.hasOwn(deleted.request, 'bodyBase64'), false);
console.log('Script result envelope: unchanged large bodies stay script-visible but are omitted from result JSON; edits and deletion remain explicit.');
