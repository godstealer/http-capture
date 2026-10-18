const assert = require('node:assert/strict');
const vm = require('node:vm');
const { buildSync, transformSync } = require('esbuild');

const compiled = buildSync({
  entryPoints: ['apps/frontend/src/codegen/web.ts'],
  bundle: true,
  platform: 'node',
  format: 'cjs',
  write: false,
});
const moduleBox = { exports: {} };
new Function('module', 'exports', compiled.outputFiles[0].text)(moduleBox, moduleBox.exports);
const { webTargets } = moduleBox.exports;
assert.equal(webTargets.length, 7);
assert.equal(new Set(webTargets.map(target => target.id)).size, webTargets.length);

function sample(bytes, bodyText = null, method = 'POST') {
  return {
    method,
    url: 'https://example.invalid/path?q=%E4%B8%AD%E6%96%87&quote=%22&slash=%5C',
    headers: [
      { name: 'Content-Type', value: 'application/octet-stream' },
      { name: 'X-Example', value: 'quote" and backslash\\' },
      { name: 'x-example', value: 'second value' },
      { name: 'Cookie', value: 'first=1' },
      { name: 'cookie', value: 'second=2' },
    ],
    bodyBase64: Buffer.from(bytes).toString('base64'),
    bodyBytes: Uint8Array.from(bytes),
    bodyText,
  };
}

// No generated code receives require, import(), process, sockets, or the real
// fetch/XHR clients. Imports are syntax-checked before replacing their bindings
// with the isolated stubs below.
function sandboxFor() {
  const sent = [];
  const errors = [];
  class FakeAxiosHeaders {
    constructor(headers) { this.values = { ...headers }; }
    has(name) {
      return Object.keys(this.values).some(key => key.toLowerCase() === name.toLowerCase());
    }
    set(name, value) { this.values[name] = value; }
  }
  const transport = {
    request(url, options, onResponse) {
      const recorded = { kind: 'native', url: String(url), ...options };
      sent.push(recorded);
      return {
        on() { return this; },
        end(body) {
          recorded.body = body;
          onResponse({
            statusCode: 200,
            headers: {},
            on(event, callback) {
              if (event === 'data') callback(Buffer.from('mock response'));
              if (event === 'end') callback();
              return this;
            },
          });
          return this;
        },
      };
    },
  };
  const sandbox = {
    Buffer, Uint8Array, TextDecoder, URL, atob,
    console: { log() {}, error(...args) { errors.push(args); } },
    http: transport,
    https: transport,
    fetch: async (url, options) => {
      sent.push({ kind: 'fetch', url, ...options });
      return { status: 200, headers: [], text: async () => 'mock response' };
    },
    axios: {
      AxiosHeaders: FakeAxiosHeaders,
      request: async options => {
        sent.push({ kind: 'axios', ...options });
        return { status: 200, headers: {}, data: new ArrayBuffer(0) };
      },
    },
    $: {
      ajax(options) {
        sent.push({ kind: 'jquery', ...options });
        return {
          done(callback) {
            callback('mock response', 'success', { status: 200, getAllResponseHeaders: () => '' });
            return this;
          },
          fail() { return this; },
        };
      },
    },
    XMLHttpRequest: class {
      constructor() {
        this.status = 200;
        this.response = new ArrayBuffer(0);
        this.recorded = { kind: 'xhr', headers: [] };
      }
      open(method, url, asynchronous) { Object.assign(this.recorded, { method, url, asynchronous }); }
      setRequestHeader(name, value) { this.recorded.headers.push([name, value]); }
      getAllResponseHeaders() { return ''; }
      send(body) {
        sent.push({ ...this.recorded, body });
        if (this.onload) this.onload();
      }
    },
  };
  return { sandbox, sent, errors, FakeAxiosHeaders };
}

function withoutImports(code) {
  const imports = code.match(/^import .*;\r?$/gm) ?? [];
  for (const line of imports) {
    assert.match(line, /^import .+ from ['"](?:axios|jquery|node:buffer|node:http|node:https)['"];$/);
  }
  const executable = code.replace(/^import .*;\r?\n/gm, '');
  assert.doesNotMatch(executable, /\bimport\s*\(|\brequire\s*\(/);
  return executable;
}

function headersFrom(recorded, FakeAxiosHeaders) {
  if (recorded.headers instanceof FakeAxiosHeaders) return recorded.headers.values;
  if (Array.isArray(recorded.headers)) return Object.fromEntries(recorded.headers);
  return recorded.headers;
}

let syntaxChecks = 0;
let semanticChecks = 0;

async function verify(target, request) {
  const original = JSON.stringify(request);
  const code = target.generate(request);
  transformSync(code, { loader: 'js', format: 'esm', target: 'es2022' });
  syntaxChecks++;
  assert.equal(JSON.stringify(request), original, `${target.id}: generating must not mutate the input`);
  const isolated = sandboxFor();
  await vm.runInNewContext(withoutImports(code), isolated.sandbox, { timeout: 2000 });
  assert.deepEqual(isolated.errors, [], `${target.id}: the sample must finish without errors`);
  assert.equal(isolated.sent.length, 1, `${target.id}: exactly one mocked send`);
  const recorded = isolated.sent[0];
  assert.equal(recorded.url, request.url);
  assert.equal(recorded.method, request.method);
  const outgoing = recorded.body ?? recorded.data;
  const outgoingBytes = outgoing == null ? [] : Array.from(new Uint8Array(outgoing));
  assert.deepEqual(outgoingBytes, Array.from(request.bodyBytes), `${target.id}: original request bytes`);
  assert.equal(JSON.stringify(request), original, `${target.id}: running a sample must not mutate the input`);

  if (target.id === 'node-native') {
    assert.equal(target.preservesDuplicates, true);
    assert.deepEqual(Array.from(recorded.headers), [
      'Host', 'example.invalid',
      ...request.headers.flatMap(header => [header.name, header.value]),
      'Content-Length', String(request.bodyBytes.length),
    ], 'Native HTTP retains duplicate lines, case and values');
    assert(Buffer.isBuffer(recorded.body), 'Native HTTP writes Buffer bytes directly');
  } else {
    assert.equal(target.preservesDuplicates, false);
    const headers = headersFrom(recorded, isolated.FakeAxiosHeaders);
    assert.equal(headers['X-Example'], 'quote" and backslash\\, second value');
    assert.equal(headers.Cookie, 'first=1; second=2');
    assert.equal(headers['Content-Type'], 'application/octet-stream');
  }
  if (recorded.kind === 'axios') {
    assert.equal(recorded.adapter, target.browser ? 'xhr' : 'http');
    assert.equal(recorded.transformRequest.length, 1);
    assert.equal(recorded.transformRequest[0](recorded.data), recorded.data, 'Axios must not re-encode the body');
  }
  if (recorded.kind === 'jquery') {
    assert.equal(recorded.processData, false, 'jQuery must not serialize the binary body');
    assert.equal(recorded.contentType, false, 'jQuery must not replace the selected content type');
    assert.equal(recorded.dataType, 'text', 'Responses must not be evaluated as scripts');
  }
  semanticChecks++;
}

async function main() {
  const binary = sample([0, 255, 34, 92, 13, 10, 128]);
  const text = '\uFEFF中文 😀 e\u0301\r\n{"quote":"\\\\"}\u0000';
  const unicode = sample(Buffer.from(text, 'utf8'), text);
  const empty = sample([], '', 'GET');
  for (const target of webTargets) {
    await verify(target, binary);
    await verify(target, unicode);
    await verify(target, empty);

    // Missing Content-Type must not make Axios assign form serialization rules.
    if (target.library === 'Axios') {
      const noType = { ...binary, headers: binary.headers.filter(header => header.name !== 'Content-Type') };
      const isolated = sandboxFor();
      await vm.runInNewContext(withoutImports(target.generate(noType)), isolated.sandbox, { timeout: 2000 });
      assert.equal(isolated.sent[0].headers.values['Content-Type'], false);
    }

    // A copied browser snippet must not be terminated by values containing HTML
    // or JavaScript string delimiters. No generated code is executed here.
    const escaped = { ...binary, url: 'https://example.invalid/?q=</script>"\\\u2028\u2029' };
    const escapedCode = target.generate(escaped);
    transformSync(escapedCode, { loader: 'js', format: 'esm', target: 'es2022' });
    syntaxChecks++;
    assert(!escapedCode.includes('</script>'));
  }
  console.log(`Web code export: ${webTargets.length} targets, ${syntaxChecks} syntax checks and ${semanticChecks} isolated sends passed; binary/Unicode bytes, duplicate headers, no re-encoding and input isolation verified. No network requests executed.`);
}

main().catch(error => { console.error(error); process.exitCode = 1; });
