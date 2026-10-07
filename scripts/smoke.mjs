// A real local HTTP request through the running proxy, not seeded UI data.
import http from 'node:http';
import assert from 'node:assert/strict';
import { once } from 'node:events';

const proxyPort = Number(process.argv[2] || 8080);
const origin = http.createServer((req, res) => {
  const pieces = [];
  req.on('data', chunk => pieces.push(chunk));
  req.on('end', () => {
    const pairs = [];
    for (let i = 0; i < req.rawHeaders.length; i += 2) pairs.push([req.rawHeaders[i], req.rawHeaders[i + 1]]);
    res.writeHead(200, { 'Content-Type': 'application/json; charset=utf-8', 'X-Capture-Test': 'local' });
    res.end(JSON.stringify({ message: '真实请求已通过 HTTP Capture 代理', headers: pairs, body: Buffer.concat(pieces).toString('utf8') }, null, 2));
  });
});
origin.listen(0, '127.0.0.1');
await once(origin, 'listening');
try {
  const port = origin.address().port;
  const body = JSON.stringify({ test: 'capture', text: '你好，HTTP Capture' });
  const result = await new Promise((resolve, reject) => {
    const request = http.request({ host: '127.0.0.1', port: proxyPort, method: 'POST',
      path: `http://127.0.0.1:${port}/capture-check`,
      headers: ['Host', `127.0.0.1:${port}`, 'X-First', 'one', 'X-Middle', 'two', 'x-first', 'three',
        'Content-Type', 'application/json', 'Content-Length', String(Buffer.byteLength(body))],
    }, response => {
      const chunks = []; response.on('data', chunk => chunks.push(chunk));
      response.on('end', () => resolve({ status: response.statusCode, body: Buffer.concat(chunks).toString('utf8') }));
    });
    request.on('error', reject); request.setTimeout(10000, () => request.destroy(new Error('timeout')));
    request.end(body);
  });
  assert.equal(result.status, 200);
  const payload = JSON.parse(result.body);
  assert.deepEqual(payload.headers.filter(([name]) => name.toLowerCase().startsWith('x-')), [['X-First', 'one'], ['X-Middle', 'two'], ['x-first', 'three']]);
  assert.equal(payload.body, body);
  console.log('PASS: HTTP 200; ordered duplicate headers and UTF-8 body preserved. A real /capture-check request is now visible in the GUI.');
} finally { origin.close(); }
