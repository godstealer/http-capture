const { buildSync } = require('esbuild');
const assert = require('node:assert/strict');

const loaded = { exports: {} };
new Function('module', 'exports', buildSync({
  entryPoints: ['apps/frontend/src/headerText.ts'], bundle: true, platform: 'node', format: 'cjs', write: false,
}).outputFiles[0].text)(loaded, loaded.exports);
const { parseHeaderText, parseHeaders, serializeHeaders, HeaderTextError } = loaded.exports;
const h = (name, value) => ({ name, value });

// Raw requests pasted from an HTTP inspector retain separate Cookie entries and
// URL/authorization values containing colons; the request line is not a header.
const raw = 'POST /search?day=tomorrow HTTP/1.1\r\nHost: www.ana.co.jp\r\nCookie: first=1\r\nx-Link: https://example.test:8443/a:b  \r\nCookie: second=2\r\nX-Empty:\r\n\r\n';
const expected = [h('Host', 'www.ana.co.jp'), h('Cookie', 'first=1'), h('x-Link', 'https://example.test:8443/a:b  '), h('Cookie', 'second=2'), h('X-Empty', '')];
assert.deepEqual(parseHeaders(raw), expected);
assert.deepEqual(parseHeaders('HTTP/1.1 200 OK\nContent-Type: text/plain\nX-Empty:'), [h('Content-Type', 'text/plain'), h('X-Empty', '')]);
assert.deepEqual(parseHeaderText(''), { headers: [], pseudoHeaders: [] });
assert.deepEqual(parseHeaders(' \t\r\n\n'), []);

// Actual DevTools selection format alternates a name and a value, including
// pseudo headers. A value line may itself contain a colon.
const devtools = ':authority\nanamall.anx-personalize.ana.co.jp\n:method\nGET\n:path\n/ja/jp/gadget/anaMallPersonalize.js\n:scheme\nhttps\naccept\n*/*\nx-url\nhttps://www.ana.co.jp:443/search\ncookie\na=1\ncookie\nb=2\nx-empty\n\n';
const parsedDevtools = parseHeaderText(devtools);
assert.deepEqual(parsedDevtools.pseudoHeaders, [h(':authority', 'anamall.anx-personalize.ana.co.jp'), h(':method', 'GET'), h(':path', '/ja/jp/gadget/anaMallPersonalize.js'), h(':scheme', 'https')]);
assert.deepEqual(parsedDevtools.headers, [h('accept', '*/*'), h('x-url', 'https://www.ana.co.jp:443/search'), h('cookie', 'a=1'), h('cookie', 'b=2'), h('x-empty', '')]);
assert.deepEqual(parseHeaders(devtools), parsedDevtools.headers);
assert.deepEqual(parseHeaderText(':method: POST\r\n:scheme: https\r\n:path: /search\r\n:authority: www.ana.co.jp:443\r\nX-Key: value'), {
  pseudoHeaders: [h(':method', 'POST'), h(':scheme', 'https'), h(':path', '/search'), h(':authority', 'www.ana.co.jp:443')], headers: [h('X-Key', 'value')],
});

const editable = [h('Cookie', 'first=1'), { ...h('X-Off', 'keep in editor'), disabled: true }, { ...h('x-URL', 'https://example.test:8443/path  '), disabled: false }, h('Cookie', 'second=2'), h('X-Empty', '')];
assert.equal(serializeHeaders(editable), 'Cookie: first=1\r\nx-URL: https://example.test:8443/path  \r\nCookie: second=2\r\nX-Empty: ');
assert.equal(serializeHeaders(editable, '\n'), 'Cookie: first=1\nx-URL: https://example.test:8443/path  \nCookie: second=2\nX-Empty: ');
assert.deepEqual(parseHeaders(serializeHeaders(editable)), editable.filter(header => !header.disabled).map(({ name, value }) => h(name, value)));
assert.equal(serializeHeaders([{ name: 'X-Disabled', value: 'not\r\nsent', disabled: true }]), '');

function rejects(action, code, line, name) {
  assert.throws(action, error => {
    assert.ok(error instanceof HeaderTextError);
    assert.equal(error.name, 'HeaderTextError');
    assert.equal(error.code, code);
    assert.equal(error.line, line);
    if (name !== undefined) assert.equal(error.headerName, name);
    return true;
  });
}
rejects(() => parseHeaders('Host: example.test\nthis is not a header\nAccept: */*'), 'invalidLine', 2);
rejects(() => parseHeaders('Host: example.test\nMissingColon\nAccept: */*'), 'invalidLine', 2);
rejects(() => parseHeaders('Host: example.test\nBad Name: value'), 'invalidName', 2, 'Bad Name');
rejects(() => parseHeaders('Host: example.test\nX-Header : value'), 'invalidName', 2, 'X-Header ');
rejects(() => parseHeaders('Host: example.test\n folded-value'), 'invalidLine', 2);
rejects(() => parseHeaders('Host: example.test\n injected: value'), 'invalidName', 2, ' injected');
rejects(() => parseHeaders('accept\n*/*\nx-missing\n'), 'missingValue', 3, 'x-missing');
rejects(() => parseHeaders(':Method: GET'), 'invalidName', 1, ':Method');
rejects(() => parseHeaders('Host: example.test\nX-Value: one\rinjected: two'), 'invalidLineEnding', 2);
rejects(() => parseHeaders('Host: example.test\nX-Value: one\0two'), 'invalidValue', 2);
rejects(() => parseHeaders('Host: example.test\nX-Value: one\x7ftwo'), 'invalidValue', 2);
rejects(() => parseHeaders('\v'), 'invalidValue', 1);
rejects(() => parseHeaders('\f\n'), 'invalidValue', 1);
rejects(() => serializeHeaders([h('Good', 'ok'), h('X-Bad', 'one\r\nInjected: two')]), 'invalidValue', 2, 'X-Bad');
rejects(() => serializeHeaders([h('X-Bad\r\nInjected', 'value')]), 'invalidName', 1, 'X-Bad\r\nInjected');
rejects(() => serializeHeaders([h('X-Bad', 'one\0two')]), 'invalidValue', 1, 'X-Bad');
rejects(() => serializeHeaders([h(':method', 'POST')]), 'invalidName', 1, ':method');
rejects(() => serializeHeaders([h('X-Value', 'ok')], '\r\nInjected: x\r\n'), 'invalidLineEnding', 1);

console.log('Header text: raw/DevTools import, separate pseudo headers, duplicate order/case, empty/colon values, disabled filtering and line-numbered injection checks passed.');
