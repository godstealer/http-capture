// Requires npm run dev and npm run capture. Uses only a loopback fixture.
import http from 'node:http';
import assert from 'node:assert/strict';
const sockets = new Set();
const server = http.createServer((_req, res) => {
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
  res.write('id: 1\nevent: smoke\ndata: live-before-close\n\n');
});
server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); });
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const call = async (path, args) => {
  const response = await fetch(`http://127.0.0.1:1420/__capture/${path}`, {
    method: args === undefined ? 'GET' : 'POST', headers: { 'x-capture-ui': '1', 'content-type': 'application/json' },
    body: args === undefined ? undefined : JSON.stringify(args), signal: AbortSignal.timeout(10000),
  });
  assert.equal(response.status, 200, await response.clone().text()); return response.json();
};
let id;
try {
  id = await call('replay/prepare', {});
  let completed = false;
  const pending = call('replay', { executionId: id, request: {
    url: `http://127.0.0.1:${server.address().port}/events`, method: 'GET', engine: 'auto',
    headers: [], pseudoHeaders: [], bodyBase64: '', tls: { preset: 'native' }, scripts: {},
  }}).then(flow => { completed = true; return flow; });
  // Attach rejection immediately; a fixture failure must not create an unhandled rejection.
  pending.catch(() => {});
  let live;
  for (let i = 0; i < 30; i++) {
    await new Promise(resolve => setTimeout(resolve, 100));
    live = (await call('flows')).find(flow => flow.id === id);
    if (live?.response?.bodyBase64) break;
  }
  assert(!completed, 'response must still be streaming');
  assert.match(Buffer.from(live.response.bodyBase64, 'base64').toString(), /live-before-close/);
  assert.equal(await call('replay/cancel', { executionId: id }), true);
  const final = await pending;
  assert(final.error);
  assert.equal(final.response.bodyBase64, live.response.bodyBase64);
  assert(!final.notes.includes('SSE 接收中'));
  console.log('Live service SSE: event visible before EOF; cancellation retains body.');
} finally {
  if (id) await call('replay/cancel', { executionId: id }).catch(() => {});
  for (const socket of sockets) socket.destroy();
  await new Promise(resolve => server.close(resolve));
}
