const assert = require('node:assert/strict');
const fs = require('node:fs');
const ts = require('typescript');
const { buildSync } = require('esbuild');
let focused = '', dismissed = 0, received;
const menuButtons = [{ focus() { focused = 'first'; } }, { focus() { focused = 'last'; } }];
const menu = { querySelectorAll() { return menuButtons; } };
global.document = { body: { dataset: {} }, activeElement: menuButtons[0] };
global.window = { innerWidth: 1000, innerHeight: 700 };
const jsx = (type, props) => ({ type, props });
const result = buildSync({entryPoints:['apps/frontend/src/RequestTabMenu.tsx'],bundle:true,platform:'node',format:'cjs',write:false,jsx:'automatic',packages:'external',loader:{'.css':'empty'},external:['./i18n']});
const loaded = { exports: {} };
new Function('module','exports','require',result.outputFiles[0].text)(loaded,loaded.exports,name=>name==='./i18n'?{t:key=>key}:name==='react'?{
  useState:value=>[typeof value==='function'?value():value,()=>{}],useRef:()=>({current:menu}),useEffect(){},useLayoutEffect(){},
}:name==='react-dom'?{createPortal:value=>value}:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:require(name));
const {tabCloseIds,default:RequestTabMenu}=loaded.exports;
const ids=['A','B','C','D'];
assert.deepEqual(tabCloseIds(ids,'B','current'),['B']);
assert.deepEqual(tabCloseIds(ids,'B','others'),['A','C','D']);
assert.deepEqual(tabCloseIds(ids,'B','left'),['A']);
assert.deepEqual(tabCloseIds(ids,'B','right'),['C','D']);
assert.deepEqual(tabCloseIds(ids,'B','all'),ids);
assert.deepEqual(tabCloseIds(ids,null,'all'),ids);
assert.deepEqual(tabCloseIds(ids,'missing','others'),[]);
assert.deepEqual(tabCloseIds(ids,'A','left'),[]);
assert.deepEqual(tabCloseIds(ids,'D','right'),[]);
const target={id:'A',x:999,y:699,anchor:{isConnected:true,focus(){focused='anchor';}}};
const props={target,ids,onClose(){dismissed++;},onCloseTabs:value=>{received=value;}};
function nodes(tree) {return Array.isArray(tree)?tree.flatMap(nodes):tree&&typeof tree==='object'?[tree,...nodes(tree.props?.children)]:[];}
const tree=RequestTabMenu(props),buttons=nodes(tree).filter(n=>n.type==='button');
assert.equal(buttons.length,5);assert(buttons[2].props.disabled);assert(!buttons[3].props.disabled);
buttons[1].props.onClick();assert.deepEqual(received,['B','C','D']);assert.equal(dismissed,1);
tree.props.onKeyDown({key:'End',preventDefault(){}});assert.equal(focused,'last');
tree.props.onKeyDown({key:'Escape',preventDefault(){}});assert.equal(focused,'anchor');
const fixed=nodes(RequestTabMenu({...props,target:{...target,id:null}})).filter(n=>n.type==='button');
assert.equal(fixed.length,1);assert.equal(fixed[0].props.children,'关闭全部请求标签');
assert(nodes(RequestTabMenu({...props,ids:[]})).filter(n=>n.type==='button').every(n=>n.props.disabled));
// Execute App's actual batch-close handler to catch stale-state loops and leaked sessions.
const source=fs.readFileSync('apps/frontend/src/App.tsx','utf8');
const ast=ts.createSourceFile('App.tsx',source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let handler;
function visit(node){if(ts.isFunctionDeclaration(node)&&node.name?.text==='closeEditors')handler=node;ts.forEachChild(node,visit);}
visit(ast);assert(handler);
for(const closing of [['A','B','C'],ids,['missing']]) {
  const editors=ids.map(id=>({id,selected:id==='C'?'socket-C':null}));
  const state={editors,active:'B',view:'replay',drafts:{A:1,B:2,C:3,D:4},sessions:{B:'socket-B'},connecting:{B:true},modes:{A:'http',B:'websocket',D:'grpc'}};
  const cancelled=[],sockets=[],closedEditors={current:new Set()};
  const bindings={editors,activeEditor:'B',closedEditors,wsSessions:state.sessions,flows:[{id:'socket-C',websocket:{}}],runs:{cancel:async id=>{cancelled.push(id);}},invoke:async(command,args)=>{sockets.push(args.executionId);},notify:assert.fail,
    setEditors:value=>{state.editors=value;},setActiveEditor:value=>{state.active=value;},setView:value=>{state.view=value;},
    setWsDrafts:fn=>{state.drafts=fn(state.drafts);},setWsSessions:fn=>{state.sessions=fn(state.sessions);},setWsConnecting:fn=>{state.connecting=fn(state.connecting);},setRequestModes:fn=>{state.modes=fn(state.modes);}};
  const code=ts.transpileModule(handler.getText(ast),{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
  new Function(...Object.keys(bindings),code+';return closeEditors;')(...Object.values(bindings))(closing);
  const valid=ids.filter(id=>closing.includes(id));
  assert.deepEqual(cancelled,valid);assert.deepEqual([...closedEditors.current],valid);
  assert.deepEqual(state.editors.map(e=>e.id),ids.filter(id=>!closing.includes(id)));
  if(valid.length){assert.deepEqual(sockets,['socket-B','socket-C']);assert.equal(state.active,closing.length===4?'':'D');assert.equal(state.view,closing.length===4?'capture':'replay');assert.deepEqual(state.sessions,{});assert.deepEqual(state.connecting,{});assert(!('A' in state.drafts));}
  else {assert.equal(state.active,'B');assert.deepEqual(sockets,[]);}
}
console.log('Request tab menu: all close scopes, disabled boundaries, keyboard/focus, fixed capture tab, atomic batch removal, request/socket cancellation and state cleanup passed.');
