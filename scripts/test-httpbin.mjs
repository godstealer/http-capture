// Explicit external integration test. Sends only synthetic data to httpbin.org.
import fs from 'node:fs';
import { gunzipSync, inflateSync } from 'node:zlib';
const token = fs.readFileSync('.local/capture/control.token', 'utf8').trim();
const cases = [
  ['legacy', 'GET', '/legacy', 200],
  ['get', 'GET', '/get?capture_test=hello', 200],
  ['post', 'POST', '/post', 200],
  ['html', 'GET', '/html', 200],
  ['gzip', 'GET', '/gzip', 200],
  ['deflate', 'GET', '/deflate', 200],
  ['status', 'GET', '/status/418', 418],
  ['redirect', 'GET', '/redirect/1', 302],
];
const results = [];
for (const [name, method, path, expected] of cases) {
  const payload = { message: '你好，HTTP Capture', test: true };
  const request = { engine: 'native', method, url: `https://httpbin.org${path}`,
    headers: [{ name: 'X-Capture-Test', value: 'httpbin-integration' }, ...(method === 'POST' ? [{ name: 'Content-Type', value: 'application/json' }] : [])],
    bodyBase64: method === 'POST' ? Buffer.from(JSON.stringify(payload)).toString('base64') : '', tls: { preset: 'native' } };
  try {
    const response = await fetch('http://127.0.0.1:1421/replay', { method: 'POST', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ request, parentId: null }), signal: AbortSignal.timeout(35000) });
    if (!response.ok) throw new Error(`Control API ${response.status}`);
    const flow = await response.json();
    const head = field => flow.response?.headers.find(h => h.name.toLowerCase() === field)?.value ?? '';
    let bytes = Buffer.from(flow.response?.bodyBase64 ?? '', 'base64');
    const encoding = head('content-encoding');
    if (encoding === 'gzip') bytes = gunzipSync(bytes);
    if (encoding === 'deflate') bytes = inflateSync(bytes);
    const text = bytes.toString('utf8');
    let valid = flow.response?.status === expected && !flow.error;
    if (valid && ['get', 'post', 'gzip', 'deflate'].includes(name)) {
      const json = JSON.parse(text);
      if (name === 'get') valid &&= json.args.capture_test === 'hello';
      if (name === 'post') valid &&= json.json.message === payload.message && json.json.test === true;
      if (name === 'gzip') valid &&= json.gzipped === true;
      if (name === 'deflate') valid &&= json.deflated === true;
    }
    if (valid && ['html', 'legacy'].includes(name)) valid &&= /<html|<!doctype/i.test(text);
    if (valid && name === 'redirect') valid &&= !!head('location');
    const result = { name, status: flow.response?.status, encoding, contentType: head('content-type'), valid, id: flow.id, error: flow.error };
    results.push(result); console.log(JSON.stringify(result));
  } catch (error) { results.push({ name, valid: false, error: String(error) }); console.log(name, String(error)); }
}
fs.writeFileSync('.local/httpbin-results.json', JSON.stringify(results, null, 2));
if (results.some(result => !result.valid)) process.exitCode = 1;
