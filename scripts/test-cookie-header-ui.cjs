const assert=require('node:assert/strict');
const {build}=require('esbuild');

// Render the actual controlled component. Only React's hook storage, icons,
// translations and platform clipboard APIs are replaced by this small harness.
let current;
const react={
  useId(){const h=current,i=h.index++;return h.slots[i]??=`cookie-header-${i}`},
  useState(initial){const h=current,i=h.index++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return [h.slots[i],value=>{h.slots[i]=typeof value==='function'?value(h.slots[i]):value}]},
};
const jsx=(type,props)=>({type,props:props??{}});
function find(tree,predicate){const result=[];function walk(node){if(!node||typeof node!=='object')return;if(Array.isArray(node)){node.forEach(walk);return}if(predicate(node))result.push(node);walk(node.props?.children)}walk(tree);return result}
function text(node){if(node==null||node===false)return '';if(typeof node==='string'||typeof node==='number')return String(node);if(Array.isArray(node))return node.map(text).join('');return text(node.props?.children)}
const sideEffects={network:0,clipboard:0,invoke:0};
global.fetch=()=>{sideEffects.network++;throw Error('Cookie editing must never fetch')};
global.__cookieClipboard=()=>{sideEffects.clipboard++;throw Error('Cookie editing must never read or write the clipboard')};
global.__cookieInvoke=()=>{sideEffects.invoke++;throw Error('Cookie editing must never invoke a host API')};

async function load(){
  const output=await build({entryPoints:['apps/frontend/src/OrderedHeaders.tsx'],bundle:true,platform:'node',format:'cjs',jsx:'automatic',write:false,packages:'external',loader:{'.css':'empty'},plugins:[{name:'cookie-header-ui',setup(builder){
    builder.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export function t(key,values={}){return key.replace(/\\{([^}]+)\\}/g,(_,name)=>String(values[name]??"{"+name+"}"));}'}));
    builder.onLoad({filter:/[/\\]clipboard\.ts$/},()=>({contents:'export const readClipboard=(...args)=>globalThis.__cookieClipboard(...args);export const writeClipboard=readClipboard;'}));
    builder.onLoad({filter:/[/\\]api\.ts$/},()=>({contents:'export const invoke=(...args)=>globalThis.__cookieInvoke(...args);'}));
  }}]});
  const module={exports:{}};
  new Function('module','exports','require',output.outputFiles[0].text)(module,module.exports,name=>name==='react'?react:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:name==='lucide-react'?new Proxy({},{get:()=>()=>null}):require(name));
  return module.exports.default;
}
function harness(Component,headers,context){
  const h={index:0,slots:[],tree:null,changes:[]};
  const props={headers:structuredClone(headers),context,onChange(next){h.changes.push(next);props.headers=next}};
  h.render=()=>{current=h;h.index=0;h.tree=Component(props);return h.tree};
  h.headers=()=>props.headers;
  h.find=predicate=>find(h.tree,predicate);
  h.input=label=>h.find(node=>node.type==='input'&&node.props['aria-label']===label)[0];
  h.button=(action,row)=>h.find(node=>node.type==='button'&&node.props['aria-label']===`${action} Cookie ${row}`)[0];
  h.action=(action,row)=>{const button=h.button(action,row);assert(button,`${action} action must exist for row ${row}`);button.props.onClick();h.render()};
  h.value=(row,value)=>{h.input(`字段值 ${row}`).props.onChange({target:{value}});h.render()};
  h.name=(row,value)=>{h.input(`字段名 ${row}`).props.onChange({target:{value}});h.render()};
  h.render();return h;
}
const unavailable=button=>button.props['aria-disabled']===true||button.props['aria-disabled']==='true';
function actionsFor(h,row,split,merge){
  for(const [action,available] of [['拆分',split],['合并',merge]]){
    const button=h.button(action,row);assert(button,`Cookie row ${row} always exposes ${action}`);
    assert.equal(unavailable(button),!available,`${action} availability for row ${row}`);
    assert.notEqual(button.props.disabled,true,'semantic-disabled actions stay keyboard focusable for their explanation');
    assert.notEqual(button.props.tabIndex,-1);
    assert(String(button.props.title).includes(`${action} Cookie`));
    assert(String(button.props.title).includes(String(row)),'the explanation identifies the row');
  }
}
function noChange(h,action,row){const before=h.headers(),count=h.changes.length;h.action(action,row);assert.equal(h.changes.length,count,'unavailable action must not emit onChange');assert.equal(h.headers(),before)}

(async()=>{
  const Component=await load();
  const original=[{name:'X-Before',value:'before'},{name:'Cookie',value:'sid=one; theme=dark; sid=two'},{name:'X-After',value:'after'}];
  const h=harness(Component,original);
  const inputHeaders=h.headers(),inputSnapshot=structuredClone(inputHeaders);
  actionsFor(h,2,true,false);
  const row=h.find(node=>node.type==='div'&&String(node.props.className).split(' ').includes('header-row'))[1];
  assert(find(row,node=>node!==row&&node.props?.children&&find(node,item=>item.type==='input'&&item.props['aria-label']==='字段名 2').length===1&&find(node,item=>item.type==='button'&&/^(拆分|合并) Cookie 2$/.test(item.props['aria-label']??'')).length===2).length,'Cookie controls are grouped beside the field-name editor');
  noChange(h,'合并',2);
  h.action('拆分',2);assert.equal(h.changes.length,1);
  assert.deepEqual(h.headers(),[{name:'X-Before',value:'before'},{name:'Cookie',value:'sid=one'},{name:'Cookie',value:'theme=dark'},{name:'Cookie',value:'sid=two'},{name:'X-After',value:'after'}]);
  actionsFor(h,2,false,true);actionsFor(h,3,false,true);actionsFor(h,4,false,true);
  h.action('合并',2);assert.equal(h.changes.length,2);assert.deepEqual(h.headers(),original,'split and merge preserve duplicate cookie names and pair order');
  assert.deepEqual(inputHeaders,inputSnapshot,'the controlled input array and its header objects remain unchanged');
  assert(h.find(node=>node.props.role==='status').length,'a local transformation reports success');

  const interleaved=harness(Component,[{name:'X-First',value:'first'},{name:'cOoKiE',value:'sid=one'},{name:'X-Middle',value:'middle'},{name:'COOKIE',value:'sid=two'},{name:'Set-Cookie',value:'server=new; Path=/'},{name:'cookie',value:'consent=yes'},{name:'X-Last',value:'last'}]);
  actionsFor(interleaved,2,false,true);
  assert(String(interleaved.button('合并',2).props.title).includes('启用状态'),'merge explains that enabled/disabled groups are separate');
  interleaved.action('合并',2);assert.equal(interleaved.changes.length,1);
  assert.deepEqual(interleaved.headers(),[{name:'X-First',value:'first'},{name:'cOoKiE',value:'sid=one; sid=two; consent=yes'},{name:'X-Middle',value:'middle'},{name:'Set-Cookie',value:'server=new; Path=/'},{name:'X-Last',value:'last'}],'merge retains the first Cookie position/casing and the order of all other headers');

  const grouped=harness(Component,[{name:'Cookie',value:'a=1; e=2'},{name:'Cookie',value:'d=1; d=2',disabled:true},{name:'X-Stable',value:'keep'},{name:'cookie',value:'e=3'},{name:'COOKIE',value:'d=3',disabled:true}]);
  actionsFor(grouped,1,true,true);actionsFor(grouped,2,true,true);
  grouped.action('拆分',2);assert.equal(grouped.changes.length,1);
  assert.deepEqual(grouped.headers().slice(1,3),[{name:'Cookie',value:'d=1',disabled:true},{name:'Cookie',value:'d=2',disabled:true}],'splitting a disabled row keeps every pair disabled');
  grouped.action('合并',2);assert.equal(grouped.changes.length,2);
  assert.deepEqual(grouped.headers(),[{name:'Cookie',value:'a=1; e=2'},{name:'Cookie',value:'d=1; d=2; d=3',disabled:true},{name:'X-Stable',value:'keep'},{name:'cookie',value:'e=3'}],'merging disabled rows never consumes an enabled row');
  grouped.action('合并',1);assert.equal(grouped.changes.length,3);
  assert.deepEqual(grouped.headers(),[{name:'Cookie',value:'a=1; e=2; e=3'},{name:'Cookie',value:'d=1; d=2; d=3',disabled:true},{name:'X-Stable',value:'keep'}]);
  actionsFor(grouped,1,true,false);actionsFor(grouped,2,true,false);
  noChange(grouped,'合并',1);noChange(grouped,'合并',2);
  grouped.input('启用字段 2').props.onChange({target:{checked:true}});grouped.render();
  assert.equal(grouped.changes.length,4);actionsFor(grouped,1,true,true);actionsFor(grouped,2,true,true);
  assert.equal(grouped.headers()[1].disabled,undefined,'the existing enable control retains its behavior');

  const editing=harness(Component,[{name:'X-Test',value:'single=one'},{name:'Set-Cookie',value:'id=server; Path=/'},{name:'set-cookie',value:'another=server'},{name:'X-Cookie',value:'not=a; cookie=row'}]);
  assert.equal(editing.find(node=>node.type==='button'&&/^(拆分|合并) Cookie /.test(node.props['aria-label']??'')).length,0,'other headers and every Set-Cookie casing have no Cookie controls');
  editing.name(1,'COOKIE');actionsFor(editing,1,false,false);noChange(editing,'拆分',1);noChange(editing,'合并',1);
  editing.value(1,'single=one; extra=two');actionsFor(editing,1,true,false);
  editing.value(1,'');actionsFor(editing,1,false,false);noChange(editing,'拆分',1);
  editing.value(1,'again=one');editing.name(4,'Cookie');actionsFor(editing,1,false,true);
  editing.name(4,'Set-Cookie');actionsFor(editing,1,false,false);
  editing.name(1,'Set-Cookie');assert.equal(editing.find(node=>node.type==='button'&&/^(拆分|合并) Cookie /.test(node.props['aria-label']??'')).length,0);
  assert.equal(editing.changes.length,7,'each ordinary input change emits exactly once, and unavailable actions emit nothing');
  assert.deepEqual(sideEffects,{network:0,clipboard:0,invoke:0},'Cookie split/merge is a local edit, with no request, clipboard access or host invocation');
  const button=(ui,label)=>ui.find(node=>node.type==='button'&&text(node)===label)[0];
  assert(button(h,'复制请求头'));assert(button(h,'添加请求头'),'request context remains the default');
  assert(!button(h,'复制响应头'));
  const response=harness(Component,[{name:'Set-Cookie',value:'a=1'},{name:'set-cookie',value:'b=2'},{name:'X-Disabled',value:'keep',disabled:true}],'response');
  assert(button(response,'复制响应头'));assert(button(response,'添加响应头'));assert(!button(response,'复制请求头'));
  const copied=[];global.__cookieClipboard=async value=>{copied.push(value)};
  button(response,'复制响应头').props.onClick();await new Promise(resolve=>setImmediate(resolve));response.render();
  assert.deepEqual(copied,['Set-Cookie: a=1\r\nset-cookie: b=2'],'response context retains duplicate/case/disabled copy semantics');
  assert(text(response.tree).includes('已复制启用的响应头'));
  button(response,'文本编辑').props.onClick();response.render();
  assert(response.find(node=>node.type==='textarea'&&node.props['aria-label']==='响应头文本').length);
  assert(response.find(node=>node.type==='select'&&node.props['aria-label']==='响应头导入方式').length);
  response.find(node=>node.type==='textarea')[0].props.onChange({target:{value:':status: 201\nX-Response: updated'}});response.render();
  button(response,'应用到列表').props.onClick();response.render();
  assert.deepEqual(response.headers(),[{name:'X-Response',value:'updated'},{name:'X-Disabled',value:'keep',disabled:true}]);
  assert(text(response.tree).includes('已应用 1 个响应头'));assert(text(response.tree).includes('状态码保持当前设置'));
  button(response,'添加响应头').props.onClick();response.render();assert.deepEqual(response.headers().at(-1),{name:'',value:''});
  button(response,'文本编辑').props.onClick();response.render();response.find(node=>node.type==='textarea')[0].props.onChange({target:{value:'Bad header: value'}});response.render();button(response,'应用到列表').props.onClick();response.render();assert(text(response.tree).includes('第 1 行响应头格式无效'));
  console.log('Cookie header UI: real component, visible accessible row actions, split/merge order and duplicates, disabled groups, Set-Cookie exclusion, live input availability, exact single updates and no external side effects passed.');
  console.log('Header context UI: default request labels, response copy/add/import/errors, exact enabled duplicate headers and unchanged field behavior passed.');
})().catch(error=>{console.error(error);process.exitCode=1});
