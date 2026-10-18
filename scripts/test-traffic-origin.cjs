const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const storage = new Map();
global.window = { addEventListener() {}, removeEventListener() {} };
global.document = { documentElement: {} };
global.localStorage = { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) };
Object.defineProperty(global, 'navigator', { configurable: true, value: { language: 'en' } });
const compiled = buildSync({
  stdin: { contents: `export * from './apps/frontend/src/trafficOrigin'; export * from './apps/frontend/src/captureColumns'; export * from './apps/frontend/src/i18n'; export {RequestOverview} from './apps/frontend/src/RequestDetails'; export {default as CaptureView} from './apps/frontend/src/CaptureView';`, resolveDir: process.cwd(), loader: 'tsx' },
  bundle: true, platform: 'node', format: 'cjs', jsx: 'automatic', write: false,
  loader: { '.css': 'empty' }, external: ['react', 'react-dom', 'react-dom/server'], logLevel: 'silent',
}).outputFiles[0].text;
function load(react = React) {
  const module = { exports: {} };
  new Function('require', 'module', 'exports', compiled)(name => name === 'react' ? react : require(name), module, module.exports);
  return module.exports;
}
const ui = load();
ui.setLanguage('en');
const request = { method: 'GET', url: 'https://example.invalid/origin', headers: [{ name: 'User-Agent', value: 'Chrome/123' }], bodyBase64: '', tls: { preset: 'native' } };
const origin = { entry: 'explicit-proxy', clientAddress: '127.0.0.1:59123', processStatus: 'identified', process: { pid: 4321, name: 'Chrome.EXE', path: 'C:\\Program Files\\Chrome\\Chrome.EXE' } };
function flow(trafficOrigin = origin, overrides = {}) {
  return { id: 'origin', source: 'capture', state: 'completed', parentId: null, startedAt: 0, durationMs: 3, request, response: null, error: null, notes: [], trafficOrigin, ...overrides };
}
const identified = ui.describeTrafficOrigin(flow());
assert.equal(identified.kind, 'identified');
assert.equal(identified.application, 'Chrome.EXE', 'process name casing must be preserved');
assert.equal(identified.pid, '4321');
assert.equal(identified.path, origin.process.path);
assert.equal(identified.clientAddress, '127.0.0.1:59123');
assert.equal(identified.entry, 'Explicit proxy');
assert.ok(ui.trafficOriginSearchText(flow()).includes('chrome.exe'));
assert.ok(ui.trafficOriginSearchText(flow()).includes('c:\\program files\\chrome'));
assert.ok(ui.trafficOriginTooltip(identified).includes('PID 4321'));
assert.equal(ui.trafficEntryLabel('tun'), 'TUN');
assert.equal(ui.trafficEntryLabel('quic-reverse'), 'QUIC reverse proxy');
assert.equal(ui.trafficEntryLabel('TUN'), 'Unknown entry (TUN)', 'unknown entry casing is not silently reinterpreted');
assert.equal(ui.trafficEntryLabel('future-entry'), 'Unknown entry (future-entry)');
assert.equal(ui.trafficEntryLabel({ toString: () => 'tun' }), 'Not recorded');

for (const value of [undefined, null, [], false, 'legacy']) {
  const missing = ui.describeTrafficOrigin(flow(value, { trafficOrigin: value }));
  assert.equal(missing.application, 'Not recorded');
  assert.equal(missing.pid, '—');
}
assert.equal(ui.describeTrafficOrigin({ source: 'capture' }).application, 'Not recorded');
assert.equal(ui.describeTrafficOrigin(flow(null)).application, 'Not recorded', 'User-Agent must never be used to guess an application');
for (const pid of [0, -1, 1.5, NaN, Infinity, '4321', 0x100000000, null]) {
  const invalid = ui.describeTrafficOrigin(flow({ ...origin, process: { ...origin.process, pid } }));
  assert.equal(invalid.kind, 'unidentified', String(pid));
  assert.equal(invalid.pid, '—');
  assert.equal(invalid.path, 'Not recorded');
  assert.ok(!invalid.application.includes('Chrome'));
}
assert.equal(ui.describeTrafficOrigin(flow({ ...origin, process: { pid: 42, name: '' } })).application, 'Process 42');
const malformedFields = ui.describeTrafficOrigin(flow({ ...origin, clientAddress: 123, unknownField: 'do not display', process: { pid: 42, name: {}, path: ['not a path'], futureField: 'do not display' } }));
assert.equal(malformedFields.application, 'Process 42');
assert.equal(malformedFields.path, 'Not recorded');
assert.equal(malformedFields.clientAddress, 'Not recorded');
assert.ok(!JSON.stringify(malformedFields).includes('do not display'));
for (const status of ['unknown', 'unsupported', 'transport-unsupported', 'tun-unavailable', 'future-status', null]) {
  const unknown = flow({ ...origin, entry: 'tun', processStatus: status });
  const description = ui.describeTrafficOrigin(unknown);
  assert.equal(description.kind, 'unidentified');
  assert.equal(description.application, 'Unidentified application');
  assert.equal(description.pid, '—');
  assert.ok(!ui.trafficOriginSearchText(unknown).includes('chrome'), `${status} must not expose an unverified helper process`);
}
const tun = ui.describeTrafficOrigin(flow({ ...origin, entry: 'tun', processStatus: 'tun-unavailable' }));
assert.equal(tun.entry, 'TUN');
assert.ok(tun.reason.includes('helper is not the source'));
const quic = ui.describeTrafficOrigin(flow({ ...origin, entry: 'quic-reverse', processStatus: 'transport-unsupported' }));
assert.equal(quic.entry, 'QUIC reverse proxy');
assert.equal(quic.reason, 'The current entry does not support identifying the source application.');
assert.ok(!quic.reason.includes('platform'));
const remote = ui.describeTrafficOrigin(flow({ ...origin, clientAddress: '[2001:db8::5]:44321', processStatus: 'remote' }));
assert.equal(remote.application, 'Remote client');
assert.equal(remote.pid, '—');
assert.equal(remote.clientAddress, '[2001:db8::5]:44321');

const replay = flow(origin, { source: 'replay', state: 'imported', parentId: 'captured-parent' });
const manual = ui.describeTrafficOrigin(replay);
assert.equal(manual.kind, 'manual');
assert.equal(manual.application, 'Sent manually by this app');
assert.equal(manual.entry, 'Manual request');
assert.equal(manual.imported, false);
assert.equal(manual.provenance, '');
for (const leaked of ['chrome', '4321', '59123', 'program files']) assert.ok(!ui.trafficOriginSearchText(replay).includes(leaked), `replay leaked ${leaked}`);
const imported = flow(origin, { state: 'imported' });
const importedDescription = ui.describeTrafficOrigin(imported);
assert.equal(importedDescription.application, 'Chrome.EXE');
assert.equal(importedDescription.reason, 'Process information recorded in the file');
assert.equal(importedDescription.imported, true);
assert.ok(importedDescription.provenance.includes('has not been verified on this device'));
assert.equal(ui.describeTrafficOrigin(flow(null, { state: 'imported' })).application, 'Imported record · Not recorded');

const malicious = flow({
  ...origin, entry: '<img src=x onerror=alert(1)>', clientAddress: '127.0.0.1\nspoof',
  process: { pid: 55, name: '<script>alert(1)</script>\u202e\u0000', path: 'javascript:alert(1)\r\n<svg onload=alert(1)>' },
});
const sanitized = ui.describeTrafficOrigin(malicious);
assert.ok(!/[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/.test(Object.values(sanitized).filter(value => typeof value === 'string').join('')));
assert.equal(sanitized.clientAddress, '127.0.0.1\ufffdspoof');
assert.ok(sanitized.application.endsWith('\ufffd\ufffd'));
const huge = ui.describeTrafficOrigin(flow({ ...origin, process: { pid: 1, name: 'x'.repeat(50000), path: 'y'.repeat(50000) } }));
assert.ok(huge.application.length < 300 && huge.path.length < 4200, 'imported metadata must not flood a cell or tooltip');
const before = JSON.stringify(malicious);
const details = renderToStaticMarkup(React.createElement(ui.RequestOverview, { request, flow: malicious }));
assert.ok(details.includes('Traffic origin') && details.includes('Client address') && details.includes('Process path'));
assert.ok(details.includes('&lt;script&gt;alert(1)&lt;/script&gt;'));
assert.ok(!details.includes('<script>') && !details.includes('<img ') && !details.includes('<svg onload'));
assert.ok(!details.includes('href="javascript:'), 'process paths are inert text');
assert.equal(JSON.stringify(malicious), before, 'presentation must not mutate captured data');
const importedMarkup = renderToStaticMarkup(React.createElement(ui.RequestOverview, { request, flow: imported }));
assert.ok(importedMarkup.includes('File record') && importedMarkup.includes('has not been verified on this device'));
const replayMarkup = renderToStaticMarkup(React.createElement(ui.RequestOverview, { request, flow: replay }));
assert.ok(replayMarkup.includes('Sent manually by this app') && !replayMarkup.includes('Chrome.EXE'));

const columnsKey = 'http-capture.capture.columns.v1';
assert.ok(ui.readColumns().includes('originApplication'), 'new installs show the application column');
assert.ok(!ui.readColumns().includes('clientAddress') && !ui.readColumns().includes('trafficEntry'));
ui.saveColumns(['method', 'url', 'status']);
assert.deepEqual(ui.readColumns(), ['method', 'url', 'status'], 'saved custom selections must not gain new default columns');
ui.saveColumns(['url', 'originApplication', 'clientAddress', 'trafficEntry']);
assert.deepEqual(ui.readColumns(), ['url', 'originApplication', 'clientAddress', 'trafficEntry']);
assert.equal(new Set(ui.captureColumns.map(column => column.id)).size, ui.captureColumns.length);
storage.delete(columnsKey);
const props = { flows: [flow()], desktop: true, running: true, address: '127.0.0.1:5555', onReplay() {}, onReplayNow() {}, onInterceptResponse() {}, onFlowsChanged() {}, onCopy() {}, busy: false, active: true };
const list = renderToStaticMarkup(React.createElement(ui.CaptureView, props));
assert.ok(list.includes('Source application') && list.includes('Chrome.EXE'));
ui.saveColumns(['url', 'originApplication', 'clientAddress', 'trafficEntry']);
const optionalList = renderToStaticMarkup(React.createElement(ui.CaptureView, { ...props, flows: [imported] }));
assert.ok(optionalList.includes('127.0.0.1:59123') && optionalList.includes('Client address') && optionalList.includes('Explicit proxy'));
assert.ok(optionalList.includes('File record'));

// Drive the actual capture search input with a tiny hook host. This verifies
// metadata participates in filtering without changing URL/protocol fields.
let cursor = 0;
const hooks = [];
const testReact = {
  ...React,
  useState(initial) {
    const index = cursor++;
    if (!(index in hooks)) hooks[index] = typeof initial === 'function' ? initial() : initial;
    return [hooks[index], value => { hooks[index] = typeof value === 'function' ? value(hooks[index]) : value; }];
  },
  useRef(initial) { return hooks[cursor++] ??= { current: initial }; },
  useMemo: compute => compute(), useEffect() {},
  useSyncExternalStore: (_subscribe, getSnapshot) => getSnapshot(),
};
const host = load(testReact);
host.setLanguage('en');
const flows = [
  flow(origin, { id: 'local', clientProtocol: 'HTTP/2' }),
  flow({ ...origin, entry: 'quic-reverse', processStatus: 'remote', clientAddress: '203.0.113.42:54001' }, { id: 'remote' }),
  flow(null, { id: 'legacy' }),
  flow({ ...origin, entry: 'tun', processStatus: 'tun-unavailable' }, { id: 'tun' }),
];
const untouched = JSON.stringify(flows);
function nodes(node, predicate) {
  if (Array.isArray(node)) return node.flatMap(value => nodes(value, predicate));
  if (!node || typeof node !== 'object') return [];
  return [...(predicate(node) ? [node] : []), ...nodes(node.props?.children, predicate)];
}
function renderSearch(query) {
  cursor = 0;
  let tree = host.CaptureView({ ...props, flows });
  const search = nodes(tree, node => node.type === 'input' && node.props['aria-label'] === 'Filter captured requests')[0];
  assert.ok(search);
  search.props.onChange({ target: { value: query } });
  cursor = 0;
  tree = host.CaptureView({ ...props, flows });
  return nodes(tree, node => node.type === 'tr' && node.props['aria-haspopup'] === 'menu').map(row => row.key);
}
assert.deepEqual(renderSearch('CHROME.EXE'), ['local']);
assert.deepEqual(renderSearch('203.0.113.42'), ['remote']);
assert.deepEqual(renderSearch('QUIC reverse'), ['remote']);
assert.deepEqual(renderSearch('helper is not the source'), ['tun']);
assert.deepEqual(renderSearch('not recorded for this request'), ['legacy']);
assert.equal(renderSearch('').length, 4);
assert.equal(JSON.stringify(flows), untouched, 'search must not alter URLs or protocol data');

ui.setLanguage('zh-CN');
assert.equal(ui.describeTrafficOrigin(flow()).entry, '显式代理');
assert.equal(ui.describeTrafficOrigin(replay).application, '本应用手动发送');
assert.ok(renderToStaticMarkup(React.createElement(ui.RequestOverview, { request, flow: imported })).includes('来源信息来自导入文件，未在本机验证。'));
console.log('Traffic origin: local/remote/TUN/replay/import provenance, safe metadata, i18n, column preferences, rendered details and live search passed');
