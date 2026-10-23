const assert = require('node:assert/strict');
const fs = require('node:fs');
const { build } = require('esbuild');
let current;
const react = {
  useState(initial) { const h = current, i = h.index++; if (!(i in h.slots)) h.slots[i] = initial; return [h.slots[i], value => { h.slots[i] = typeof value === 'function' ? value(h.slots[i]) : value; h.changed = true; }]; },
  useRef(initial) { const h = current, i = h.index++; return h.slots[i] ??= { current: initial }; },
  useMemo(make, deps) { const h = current, i = h.index++, old = h.slots[i]; if (!old || deps.some((value, index) => value !== old.deps[index])) h.slots[i] = { value: make(), deps }; return h.slots[i].value; },
  useEffect(effect, deps) { const h = current, i = h.index++, old = h.slots[i]; if (!old || deps.some((value, index) => value !== old[index])) { h.effects.push(() => { h.cleanups[i]?.(); h.cleanups[i] = effect(); }); h.slots[i] = deps; } },
};
const jsx = (type, props = {}) => ({ type, props });
function harness(Component, props) {
  const h = { props, slots: [], cleanups: [], effects: [], index: 0, changed: false };
  h.render = () => { let tries = 0; do { assert(tries++ < 20); h.changed = false; h.index = 0; current = h; h.tree = Component(h.props); h.effects.splice(0).forEach(run => run()); } while (h.changed); };
  h.tick = async () => { await new Promise(resolve => setImmediate(resolve)); h.render(); };
  h.dispose = () => h.cleanups.forEach(cleanup => cleanup?.());
  h.render(); return h;
}
const flow = (version, length, state = 'receiving') => ({ id: 'live-sse', bodyRevision: version, state, bodyReferences: { response: { byteLength: length } } });
const page = (bytes, version) => ({ offset: 0, totalBytes: bytes.length, bodyBase64: bytes.toString('base64'), nextOffset: null, version });
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };

(async () => {
  const output = await build({ entryPoints: ['apps/frontend/src/RecordedSseLive.tsx'], bundle: true, platform: 'node', format: 'cjs', write: false, packages: 'external', jsx: 'automatic', plugins: [{ name: 'sse-ui', setup(builder) {
    builder.onLoad({ filter: /[/\\]RecordedSseLive\.tsx$/ }, args => ({ contents: fs.readFileSync(args.path, 'utf8').replace("new URL('./sseLive.worker.ts', import.meta.url)", "new URL('http://localhost/sse-worker')"), loader: 'tsx' }));
    builder.onLoad({ filter: /[/\\]api\.ts$/ }, () => ({ contents: 'export const invoke=(...args)=>globalThis.__sseInvoke(...args);' }));
    builder.onLoad({ filter: /[/\\]i18n\.ts$/ }, () => ({ contents: 'export const t=(key)=>key;' }));
  } }] });
  const mod = { exports: {} };
  new Function('module', 'exports', 'require', output.outputFiles[0].text)(mod, mod.exports, name => name === 'react' ? react : name === 'react/jsx-runtime' ? { jsx, jsxs: jsx } : require(name));
  class WorkerMock { constructor() { this.messages = []; this.terminated = false; WorkerMock.instances.push(this); } postMessage(value) { this.messages.push(value); } terminate() { this.terminated = true; } }
  WorkerMock.instances = [];
  global.Worker = WorkerMock;
  const calls = [];
  let bytes = Buffer.from([1, 2, 3]);
  let response = async () => page(bytes);
  global.__sseInvoke = (name, request) => { assert.equal(name, 'read_flow_body'); calls.push(request); return response(request); };
  const ui = harness(mod.exports.default, { flow: flow('v1', 3), encoding: 'gzip', onCopy() {} });
  await ui.tick();
  const worker = WorkerMock.instances[0];
  assert.equal(WorkerMock.instances.length, 1);
  assert.deepEqual([...worker.messages[0].chunk], [1, 2, 3]);
  bytes = Buffer.from([1, 2, 3, 4, 5]);
  ui.props = { ...ui.props, flow: flow('v2', 5) }; ui.render(); await ui.tick();
  assert.equal(WorkerMock.instances.length, 1, 'revision does not restart decompression');
  assert.deepEqual([...worker.messages[1].chunk], [4, 5]);
  assert.deepEqual(calls.map(call => [call.expectedVersion, call.offset, call.length]), [['v1', 0, 65536], ['v2', 0, 65536]]);
  const old = deferred();
  bytes = Buffer.from([1, 2, 3, 4, 5, 6]); response = () => old.promise;
  ui.props = { ...ui.props, flow: flow('v3', 6) }; ui.render();
  const fresh = Buffer.from([1, 2, 3, 4, 5, 6, 7]); response = async () => page(fresh);
  ui.props = { ...ui.props, flow: flow('v4', 7) }; ui.render(); await ui.tick();
  const count = worker.messages.length;
  old.resolve(page(bytes)); await ui.tick();
  assert.equal(worker.messages.length, count, 'stale version never appends bytes');
  assert.deepEqual([...worker.messages.at(-1).chunk], [6, 7]);
  bytes = Buffer.from([8, 9]); response = async () => page(bytes);
  ui.props = { ...ui.props, flow: flow('v5', 2) }; ui.render(); await ui.tick();
  assert(worker.messages.at(-2).reset, 'shrinking body resets the decoder');
  assert.deepEqual([...worker.messages.at(-1).chunk], [8, 9]);
  ui.props = { ...ui.props, flow: flow('v5', 2, 'completed') }; ui.render(); await ui.tick();
  assert(worker.messages.at(-1).finish, 'normal completion flushes the decompressor');
  const afterFinish = worker.messages.length;
  ui.props = { ...ui.props, flow: flow('v6', 2, 'completed') }; ui.render(); await ui.tick();
  assert.equal(worker.messages.length, afterFinish, 'later metadata revisions do not close twice');
  ui.dispose(); assert(worker.terminated);
  console.log('Recorded SSE UI: one worker across revisions, exact growth, stale rejection, terminal flush, shrink reset and disposal passed.');
})().catch(error => { console.error(error); process.exitCode = 1; });
