const assert=require('node:assert/strict');
const {build}=require('esbuild');
let current;
class Element {
  scrollTop=0;clientHeight=600;isConnected=true;dataset={};
  focus(){document.activeElement=this} contains(){return false}
  getBoundingClientRect(){return{left:0,top:0,bottom:120,width:320,height:240}}
  querySelector(){return null} querySelectorAll(){return[]}
}
global.HTMLElement=Element;
global.document={body:new Element(),activeElement:new Element(),addEventListener(){},removeEventListener(){},documentElement:{}};
global.window={innerWidth:1280,innerHeight:800,addEventListener(){},removeEventListener(){}};
global.localStorage={getItem(){return null},setItem(){}};
global.getComputedStyle=()=>({zoom:'1',getPropertyValue(){return '1'}});
global.ResizeObserver=class{observe(){} disconnect(){}};
const react={
  useState(initial){const h=current,i=h.index++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return[h.slots[i],value=>{const next=typeof value==='function'?value(h.slots[i]):value;if(!Object.is(next,h.slots[i])){h.slots[i]=next;h.changed=true}}]},
  useRef(initial){const h=current,i=h.index++;return h.slots[i]??={current:initial}},
  useMemo(fn,deps){const h=current,i=h.index++,old=h.slots[i];if(!old||deps.some((v,j)=>v!==old.deps[j]))h.slots[i]={value:fn(),deps};return h.slots[i].value},
  useEffect(fn,deps){const h=current,i=h.index++,old=h.slots[i];if(!old||deps.some((v,j)=>v!==old[j])){h.effects.push(()=>{h.cleanups[i]?.();h.cleanups[i]=fn()});h.slots[i]=deps}},
};
react.useLayoutEffect=react.useEffect;
const jsx=(type,props={})=>{if(typeof type==='string'&&props.ref&&!props.ref.current)props.ref.current=new Element();return{type,props}};
const nodes=node=>Array.isArray(node)?node.flatMap(nodes):node&&typeof node==='object'?[node,...nodes(node.props?.children)]:[];
const text=node=>Array.isArray(node)?node.map(text).join(''):node==null||node===false?'':typeof node==='object'?text(node.props?.children):String(node);
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
async function load(file){
  const output=await build({entryPoints:[`apps/frontend/src/${file}`],bundle:true,platform:'node',format:'cjs',write:false,jsx:'automatic',packages:'external',loader:{'.css':'empty'},plugins:[{name:'recorded-actions',setup(builder){
    builder.onLoad({filter:/[/\\]api\.ts$/},()=>({contents:'export const invoke=(...args)=>globalThis.__historyInvoke(...args);'}));
    builder.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export const t=(key,args={})=>key.replace(/\\{([^}]+)\\}/g,(_,name)=>String(args[name]??""));export const useLanguage=()=>({language:"zh-CN"});export const currentLanguage=()=>"zh-CN";'}));
    builder.onLoad({filter:/[/\\](ResponseBody|RecordedBody|GrpcPanel|SplitPane|FileProgress|HeaderOrder|WebSocketFrames)\.tsx$/},args=>{const name=args.path.split(/[/\\]/).at(-1).replace('.tsx','');return{contents:`export function ${name}(){};export default ${name};`}});
    builder.onLoad({filter:/[/\\]importSession\.ts$/},()=>({contents:'export const importSession=(...args)=>globalThis.__historyImport(...args);'}));
    builder.onLoad({filter:/[/\\]saveFile\.ts$/},()=>({contents:'export const chooseSave=(...args)=>globalThis.__historyChoose(...args);export const saveText=(...args)=>globalThis.__historySave(...args);'}));
    builder.onLoad({filter:/[/\\]har\.ts$/},()=>({contents:'export const serializeHar=(...args)=>globalThis.__historyHar(...args);'}));
  }}]});
  const m={exports:{}};new Function('module','exports','require',output.outputFiles[0].text)(m,m.exports,name=>name==='react'?react:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:name==='react-dom'?{createPortal:n=>n}:name==='lucide-react'?new Proxy({},{get:()=>()=>null}):require(name));return m.exports;
}
function harness(Component,props){
  const h={slots:[],index:0,effects:[],cleanups:[],props,changed:false};
  h.render=()=>{let limit=0;do{assert(limit++<30,'effects converge');h.changed=false;current=h;h.index=0;h.tree=Component(h.props);h.effects.splice(0).forEach(fn=>fn())}while(h.changed);return h.tree};
  h.all=predicate=>nodes(h.tree).filter(predicate);h.find=predicate=>h.all(predicate)[0];h.button=label=>h.find(n=>n.type==='button'&&text(n)===label);
  h.tick=async()=>{await new Promise(resolve=>setImmediate(resolve));h.render()};h.dispose=()=>h.cleanups.forEach(fn=>fn?.());h.render();return h;
}
const encode=value=>Buffer.from(value).toString('base64');
const full={id:'recorded',parentId:null,startedAt:1,durationMs:2,source:'capture',state:'completed',notes:[],error:null,
  request:{method:'POST',url:'https://example.invalid/api',headers:[{name:'X-Repeat',value:'one'},{name:'x-repeat',value:'two'}],tls:{preset:'native'},bodyBase64:encode('complete request')},
  originalRequest:{method:'POST',url:'https://example.invalid/api',headers:[],tls:{preset:'native'},bodyBase64:''},
  response:{status:200,version:'HTTP/2',headers:[{name:'Content-Type',value:'application/json'}],bodyBase64:encode('{"complete":true}')},
  originalResponse:{status:201,version:'HTTP/2',headers:[{name:'Set-Cookie',value:'a=1'},{name:'Set-Cookie',value:'b=2'}],bodyBase64:encode([0,255,128])}};
function metadata(value=full){const result=structuredClone(value);result.bodyRevision='store:7';result.bodyReferences={};for(const variant of ['request','response','originalRequest','originalResponse'])if(result[variant]){result.bodyReferences[variant]={byteLength:Buffer.from(result[variant].bodyBase64,'base64').length};delete result[variant].bodyBase64}return result}
(async()=>{
  const meta=metadata(),calls=[],copies=[],rewrites=[],edits=[],intercepts=[];
  let respond=async()=>structuredClone(full);
  global.__historyInvoke=async(command,args)=>{calls.push([command,args]);assert.equal(command,'get_flow');return respond()};
  const Menu=(await load('RequestMenu.tsx')).default,errors=[];
  const menu=harness(Menu,{target:{flow:meta,x:20,y:20,anchor:new Element()},onClose(){},onEdit(){},onReplay(){},onCopy:value=>copies.push(value),onError:value=>errors.push(value),replayDisabled:false});
  menu.button('复制 URLCtrl+C').props.onClick();menu.button('复制请求头').props.onClick();assert.equal(calls.length,0,'URL/header copy never loads bodies');assert.deepEqual(copies,[full.request.url,'X-Repeat: one\r\nx-repeat: two']);
  menu.button('复制 cURLCtrl+Shift+C').props.onClick();await menu.tick();assert.match(copies.at(-1),/printf '%b'.*--data-binary @-/);assert.deepEqual(calls.at(-1),['get_flow',{id:meta.id,expectedVersion:'store:7'}]);
  menu.button('复制请求 JSON').props.onClick();await menu.tick();assert.deepEqual(JSON.parse(copies.at(-1)),full.request);
  respond=async()=>{throw Error('Recorded flow changed')};const copied=copies.length;menu.button('复制请求 JSON').props.onClick();await menu.tick();assert.equal(copies.length,copied);assert.match(errors.at(-1),/changed/);menu.dispose();

  const Capture=(await load('CaptureView.tsx')).default;
  const props={flows:[meta],desktop:true,running:true,address:'127.0.0.1:8080',onReplay:f=>edits.push(f),onReplayNow(){},onInterceptResponse:f=>intercepts.push(f),onRewriteResponse:(...args)=>rewrites.push(args),onCopy:value=>copies.push(value),onFlowsChanged(){},busy:false,active:true};
  const capture=harness(Capture,props);assert.equal(calls.length,3,'list/metadata render issues no body calls');
  const row=()=>capture.find(n=>n.type==='tr'&&n.props.onDoubleClick);row().props.onClick({shiftKey:false,ctrlKey:false,metaKey:false});capture.render();
  let view=capture.find(n=>n.type?.name==='MessageView'&&n.props.variant==='response');assert(view,'response body uses the recorded body adapter');
  const body=harness(view.type,view.props);assert.equal(body.tree.type.name,'RecordedBody');assert.equal(body.tree.props.flow,meta);body.dispose();
  const requestBody=capture.all(n=>n.type==='button'&&text(n)==='正文')[0];requestBody.props.onClick();capture.render();
  view=capture.find(n=>n.type?.name==='MessageView'&&n.props.variant==='originalRequest');assert(view,'the genuinely empty original request is selected instead of falling back to current bytes');
  capture.button('自动修改响应').props.onClick();assert.equal(rewrites.at(-1)[0],meta);assert.equal(rewrites.at(-1)[1],'modify');assert.equal(calls.length,3,'modify rules need no complete request or response');
  capture.button('手动响应断点').props.onClick();assert.equal(intercepts.at(-1),meta);assert.equal(calls.length,3);
  global.__historyInvoke=async(command,args)=>{calls.push([command,args]);assert.equal(command,'read_flow_body');assert.deepEqual(args,{id:meta.id,variant:'response',expectedVersion:meta.bodyRevision,offset:0,length:65536});return{offset:0,totalBytes:meta.bodyReferences.response.byteLength,bodyBase64:full.response.bodyBase64,nextOffset:null,text:null}};
  capture.button('自动替换响应').props.onClick();await capture.tick();assert.deepEqual(rewrites.at(-1),[meta,'replace',full.response.bodyBase64],'replacement reads only the selected complete response, not the other three entities');
  const large=metadata();large.bodyReferences.response.byteLength=8*1024*1024+1;capture.props={...props,flows:[large]};capture.render();const beforeLarge=calls.length;capture.button('自动替换响应').props.onClick();capture.render();assert.equal(calls.length,beforeLarge,'oversized replacement is refused before loading all bodies');assert.match(text(capture.tree),/8 MiB/);
  global.__historyInvoke=async(command,args)=>{calls.push([command,args]);assert.equal(command,'get_flow');return respond()};
  capture.props={...props};capture.render();respond=async()=>{throw Error('Recorded flow deleted')};row().props.onDoubleClick();await capture.tick();assert.equal(edits.length,0);assert.match(text(capture.tree),/deleted/);
  const pending=deferred();respond=()=>pending.promise;row().props.onDoubleClick();capture.props={...props,active:false};capture.render();assert(!capture.find(n=>n.props?.className==='capture-inspector'),'hidden capture views unmount all inspectors');pending.resolve(full);await capture.tick();assert.equal(edits.length,0,'late complete actions cannot open editors after leaving capture');capture.dispose();

  const replacements=[],slow=deferred(),fast=deferred();let snapshotReads=0;
  global.__historyInvoke=async command=>{assert.equal(command,'flow_metadata_changes');return ++snapshotReads===1?slow.promise:fast.promise};
  const reload=harness(Capture,{...props,onFlowsChanged:rows=>replacements.push(rows)});
  reload.find(n=>n.type==='tr'&&n.props.onDoubleClick).props.onClick({shiftKey:false,ctrlKey:false,metaKey:false});reload.render();
  const refresh=()=>reload.find(n=>n.type?.name==='MessageView'&&n.props.variant==='response').props.onReloadMetadata();
  refresh();refresh();const fresh={...meta,bodyRevision:'store:9'};fast.resolve({revision:'store:9',reset:true,rows:[{position:1,flow:fresh}],deleted:[]});await reload.tick();slow.resolve({revision:'store:8',reset:true,rows:[{position:1,flow:{...meta,bodyRevision:'store:8'}}],deleted:[]});await reload.tick();assert.deepEqual(replacements.map(rows=>rows[0].bodyRevision),['store:9'],'late metadata reload cannot overwrite a newer snapshot');
  const older=deferred();global.__historyInvoke=async()=>older.promise;refresh();reload.props={...reload.props,flows:[fresh]};reload.render();older.resolve({revision:'store:8',reset:true,rows:[{position:1,flow:meta}],deleted:[]});await reload.tick();assert.equal(replacements.length,1,'an in-flight snapshot cannot overwrite newer polling results');
  const deleting=deferred();global.__historyInvoke=async(command,args)=>{assert.equal(command,'delete_flows');assert.deepEqual(args.ids,[meta.id]);return deleting.promise};reload.button('删除选中（1）').props.onClick();
  const added={...meta,id:'new-capture',bodyRevision:'store:10'};reload.props={...reload.props,flows:[fresh,added]};reload.render();deleting.resolve(null);await reload.tick();assert.deepEqual(replacements.at(-1).map(item=>item.id),['new-capture'],'deleting an old selection preserves captures received while deletion was pending');reload.dispose();

  const {RequestOverview}=await load('RequestDetails.tsx');const overview=harness(RequestOverview,{request:meta.request,flow:meta});assert(overview.all(n=>n.type?.name==='PropertyTable').flatMap(n=>n.props.rows).some(([name,value])=>name==='请求'&&value==='16 B'));overview.props={request:meta.originalRequest,flow:meta};overview.render();assert(overview.all(n=>n.type?.name==='PropertyTable').flatMap(n=>n.props.rows).some(([name,value])=>name==='请求'&&value==='0 B'));overview.dispose();
  const {saveSession}=await load('sessions.ts');const order=[],saved=[];
  global.__historyChoose=async()=>{order.push('picker');return undefined};global.__historySave=async(...args)=>{saved.push(args);return'saved'};
  global.__historyInvoke=async(command,args)=>{order.push('body');assert.equal(command,'get_flow');assert.deepEqual(args,{id:meta.id,expectedVersion:meta.bodyRevision});return full};
  await saveSession([meta],'json');assert.deepEqual(order,['picker','body']);assert.deepEqual(JSON.parse(saved[0][0]).flows,[full],'session exports all four complete entities and duplicate headers');
  for(const raw of [true,false]){global.__historyHar=async(items,rawMode)=>{assert.deepEqual(items,[full]);assert.equal(rawMode,raw);return'complete HAR'};await saveSession([meta],'har',()=>{},raw);assert.equal(saved.at(-1)[0],'complete HAR')}
  global.__historyChoose=async()=>null;const reads=order.length;await saveSession([meta]);assert.equal(order.length,reads,'cancelling picker does not fetch a complete body');
  global.__historyChoose=async()=>undefined;global.__historyInvoke=async()=>{throw Error('Recorded flow changed')};const writes=saved.length;await assert.rejects(()=>saveSession([meta]),/changed/);assert.equal(saved.length,writes,'a failed complete read never saves metadata or a partial session');
  console.log('History actions UI: metadata-only display, original empty body, pinned copy/edit/rewrite, hidden-result guards, exact exports and save-picker cancellation passed.');
})().catch(error=>{console.error(error);process.exitCode=1});
