const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');
const code = buildSync({
  entryPoints: ['apps/frontend/src/theme.ts'], bundle: true,
  platform: 'node', format: 'cjs', write: false,
}).outputFiles[0].text;
function loadTheme() {
  const module = { exports: {} };
  new Function('module', 'exports', code)(module, module.exports);
  return module.exports;
}
const storage = new Map();
global.localStorage = {
  getItem: key => storage.get(key) ?? null,
  setItem: (key, value) => storage.set(key, value),
};
global.document = { documentElement: { dataset: { unrelated: 'preserved' } } };
const themes = loadTheme();
assert.equal(themes.readTheme(), 'light', 'a fresh installation retains the existing light default');
const expectedSchemes = {
  light: 'light', dark: 'dark', cyberpunk: 'dark',
  arctic: 'light', dusk: 'dark', paper: 'light',
  sonar: 'dark', forge: 'dark', matrix: 'dark', sakura: 'light',
};

for (const [theme, scheme] of Object.entries(expectedSchemes)) {
  themes.applyTheme(theme);
  assert.equal(themes.readTheme(), theme);
  assert.equal(document.documentElement.dataset.theme, theme);
  assert.equal(document.documentElement.dataset.colorScheme, scheme);
  assert.equal(document.documentElement.dataset.unrelated, 'preserved');
  assert.equal(storage.get('http-capture.theme'), theme);
}

// Every saved theme must survive a fresh module and DOM, as on full refresh.
for (const [theme, scheme] of Object.entries(expectedSchemes)) {
  themes.applyTheme(theme);
  const startup = loadTheme();
  document.documentElement.dataset = {};
  startup.applyTheme(startup.readTheme());
  assert.deepEqual(document.documentElement.dataset, {theme, colorScheme:scheme});
}

const reloaded = loadTheme();
document.documentElement.dataset = {};
for (const previous of Object.keys(expectedSchemes)) {
  for (const [theme, scheme] of Object.entries(expectedSchemes)) {
    reloaded.applyTheme(previous);
    reloaded.applyTheme(theme);
    assert.deepEqual(document.documentElement.dataset, {theme, colorScheme:scheme},
      `${previous} -> ${theme} must replace the previous color scheme`);
    assert.equal(reloaded.readTheme(), theme);
  }
}
for (const invalid of ['', 'system', 'LIGHT', 'Dark', 'Arctic', 'cyber', 'null', '{}', '"dark"', '__proto__', 'constructor', 'toString', null, undefined, 1, {}]) {
  storage.set('http-capture.theme', invalid);
  assert.equal(loadTheme().readTheme(), 'light', invalid);
  themes.applyTheme(invalid);
  assert.deepEqual(document.documentElement.dataset, {theme:'light', colorScheme:'light'});
  assert.equal(themes.readTheme(), 'light');
}
storage.delete('http-capture.theme');
assert.equal(loadTheme().readTheme(), 'light');
themes.applyTheme('invalid runtime input');
assert.deepEqual(document.documentElement.dataset, { theme: 'light', colorScheme: 'light' });
assert.equal(themes.readTheme(), 'light');

const workingStorage = global.localStorage;
global.localStorage = {
  getItem() { throw new Error('storage read blocked'); },
  setItem() { throw new Error('quota exceeded'); },
};
assert.equal(themes.readTheme(), 'light');
for (const [theme, scheme] of Object.entries(expectedSchemes)) {
  assert.doesNotThrow(() => themes.applyTheme(theme));
  assert.deepEqual(document.documentElement.dataset, {theme, colorScheme:scheme},
    'storage failures cannot prevent applying a session theme');
}
Object.defineProperty(global, 'localStorage', {
  configurable: true,
  get() { throw new Error('storage is unavailable'); },
});
assert.equal(themes.readTheme(), 'light');
for (const [theme, scheme] of Object.entries(expectedSchemes)) {
  assert.doesNotThrow(() => themes.applyTheme(theme));
  assert.deepEqual(document.documentElement.dataset, {theme, colorScheme:scheme});
}
Object.defineProperty(global, 'localStorage', { configurable: true, writable: true, value: workingStorage });
themes.applyTheme('dark');
assert.equal(loadTheme().readTheme(), 'dark');
console.log('Theme preferences: ten themes, all 100 transitions, refresh restoration, color schemes, invalid values and unavailable storage passed');
