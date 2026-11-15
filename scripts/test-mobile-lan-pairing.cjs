const assert = require('node:assert/strict');
const { build } = require('esbuild');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const storage = new Map();
global.document = { documentElement: {} };
global.localStorage = { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) };
Object.defineProperty(global, 'navigator', { configurable: true, value: { language: 'en' } });

function nodes(node, predicate) {
  if (Array.isArray(node)) return node.flatMap(value => nodes(value, predicate));
  if (!node || typeof node !== 'object') return [];
  return [...(predicate(node) ? [node] : []), ...nodes(node.props?.children, predicate)];
}

// Mount the real component with controlled hooks and invoke, then execute its
// actual refresh effect and advance its poll through authenticated/old states.
(async () => {
  let index = 0, mounted = false;
  const values = [], effects = [], intervals = new Map(), calls = [];
  let gatewayStatus = { running: true, paired: true, address: '192.0.2.2:2443', fingerprint: null, deviceName: 'Pixel 9 Pro', sessionToken: 'MUST-NOT-RENDER-SESSION-TOKEN' };
  global.window = {
    addEventListener() {}, removeEventListener() {},
    setInterval(callback, period) { intervals.set(period, callback); return period; },
    clearInterval(period) { intervals.delete(period); },
  };
  const pairingReact = { ...React,
    useState(initial) {
      const slot = index++;
      if (!(slot in values)) values[slot] = typeof initial === 'function' ? initial() : initial;
      return [values[slot], next => { values[slot] = typeof next === 'function' ? next(values[slot]) : next; }];
    },
    useRef(initial) { return values[index++] ??= { current: initial }; },
    useEffect(effect) { if (!mounted) effects.push(effect); },
    useSyncExternalStore: (_subscribe, getSnapshot) => getSnapshot(),
  };
  global.__mobileLanInvoke = async (command, args) => {
    calls.push({ command, args });
    if (command === 'network_interfaces') return [{ name: 'Wi-Fi', address: '192.168.1.2', family: 'IPv4', available: true, loopback: false }];
    if (command === 'mobile_gateway_status') return { ...gatewayStatus };
    throw Error('Unexpected pairing command: ' + command);
  };
  const result = await build({
    stdin: { contents: "export {default as MobileLanPairing} from './apps/frontend/src/MobileLanPairing'; export * from './apps/frontend/src/i18n';", resolveDir: process.cwd(), loader: 'tsx' },
    bundle: true, platform: 'node', format: 'cjs', jsx: 'automatic', write: false,
    loader: { '.css': 'empty' }, external: ['react', 'react-dom', 'react-dom/server'], logLevel: 'silent',
    plugins: [{ name: 'pairing-invoke-fixture', setup(builder) {
      builder.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'tauri-core', namespace: 'fixture' }));
      builder.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: 'export const isTauri=()=>true; export const invoke=(name,args)=>global.__mobileLanInvoke(name,args);', loader: 'js' }));
    } }],
  });
  const moduleShim = { exports: {} };
  new Function('require', 'module', 'exports', result.outputFiles[0].text)(name => name === 'react' ? pairingReact : require(name), moduleShim, moduleShim.exports);
  const pairing = moduleShim.exports;
  pairing.setLanguage('en');
  function tree() { index = 0; return pairing.MobileLanPairing({ active: true }); }
  function markup() { return renderToStaticMarkup(tree()); }
  function stateLine() {
    const state = nodes(tree(), node => node.props?.className === 'mobile-lan-state')[0];
    assert.ok(state, 'render the actual pairing status line');
    return renderToStaticMarkup(state);
  }
  tree();
  mounted = true;
  const cleanups = effects.map(effect => effect()).filter(value => typeof value === 'function');
  await new Promise(resolve => setImmediate(resolve));
  try {
    assert.ok(stateLine().includes('Android paired') && stateLine().includes('Device name (self-reported)'));
    assert.ok(stateLine().includes('<bdi dir="auto">Pixel 9 Pro</bdi>'));
    assert.ok(stateLine().includes('does not identify an application or hardware'));
    assert.ok(!markup().includes('MUST-NOT-RENDER-SESSION-TOKEN'));
    assert.ok(calls.some(call => call.command === 'mobile_gateway_status'));
    assert.equal(intervals.size, 2);
    async function poll(next) {
      gatewayStatus = { ...gatewayStatus, ...next };
      intervals.get(2000)();
      await new Promise(resolve => setImmediate(resolve));
      return stateLine();
    }
    for (const next of [
      { paired: false }, { paired: true, running: false },
      { running: true, deviceName: undefined }, { deviceName: null },
      { deviceName: 123 }, { deviceName: { toString() { throw Error('must not coerce'); } } },
    ]) {
      const html = await poll(next);
      assert.ok(!html.includes('Pixel 9 Pro') && !html.includes('Device name (self-reported)'), 'stale and malformed names remain hidden');
    }
    const hostile = '<svg onload=alert(8)>\u061c\u200e\u200f\u2028\u2029\n';
    const html = await poll({ running: true, paired: true, deviceName: hostile });
    assert.ok(html.includes('&lt;svg onload=alert(8)&gt;') && !html.includes('<svg onload'));
    assert.ok(!/[\u061c\u200e\u200f\u2028\u2029\n]/.test(html));
    const bounded = await poll({ deviceName: 'Q'.repeat(50000) });
    assert.ok(bounded.length < 800 && bounded.includes('Q'.repeat(128) + '…'));
    const old = await poll({ deviceName: '' });
    assert.ok(old.includes('Android paired') && !old.includes('Device name (self-reported)'));
    pairing.setLanguage('zh-CN');
    assert.ok((await poll({ deviceName: '中文手机' })).includes('设备名称（自报）'));
  } finally {
    cleanups.forEach(cleanup => cleanup());
    assert.equal(intervals.size, 0, 'pairing poll and clock stop on cleanup');
    delete global.__mobileLanInvoke;
  }
  console.log('Mobile LAN pairing: real status polling, self-reported device label, revoked/legacy states, safe bounded metadata, i18n and no session-token rendering passed');
})().catch(error => { console.error(error); process.exitCode = 1; });
