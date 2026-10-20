const assert = require('node:assert/strict');
const fs = require('node:fs');
const ts = require('typescript');
const postcss = require('postcss');
const {build, transformSync} = require('esbuild');

// Test the shipped JSX and callbacks, with native/network boundaries stubbed.
let current;
class Element {
  isConnected = true;
  focus() { document.activeElement = this; }
  select() { this.selected = true; }
  querySelectorAll(selector) { this.selector = selector; return this.focusables ?? []; }
}
global.HTMLElement = Element;
global.document = {body:new Element(),activeElement:new Element(),visibilityState:'visible'};
global.localStorage = {getItem:()=>null,setItem(){}};
const react = {
  useState(initial) { const h=current,i=h.index++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return [h.slots[i],v=>{h.slots[i]=typeof v==='function'?v(h.slots[i]):v;}]; },
  useRef(initial) {const h=current,i=h.index++;return h.slots[i]??={current:initial};},
  useId() {const h=current,i=h.index++;return h.slots[i]??=`test-${i}`;},
  useEffect(effect,deps) {const h=current,i=h.index++,old=h.slots[i];if(!old||deps.some((v,j)=>v!==old[j])){h.effects.push(effect);h.slots[i]=deps;}},
};
const jsx = (type,props={}) => {
  const node={type,props};
  if(typeof type==='string') {node.element=new Element();if(props.ref){props.ref.current??=node.element;node.element=props.ref.current;}}
  return node;
};
const t=(key,args={})=>key.replace(/\{(\w+)\}/g,(_,name)=>String(args[name]??`{${name}}`));
const icon=()=>null;
function nodes(root) {if(Array.isArray(root))return root.flatMap(nodes);if(!root||typeof root!=='object')return [];return [root,...nodes(root.props?.children)];}
function text(root) {if(Array.isArray(root))return root.map(text).join('');if(root==null||typeof root==='boolean')return '';return typeof root==='object'?text(root.props?.children):String(root);}
function harness(Component,props) {
  const h={slots:[],index:0,effects:[],props};
  h.render=()=>{current=h;h.index=0;h.tree=Component(h.props);h.effects.splice(0).forEach(effect=>effect());const dialog=nodes(h.tree).find(n=>n.props?.role==='dialog');if(dialog)dialog.element.focusables=nodes(dialog).filter(n=>n.type==='a'&&n.props.href||n.type==='button'&&!n.props.disabled||n.type==='input'&&!n.props.disabled).map(n=>n.element);return h.tree;};
  h.find=predicate=>nodes(h.tree).find(predicate);
  h.button=label=>h.find(n=>n.type==='button'&&text(n)===label);
  h.tick=async()=>{await new Promise(resolve=>setImmediate(resolve));h.render();};
  h.render();return h;
}
async function updater() {
  const output=await build({entryPoints:['apps/frontend/src/AppUpdater.tsx'],bundle:true,platform:'node',format:'cjs',write:false,jsx:'automatic',packages:'external',loader:{'.css':'empty'},define:{'import.meta.env.PROD':'false'},plugins:[{name:'boundaries',setup(build){
    build.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export const t=(...args)=>globalThis.__todoTranslate(...args);'}));
    build.onLoad({filter:/[/\\]api\.ts$/},()=>({contents:'export const invoke=(...args)=>globalThis.__todoInvoke(...args);'}));
    build.onLoad({filter:/[/\\]clipboard\.ts$/},()=>({contents:'export const writeClipboard=text=>globalThis.__todoClipboard(text);'}));
  }}]});
  const module={exports:{}};
  global.__todoTranslate=t;
  new Function('module','exports','require',output.outputFiles[0].text)(module,module.exports,name=>name==='react'?react:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:name==='react-dom'?{createPortal:n=>n}:name==='lucide-react'?new Proxy({},{get:()=>icon}):name==='@tauri-apps/api/core'?{isTauri:()=>false}:name==='@tauri-apps/api/event'?{listen:()=>assert.fail('native listener not expected')}:require(name));
  return module.exports.default;
}
// Isolate existing JSX without mounting the rest of App or the traffic table.
function shippedJsx(file,className,bindings) {
  const source=fs.readFileSync(file,'utf8');const ast=ts.createSourceFile(file,source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let selected;
  function visit(node){if(ts.isJsxElement(node)){const attrs=node.openingElement.attributes.properties;const attr=attrs.find(a=>ts.isJsxAttribute(a)&&a.name.getText(ast)==='className');if(attr?.initializer&&ts.isStringLiteral(attr.initializer)&&attr.initializer.text===className)selected=node;}ts.forEachChild(node,visit);}
  visit(ast);assert(selected,`missing ${className}`);
  const names=Object.keys(bindings),code=transformSync(`export default (${names.join(',')})=>(${selected.getText(ast)})`,{loader:'tsx',jsx:'automatic',format:'cjs'}).code;
  const module={exports:{}};new Function('module','exports','require',code)(module,module.exports,name=>name==='react/jsx-runtime'?{jsx,jsxs:jsx}:require(name));
  return module.exports.default(...Object.values(bindings));
}
function cssFor(selector,width) {
  const sheet=postcss.parse(fs.readFileSync('apps/frontend/src/ui-refresh.css','utf8'));const result={};
  sheet.walkRules(rule=>{if(rule.selector!==selector)return;for(let parent=rule.parent;parent;parent=parent.parent){if(parent.type==='atrule'&&parent.name==='media'){const max=parent.params.match(/max-width:\s*(\d+)px/);if(max&&width>Number(max[1]))return;}}rule.nodes.filter(n=>n.type==='decl').forEach(n=>result[n.prop]=n.value);});
  return result;
}
(async()=>{
  const AppUpdater=await updater(),url='https://github.com/godstealer/http-capture/releases/latest';
  let opened=0,prevented=0,checked=0;
  const base={desktop:true,production:false,status:async()=>({currentVersion:'0.1.28',configured:false,manualUpdate:true,reason:'Linux portable manual update'}),check:async()=>{checked++;return null;},download:async()=>assert.fail('no download'),install:async()=>assert.fail('no install'),listen:async()=>()=>{}};
  const manual=harness(AppUpdater,{driver:{...base,openDownload:async()=>{opened++;}}});
  manual.button('检查更新').props.onClick();await manual.tick();
  assert.match(text(manual.tree),/此安装方式需要手动更新/);assert.doesNotMatch(text(manual.tree),/此构建尚未配置更新服务/);assert.equal(checked,0);
  let link=manual.find(n=>n.type==='a');assert.equal(link.props.href,url);assert.equal(link.props.rel,'noopener noreferrer');assert.equal(text(link),'下载最新版');
  link.props.onClick({preventDefault(){prevented++;}});await manual.tick();assert.equal(opened,1);assert.equal(prevented,1);
  let dialog=manual.find(n=>n.props?.role==='dialog');const first=dialog.element.focusables[0];document.activeElement=first;dialog.props.onKeyDown({key:'Tab',shiftKey:true,preventDefault(){}});assert.equal(document.activeElement,manual.find(n=>n.type==='a').element);assert.match(dialog.element.selector,/a\[href\]/);
  dialog.props.onKeyDown({key:'Tab',shiftKey:false,preventDefault(){}});assert.equal(document.activeElement,first);
  dialog.props.onKeyDown({key:'Escape',preventDefault(){}});manual.render();assert(!manual.find(n=>n.props?.role==='dialog'));

  const failing=harness(AppUpdater,{driver:{...base,openDownload:async()=>{throw Error('no browser');}}});
  failing.button('检查更新').props.onClick();await failing.tick();failing.find(n=>n.type==='a').props.onClick({preventDefault(){}});await failing.tick();
  const fallback=failing.find(n=>n.type==='input'&&n.props['aria-label']==='最新版下载地址');assert.equal(fallback.props.value,url);assert(fallback.props.readOnly);fallback.props.onFocus({currentTarget:fallback.element});assert(fallback.element.selected);
  const copied=[];global.__todoClipboard=async value=>copied.push(value);failing.button('复制下载链接').props.onClick();await failing.tick();assert.deepEqual(copied,[url]);
  assert.match(text(failing.find(n=>n.props?.role==='alert')),/no browser/);
  dialog=failing.find(n=>n.props?.role==='dialog');document.activeElement=failing.find(n=>n.type==='input').element;dialog.props.onKeyDown({key:'Tab',shiftKey:false,preventDefault(){assert.fail('input remains in normal keyboard order');}});assert.match(dialog.element.selector,/input:not\(:disabled\)/);

  const browser=harness(AppUpdater,{driver:{...base}});browser.button('检查更新').props.onClick();await browser.tick();browser.find(n=>n.type==='a').props.onClick({preventDefault(){assert.fail('normal browser link keeps default navigation');}});
  const unconfigured=harness(AppUpdater,{driver:{...base,status:async()=>({currentVersion:'dev',configured:false,reason:'missing updater source'})}});unconfigured.button('检查更新').props.onClick();await unconfigured.tick();assert.match(text(unconfigured.tree),/此构建尚未配置更新服务/);assert(!unconfigured.find(n=>n.type==='a'));
  const busy=harness(AppUpdater,{driver:{...base,status:async()=>({currentVersion:'0.1.28',configured:true}),check:async()=>({currentVersion:'0.1.28',version:'0.1.29'}),download:()=>new Promise(()=>{})}});busy.button('检查更新').props.onClick();await busy.tick();busy.button('下载并安装').props.onClick();await busy.tick();busy.find(n=>n.props?.role==='dialog').props.onKeyDown({key:'Escape',preventDefault(){}});busy.render();assert(busy.find(n=>n.props?.role==='dialog'));assert(busy.find(n=>n.props?.['aria-label']==='关闭').props.disabled);

  const destinations=[];
  for(const upstream of [{enabled:false},{enabled:true,profileName:'Test proxy'}]){
    const button=shippedJsx('apps/frontend/src/App.tsx','upstream-chip',{t,Globe2:icon,status:{upstream},openSettings:id=>destinations.push(id)});
    assert.equal(button.props['aria-label'],'上游代理设置');assert.match(button.props.title,upstream.enabled?/Test proxy/:/直连/);button.props.onClick();
  }
  assert.deepEqual(destinations,['upstream','upstream']);
  for(const width of [360,700,1000,1250,1440]){const style=cssFor('.app-shell .upstream-chip',width);assert.notEqual(style.display,'none',`upstream remains visible at ${width}px`);if(width<=1250){assert.equal(style['min-width'],'34px');assert.equal(cssFor('.app-shell .upstream-chip span',width).display,'none');}}
  for(const width of [360,600,700]){const style=cssFor('.app-shell .inspector-heading',width);assert.equal(style.height,'auto');assert.equal(style['flex-wrap'],'wrap','inspector actions wrap instead of clipping new response replacement controls');}
  const flow={request:{method:'GET',url:'https://example.invalid/'},notes:[]},intercepted=[],replayed=[],rewritten=[];
  for(const [connected,websocket] of [[true,false],[false,false],[true,true]]){
    const item={...flow,...(websocket?{websocket:{}}:{})};
    const heading=shippedJsx('apps/frontend/src/CaptureView.tsx','inspector-heading',{t,flow:item,desktop:connected,Replace:icon,Send:icon,X:icon,onInterceptResponse:f=>intercepted.push(f),onRewriteResponse:(f,action)=>rewritten.push({flow:f,action}),onReplay:f=>replayed.push(f),direction:'row',setDirection(){},setSelected(){}});
    const button=nodes(heading).find(n=>n.type==='button'&&text(n)==='手动响应断点');assert.equal(button.props.disabled,!connected||websocket);
    for(const [label,action] of [['自动替换响应','replace'],['自动修改响应','modify']]) {
      const rewrite=nodes(heading).find(n=>n.type==='button'&&text(n)===label);
      assert.equal(rewrite.props.disabled,!connected||websocket);
      if(!rewrite.props.disabled){rewrite.props.onClick();assert.equal(rewritten.at(-1).flow,item);assert.equal(rewritten.at(-1).action,action);}
    }
    if(connected&&!websocket){assert.match(button.props.title,/后续匹配请求/);button.props.onClick();assert.equal(intercepted.at(-1),item);}else assert.match(button.props.title,connected?/WebSocket/:/连接内核/);
  }
  assert.equal(intercepted.length,1);assert.equal(replayed.length,0,'configuring interception never resends a historical request');
  console.log('TODO UI: manual update link/native and browser clicks/fallback/copy/keyboard/busy guard, responsive upstream action, and response interception entry passed.');
})().catch(error=>{console.error(error);process.exitCode=1;});
