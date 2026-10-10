const fs = require('node:fs');
const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');
const ts = require('typescript');
const catalog = require('../apps/frontend/src/locales/en.json');
// Check all literal translation calls to prevent silent mixed-language regressions.
for (const file of fs.readdirSync('apps/frontend/src').filter(f => /\.tsx?$/.test(f))) {
  const root = ts.createSourceFile(file, fs.readFileSync('apps/frontend/src/' + file, 'utf8'), 99, true, file.endsWith('tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  function walk(node) {
    if (ts.isCallExpression(node) && node.expression.getText(root) === 't' && node.arguments[0] && ts.isStringLiteral(node.arguments[0])) assert.ok(Object.hasOwn(catalog, node.arguments[0].text), `${file}: missing ${node.arguments[0].text}`);
    ts.forEachChild(node, walk);
  }
  walk(root);
}
const events = new Map(), storage = new Map();
global.window = { addEventListener: (name, callback) => events.set(name, callback) };
global.document = { documentElement: {} };
global.localStorage = { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) };
Object.defineProperty(global, 'navigator', { configurable: true, value: { language: 'zh-CN' } });
const source = `export {default as RequestProxySelector} from './apps/frontend/src/RequestProxySelector'; export * from './apps/frontend/src/i18n'; export {default as CaptureView} from './apps/frontend/src/CaptureView'; export {default as LanguageSelector} from './apps/frontend/src/LanguageSelector'; export {RequestOverview} from './apps/frontend/src/RequestDetails';`;
const result = buildSync({ stdin: { contents: source, resolveDir: process.cwd(), loader: 'tsx' }, bundle: true, platform: 'node', format: 'cjs', jsx: 'automatic', loader: { '.css': 'empty' }, external: ['react', 'react-dom', 'react-dom/server'], write: false, logLevel: 'silent' });
function load() { const m = { exports: {} }; new Function('require','module','exports',result.outputFiles[0].text)(require, m, m.exports); return m.exports; }
const ui = load();
assert.equal(ui.t('请求头'), '请求头');
assert.equal(ui.resolveLanguage('system','zh-TW'),'zh-CN');
assert.equal(ui.resolveLanguage('system','fr-FR'),'en');
ui.setLanguage('en'); assert.equal(ui.t('请求头'), 'Request headers'); assert.equal(document.documentElement.lang,'en');
assert.equal(ui.t('已删除 {v0} 条记录',{v0:3}),'Deleted 3 records');
assert.equal(ui.t('constructor'),'constructor');
assert.equal(load().currentLanguage(),'en', 'preference survives reload');
ui.setLanguage('system'); navigator.language='en-US'; events.get('languagechange')();
// The reloaded module owns this listener; both use the persisted preference on reload.
const current = load(); current.setLanguage('system'); events.get('languagechange')(); assert.equal(current.currentLanguage(),'en');
navigator.language='zh-CN'; events.get('languagechange')(); assert.equal(current.currentLanguage(),'zh-CN');
current.setLanguage('en'); events.get('languagechange')(); assert.equal(current.currentLanguage(),'en');
const React = require('react'), {renderToStaticMarkup} = require('react-dom/server');
const props = { flows: [], desktop: true, running: false, address: '127.0.0.1:8080', onReplay(){}, onReplayNow(){}, busy:false, active:true, onCopy(){}, onFlowsChanged(){} };
ui.setLanguage('en'); const en = renderToStaticMarkup(React.createElement(ui.CaptureView, props)); assert.ok(en.includes('Import HAR / session')); assert.ok(en.includes('No captured requests')); assert.ok(en.includes('Headers') === false); assert.ok(!en.includes('暂无捕获的请求'));
ui.setLanguage('zh-CN'); const zh = renderToStaticMarkup(React.createElement(ui.CaptureView, props)); assert.ok(zh.includes('暂无捕获的请求'));
const request = {method:'GET', url:'https://example.com/请求头', headers:[{name:'X-Test',value:'响应'}], bodyBase64:'', tls:{preset:'native'}};
const before=JSON.stringify(request); ui.setLanguage('en'); const details=renderToStaticMarkup(React.createElement(ui.RequestOverview,{request})); assert.ok(details.includes('https://example.com/请求头')); assert.equal(JSON.stringify(request),before);
const selector=renderToStaticMarkup(React.createElement(ui.LanguageSelector));assert.ok(selector.includes('Follow system'));assert.ok(selector.includes('简体中文'));assert.ok(selector.includes('English'));
console.log('i18n: catalog coverage, system/explicit language, persistence, interpolation, rendered UI and untouched request data passed');

const httpcloakProxy=renderToStaticMarkup(React.createElement(ui.RequestProxySelector,{draft:{...request,engine:'httpcloak'},onChange(){},onManage(){}}));
assert.ok(!httpcloakProxy.includes('This engine does not support upstream proxies'), 'httpcloak proxy selection must be supported');
const wreqProxy=renderToStaticMarkup(React.createElement(ui.RequestProxySelector,{draft:{...request,engine:'wreq'},onChange(){},onManage(){}}));
assert.ok(!wreqProxy.includes('This engine does not support upstream proxies'), 'wreq proxy selection must be supported');
console.log('Request proxy UI: httpcloak and wreq supported passed');
const h3Proxy=renderToStaticMarkup(React.createElement(ui.RequestProxySelector,{draft:{...request,engine:'h3'},onChange(){},onManage(){}}));
assert.ok(h3Proxy.includes('UDP ASSOCIATE'), 'H3 must describe the SOCKS5 UDP requirement');
assert.ok(!h3Proxy.includes('This engine does not support upstream proxies'), 'H3 supports SOCKS5 proxies');


// All records remain available; only viewport rows render.
const many = Array.from({length: 12001}, (_, i) => ({id:String(i), source:'capture', request:{method:'GET',url:`https://example.invalid/${i}`,headers:[],bodyBase64:''},response:null,error:null,durationMs:0,notes:[]}));
const large = renderToStaticMarkup(React.createElement(ui.CaptureView,{...props, flows:many}));
assert.ok(large.includes('12001'));
assert.ok((large.match(/aria-haspopup="menu"/g) || []).length < 100);
assert.ok(large.includes('virtual-spacer'));
console.log('Capture list: 12,001 records retained with fewer than 100 rendered rows.');
