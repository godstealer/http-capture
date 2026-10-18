const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');
const loaded = { exports: {} };
new Function('module', 'exports', buildSync({
  entryPoints: ['apps/frontend/src/listenerPreferences.ts'], bundle: true,
  platform: 'node', format: 'cjs', write: false,
}).outputFiles[0].text)(loaded, loaded.exports);
const { parseListenerPort, normalizeListenerHost, readListenerPreferences, saveListenerPreferences } = loaded.exports;
const storage = new Map();
global.localStorage = { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) };
const defaultPreferences = { host: '127.0.0.1', port: 8080 };

assert.deepEqual(readListenerPreferences(), defaultPreferences);
for (const [input, expected] of [['1024', 1024], ['65535', 65535], ['08080', 8080], ['', null], ['10', null], ['65536', null], ['1e4', null], [' 8080', null], ['8080.0', null], ['8080x', null]]) {
  assert.equal(parseListenerPort(input), expected, input);
}
for (const [input, expected] of [
  ['192.168.1.10', '192.168.1.10'], [' 0.0.0.0 ', '0.0.0.0'], ['[::1]', '::1'],
  ['2001:DB8:0:0::1', '2001:db8::1'], ['fe80::1%12', 'fe80::1%12'],
  ['', null], ['192.168.1.', null], ['192.168.1.999', null], ['127.1', null], ['127.00.0.1', null],
  ['localhost', null], ['https://127.0.0.1', null], ['224.0.0.1', null], ['ff02::1', null],
  ['fe80::1', null], ['fe80::1%0', null], ['fe80::1%eth0', null], ['fe80::1%4294967296', null], ['::g', null],
]) assert.equal(normalizeListenerHost(input), expected, input);

assert.equal(saveListenerPreferences({ host: '192.168.1.10', port: 9090 }), true);
assert.deepEqual(readListenerPreferences(), { host: '192.168.1.10', port: 9090 }, 'preferences survive a fresh read');
for (const invalid of [{ host: '', port: 9091 }, { host: '192.168.', port: 9091 }, { host: '127.0.0.1', port: 0 }, { host: '127.0.0.1', port: 65536 }, { host: '127.0.0.1', port: 8080.5 }]) {
  assert.equal(saveListenerPreferences(invalid), false);
  assert.deepEqual(readListenerPreferences(), { host: '192.168.1.10', port: 9090 });
}
const key = [...storage.keys()][0];
for (const value of ['not json', 'null', '[]', 'true', '"string"']) {
  storage.set(key, value); assert.deepEqual(readListenerPreferences(), defaultPreferences);
}
storage.set(key, JSON.stringify({ host: 'not an IP', port: 23456 }));
assert.deepEqual(readListenerPreferences(), { host: '127.0.0.1', port: 23456 });
storage.set(key, JSON.stringify({ host: '::1', port: '23456' }));
assert.deepEqual(readListenerPreferences(), { host: '::1', port: 8080 });
const workingStorage = global.localStorage;
Object.defineProperty(global, 'localStorage', { configurable: true, get() { throw Error('storage disabled'); } });
assert.deepEqual(readListenerPreferences(), defaultPreferences);
assert.equal(saveListenerPreferences({ host: '::1', port: 9090 }), false);
Object.defineProperty(global, 'localStorage', { configurable: true, writable: true, value: workingStorage });
global.localStorage = { getItem: () => { throw Error('blocked'); }, setItem: () => { throw Error('quota'); } };
assert.deepEqual(readListenerPreferences(), defaultPreferences);
assert.equal(saveListenerPreferences({ host: '::1', port: 9090 }), false);
global.localStorage = workingStorage;

// Exercise the actual settings event handlers and imperative Done callback with
// a small hook host. No browser, socket binding or desktop command is needed.
const component = buildSync({
  entryPoints: ['apps/frontend/src/ListenerSettings.tsx'], bundle: true,
  platform: 'node', format: 'cjs', jsx: 'automatic', write: false,
  external: ['react', 'react/jsx-runtime', './api', './i18n'],
}).outputFiles[0].text;
function mountSettings(initial = defaultPreferences) {
  let preferences = { ...initial }, props, tree, cursor, dirty = true, mounted = true;
  const hooks = [], checks = [], effects = [], ref = { current: null };
  const calls = [], commits = [];
  function changed(previous, next) { return !previous || !next || next.some((value, i) => !Object.is(value, previous[i])); }
  const react = {
    ...require('react'), forwardRef: render => render,
    useState(initialValue) {
      const index = cursor++;
      if (!hooks[index]) hooks[index] = { value: typeof initialValue === 'function' ? initialValue() : initialValue };
      const cell = hooks[index];
      return [cell.value, value => {
        assert.ok(mounted, 'an unmounted settings instance must not set state');
        const next = typeof value === 'function' ? value(cell.value) : value;
        if (!Object.is(next, cell.value)) { cell.value = next; dirty = true; }
      }];
    },
    useRef(initialValue) { return hooks[cursor++] ??= { current: initialValue }; },
    useEffect(effect, dependencies) {
      const index = cursor++;
      const cell = hooks[index] ??= {};
      if (changed(cell.dependencies, dependencies)) {
        cell.dependencies = dependencies;
        effects.push(() => { cell.cleanup?.(); cell.cleanup = effect(); });
      }
    },
    useImperativeHandle(target, create, dependencies) {
      react.useEffect(() => { target.current = create(); return () => { target.current = null; }; }, dependencies);
    },
  };
  const module = { exports: {} };
  new Function('require', 'module', 'exports', component)(name => {
    if (name === 'react') return react;
    if (name === './i18n') return { t: value => value };
    if (name === './api') return { invoke: (command, args) => {
      calls.push(command);
      if (command === 'network_interfaces') return Promise.resolve([]);
      assert.equal(command, 'check_listener_port', 'restoring preferences must not start a listener');
      let resolve, reject;
      const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
      checks.push({ ...args, resolve, reject }); return promise;
    } };
    return require(name);
  }, module, module.exports);
  function commit(field, value) {
    preferences[field] = value; commits.push([field, value]);
    props = { ...props, [field]: value }; dirty = true;
    saveListenerPreferences(preferences);
  }
  props = { ...initial, running: false, onHost: value => commit('host', value), onPort: value => commit('port', value) };
  function flush() {
    while (mounted && dirty) {
      dirty = false; cursor = 0; tree = module.exports.default(props, ref);
      while (effects.length) effects.shift()();
    }
  }
  function nodes(node, matches) {
    if (Array.isArray(node)) return node.flatMap(child => nodes(child, matches));
    if (!node || typeof node !== 'object') return [];
    return [...(matches(node) ? [node] : []), ...nodes(node.props?.children, matches)];
  }
  function input(label) { flush(); return nodes(tree, node => node.props?.['aria-label'] === label)[0].props; }
  flush();
  return {
    checks, calls, commits, ref, flush,
    get preferences() { return preferences; },
    input,
    edit(label, value) { input(label).onChange({ target: { value } }); flush(); },
    blur(label) { input(label).onBlur(); flush(); },
    update(update) { props = { ...props, ...update }; dirty = true; flush(); },
    unmount() { hooks.forEach(cell => cell?.cleanup?.()); mounted = false; },
  };
}

(async () => {
  storage.clear(); saveListenerPreferences({ host: '::1', port: 23456 });
  let ui = mountSettings(readListenerPreferences());
  assert.equal(ui.input('监听端口').value, '23456');
  assert.equal(ui.input('监听 IP').value, '::1');
  assert.deepEqual(ui.calls, ['network_interfaces']);
  for (const text of ['', '1023', '65536']) {
    ui.edit('监听端口', text);
    assert.equal(await ui.ref.current.commit(), false);
    assert.deepEqual(ui.preferences, { host: '::1', port: 23456 });
  }
  ui.edit('监听端口', '9090'); ui.edit('监听端口', '9090x');
  assert.equal(ui.input('监听端口').value, '9090', 'non-digits are rejected');
  ui.blur('监听端口');
  const done = ui.ref.current.commit();
  assert.equal(ui.checks.length, 1, 'Done shares an in-flight blur check');
  assert.deepEqual(ui.preferences, { host: '::1', port: 23456 });
  ui.checks[0].resolve(); assert.equal(await done, true); ui.flush();
  assert.deepEqual(readListenerPreferences(), { host: '::1', port: 9090 });
  ui.unmount();
  ui = mountSettings(readListenerPreferences());
  assert.equal(ui.input('监听端口').value, '9090', 'reopening restores the successful port');
  ui.edit('监听端口', '9091');
  const occupied = ui.ref.current.commit(); ui.checks[0].reject(Error('port occupied'));
  assert.equal(await occupied, false); ui.flush();
  assert.equal(ui.input('监听端口')['aria-invalid'], true);
  assert.equal(ui.preferences.port, 9090);
  const retry = ui.ref.current.commit(); ui.checks[1].resolve();
  assert.equal(await retry, true); ui.flush(); assert.equal(ui.preferences.port, 9091);
  ui.edit('监听端口', '9092'); const stale = ui.ref.current.commit();
  ui.edit('监听端口', '9093'); const latest = ui.ref.current.commit();
  ui.checks[2].resolve(); assert.equal(await stale, false);
  assert.equal(ui.preferences.port, 9091);
  ui.checks[3].resolve(); assert.equal(await latest, true); ui.flush();
  assert.equal(ui.preferences.port, 9093);
  ui.edit('监听 IP', '192.168.');
  assert.equal(await ui.ref.current.commit(), false);
  assert.deepEqual(readListenerPreferences(), { host: '::1', port: 9093 });
  ui.edit('监听 IP', '192.168.1.10');
  const address = ui.ref.current.commit();
  assert.equal(ui.checks[4].host, '192.168.1.10'); ui.checks[4].resolve();
  assert.equal(await address, true); ui.flush();
  assert.deepEqual(readListenerPreferences(), { host: '192.168.1.10', port: 9093 });
  ui.edit('监听端口', '9094'); const cancelled = ui.ref.current.commit();
  ui.unmount(); ui.checks[5].resolve(); assert.equal(await cancelled, false);
  assert.deepEqual(readListenerPreferences(), { host: '192.168.1.10', port: 9093 });
  ui = mountSettings(readListenerPreferences());
  ui.edit('监听端口', '9095'); const started = ui.ref.current.commit();
  ui.update({ running: true }); ui.checks[0].resolve();
  assert.equal(await started, false);
  assert.equal(ui.preferences.port, 9093);
  assert.equal(await ui.ref.current.commit(), true, 'closing running settings does not probe its occupied listener');
  ui.unmount();
  await Promise.resolve();
  console.log('Listener preferences: restart restoration, valid IP/port persistence, storage fallback, Done/blur checks, occupied ports, stale checks and cancellation passed.');
})().catch(error => { console.error(error); process.exitCode = 1; });
