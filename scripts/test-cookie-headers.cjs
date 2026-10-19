const { buildSync } = require('esbuild');
const assert = require('node:assert/strict');

const loaded = { exports: {} };
new Function('module', 'exports', buildSync({
  entryPoints: ['apps/frontend/src/cookieHeaders.ts'], bundle: true, platform: 'node', format: 'cjs', write: false,
}).outputFiles[0].text)(loaded, loaded.exports);
const { cookieHeaderActions, splitCookieHeader, mergeCookieHeaders } = loaded.exports;
const h = (name, value, ...disabled) => ({ name, value, ...(disabled.length ? { disabled: disabled[0] } : {}) });
const freeze = headers => Object.freeze(headers.map(header => Object.freeze(header)));

// A browser's single Cookie field can become one field per pair. Repeated keys,
// empty values, '=' in tokens and encoded octets must survive verbatim.
const source = freeze([
  h('X-Before', 'one'),
  h('cOoKiE', 'same=1; same=2; base64=YWJj==; encoded=%3B%2C%3D; empty=; comma=a,b'),
  h('Set-Cookie', 'response=1; Path=/'), h('X-After', 'two'),
]);
const snapshot = JSON.stringify(source);
assert.deepEqual(cookieHeaderActions(source, 1), { canSplit: true, canMerge: false, mergeCount: 1 });
const expanded = splitCookieHeader(source, 1);
assert.deepEqual(expanded, [source[0], ...['same=1', 'same=2', 'base64=YWJj==', 'encoded=%3B%2C%3D', 'empty=', 'comma=a,b'].map(value => h('cOoKiE', value)), source[2], source[3]]);
assert.equal(expanded[0], source[0]);
assert.equal(expanded.at(-1), source.at(-1));
assert.deepEqual(mergeCookieHeaders(expanded, 4), source);
assert.equal(JSON.stringify(source), snapshot);

// Only the selected row expands. Preserve explicit false/true as well as an
// omitted disabled flag, rather than silently enabling a disabled credential.
for (const state of [[], [false], [true]]) {
  const headers = freeze([h('Cookie', 'first=1; second=2', ...state), h('cookie', 'other=3; last=4', true)]);
  assert.deepEqual(splitCookieHeader(headers, 0), [h('Cookie', 'first=1', ...state), h('Cookie', 'second=2', ...state), headers[1]]);
}

// Quoted and escaped semicolons are data. Apostrophes are ordinary cookie
// characters, commas are never a delimiter, and value whitespace is preserved.
for (const [value, expected] of [
  ['quoted="one;two"; next=3', ['quoted="one;two"', 'next=3']],
  ['quoted="one\\\";two"; next=3', ['quoted="one\\\";two"', 'next=3']],
  ['escaped=one\\;two; next=3', ['escaped=one\\;two', 'next=3']],
  ["name=O'Reilly; empty=", ["name=O'Reilly", 'empty=']],
  ['spaces=  x  ; next= y ', ['spaces=  x  ', 'next= y ']],
]) {
  const headers = freeze([h('Cookie', value)]);
  assert.equal(cookieHeaderActions(headers, 0).canSplit, true, value);
  assert.deepEqual(splitCookieHeader(headers, 0), expected.map(pair => h('Cookie', pair)), value);
}
assert.deepEqual(cookieHeaderActions([h('Cookie', 'quoted="one;two"')], 0), { canSplit: false, canMerge: false, mergeCount: 1 });
assert.deepEqual(splitCookieHeader([h('Cookie', 'comma=a,b=c')], 0), [h('Cookie', 'comma=a,b=c')]);

// Merge at the first matching row, even when a later Cookie is selected. Fields
// in the other enabled group and response Set-Cookie rows remain untouched.
const mixed = freeze([
  h('X-Start', 'one'), h('cOoKiE', 'same=1'), h('Cookie', 'disabled=secret', true),
  h('Accept', '*/*'), h('COOKIE', 'same=2', false), h('Set-Cookie', 'server=3; Path=/'),
  h('cookie', 'disabled=other', true), h('cookie', 'empty='), h('X-End', 'two'),
]);
assert.deepEqual(cookieHeaderActions(mixed, 4), { canSplit: false, canMerge: true, mergeCount: 3 });
assert.deepEqual(mergeCookieHeaders(mixed, 4), [mixed[0], h('cOoKiE', 'same=1; same=2; empty='), mixed[2], mixed[3], mixed[5], mixed[6], mixed[8]]);
assert.deepEqual(cookieHeaderActions(mixed, 6), { canSplit: false, canMerge: true, mergeCount: 2 });
assert.deepEqual(mergeCookieHeaders(mixed, 6), [mixed[0], mixed[1], h('Cookie', 'disabled=secret; disabled=other', true), mixed[3], mixed[4], mixed[5], mixed[7], mixed[8]]);
assert.deepEqual(mergeCookieHeaders(freeze([h('COOKIE', 'a=1', false), h('cookie', 'b=2')]), 1), [h('COOKIE', 'a=1; b=2', false)]);
assert.deepEqual(mergeCookieHeaders(freeze([h('Cookie', 'a="1;2"'), h('Cookie', 'b=3')]), 0), [h('Cookie', 'a="1;2"; b=3')]);
const shared = Object.freeze(h('Cookie', 'repeated=1'));
assert.deepEqual(mergeCookieHeaders(Object.freeze([shared, shared]), 1), [h('Cookie', 'repeated=1; repeated=1')]);

// Reject the whole operation for malformed data; never partially split a valid
// prefix or join an invalid same-state field. An invalid disabled group cannot
// prevent a separate valid enabled group from being merged.
const invalid = [
  '', ' ', 'a=1;', ';a=1', 'a=1;; b=2', 'a=1; missing', '=1; b=2',
  'bad name=1; b=2', 'a=1; b="unclosed', 'a=1; b=trailing\\',
  'a=1\r\nInjected: yes; b=2', 'a=1\0; b=2', 'a=1\t; b=2', 'a=1\x7f; b=2',
];
for (const value of invalid) {
  const headers = freeze([h('Cookie', 'valid=1'), h('Cookie', value), h('X-Keep', 'two')]);
  const original = JSON.stringify(headers);
  assert.equal(cookieHeaderActions(headers, 1).canSplit, false, value);
  assert.equal(cookieHeaderActions(headers, 0).canMerge, false, value);
  assert.deepEqual(splitCookieHeader(headers, 1), headers, value);
  assert.deepEqual(mergeCookieHeaders(headers, 0), headers, value);
  assert.equal(JSON.stringify(headers), original);
}
const separate = freeze([h('Cookie', 'a=1'), h('Cookie', 'broken', true), h('cookie', 'b=2', false)]);
assert.equal(cookieHeaderActions(separate, 0).canMerge, true);
assert.deepEqual(mergeCookieHeaders(separate, 2), [h('Cookie', 'a=1; b=2'), separate[1]]);

// Exhaustive control-byte rejection also covers balanced quotes. A state change
// must never become a way to move control characters into a generated header.
for (const byte of [...Array(32).keys(), 127]) {
  const headers = freeze([h('Cookie', `a="x${String.fromCharCode(byte)}y"; b=2`)]);
  assert.equal(cookieHeaderActions(headers, 0).canSplit, false, String(byte));
  assert.deepEqual(splitCookieHeader(headers, 0), headers, String(byte));
}

// Non-Cookie fields, invalid indexes and groups of one always remain unchanged.
for (const headers of [source, freeze([])]) {
  for (const index of [-1, headers.length, 0.5, NaN, Infinity]) {
    assert.deepEqual(cookieHeaderActions(headers, index), { canSplit: false, canMerge: false, mergeCount: 0 });
    assert.deepEqual(splitCookieHeader(headers, index), headers);
    assert.deepEqual(mergeCookieHeaders(headers, index), headers);
  }
}
for (const name of ['Set-Cookie', 'X-Cookie', ' Cookie']) {
  const headers = freeze([h(name, 'a=1; b=2'), h('Cookie', 'c=3')]);
  assert.deepEqual(cookieHeaderActions(headers, 0), { canSplit: false, canMerge: false, mergeCount: 0 });
  assert.deepEqual(splitCookieHeader(headers, 0), headers);
  assert.deepEqual(mergeCookieHeaders(headers, 0), headers);
  assert.deepEqual(mergeCookieHeaders(headers, 1), headers);
}

console.log('Cookie headers: ordered split/merge, repeated names, raw values, quotes/escapes, enabled groups, atomic rejection and immutable inputs passed.');
