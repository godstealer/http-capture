const assert=require('node:assert/strict');
const path=require('node:path');
const {build}=require('esbuild');
let current;
const react={
  useState(initial){const h=current,i=h.index++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return[h.slots[i],value=>{const next=typeof value==='function'?value(h.slots[i]):value;if(!Object.is(next,h.slots[i])){h.slots[i]=next;h.changed=true}}]},
  useRef(initial){const h=current,i=h.index++;return h.slots[i]??={current:initial}},
  useEffect(effect,deps){const h=current,i=h.index++,old=h.slots[i];if(!old||!deps||deps.some((value,index)=>value!==old[index])){h.slots[i]=deps;if(effect.toString().includes('proxy_status'))h.effects.push(()=>{h.cleanups[i]?.();h.cleanups[i]=effect()})}},
};
const jsx=(type,props={})=>({type,props});
const nodes=node=>Array.isArray(node)?node.flatMap(nodes):node&&typeof node==='object'?[node,...nodes(node.props?.children)]:[];
const text=node=>Array.isArray(node)?node.map(text).join(''):node&&typeof node==='object'?text(node.props?.children):node===null||node===undefined||node===false?'':String(node);
global.crypto??=require('node:crypto').webcrypto;
global.localStorage={getItem:()=>null,setItem(){},removeItem(){}};
global.window={addEventListener(){},removeEventListener(){},matchMedia:()=>({matches:false})};
global.document={documentElement:{style:{setProperty(){}},dataset:{}}};
const timeouts=[];global.setTimeout=(callback,ms)=>{const entry={callback,ms};timeouts.push(entry);return entry};global.clearTimeout=entry=>{if(entry)entry.cancelled=true};
const encoded=text=>Buffer.from(text).toString('base64');
const full={id:'saved-flow',parentId:null,source:'replay',startedAt:1,durationMs:1,state:'complete',request:{method:'POST',url:'https://history.test/api',headers:[],bodyBase64:encoded('complete request body'),tls:{preset:'native'},engine:'auto'},response:{status:200,version:'HTTP/2',headers:[],bodyBase64:encoded('complete response body')},notes:[],error:null};
const meta={...full,request:{...full.request},response:{...full.response},bodyRevision:'store:1',bodyReferences:{request:{byteLength:21},response:{byteLength:22}}};delete meta.request.bodyBase64;delete meta.response.bodyBase64;
const status={running:false,address:null,caPath:'',browserReplay:false,sendEngines:[{id:'auto',available:true,profiles:['native'],reason:null}]};
let workspace;
async function load(){
  const app=path.resolve('apps/frontend/src/App.tsx');
  const result=await build({entryPoints:[app],bundle:true,platform:'node',format:'cjs',write:false,packages:'external',jsx:'automatic',loader:{'.css':'empty'},plugins:[{name:'history-app-boundaries',setup(builder){
    builder.onResolve({filter:/^\.\//},args=>{
      if(args.importer===app&&/^[A-Z]/.test(args.path.slice(2))&&args.path!=='./App')return{path:args.path.slice(2),namespace:'view'};
    });
    builder.onLoad({filter:/.*/,namespace:'view'},args=>({contents:`export default function ${args.path}(){}; export function RequestOverview(){};export function TlsDetails(){};${args.path==='FileProgress'?'export {FileProgress};':'export function FileProgress(){};'}export const emptyScripts={enabled:false,before:'',after:'',variables:{}};`}));
    builder.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export const t=(key,args={})=>key.replace(/\\{(v[0-9]+)\\}/g,(_,name)=>String(args[name]??""));export const useLanguage=()=>{};'}));
    builder.onLoad({filter:/[/\\]api\.ts$/},()=>({contents:'export const invoke=(...args)=>globalThis.__historyInvoke(...args);'}));
    builder.onLoad({filter:/[/\\]clipboard\.ts$/},()=>({contents:'export const writeClipboard=(value)=>globalThis.__historyCopy(value);export const readClipboard=async()=>"";'}));
    builder.onLoad({filter:/[/\\]useRequestWorkspace\.ts$/},()=>({contents:'export const useRequestWorkspace=()=>globalThis.__historyWorkspace();'}));
    builder.onLoad({filter:/[/\\]useDesktopClose\.ts$/},()=>({contents:'export const useDesktopClose=()=>false;'}));
    builder.onLoad({filter:/[/\\]useUiZoom\.ts$/},()=>({contents:'export const useUiZoom=()=>{};'}));
  }}]});
  const module={exports:{}};new Function('module','exports','require',result.outputFiles[0].text)(module,module.exports,name=>name==='react'?react:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:name==='@tauri-apps/api/core'?{isTauri:()=>false}:name==='lucide-react'?new Proxy({},{get:()=>()=>null}):require(name));return module.exports.default;
}
function mount(Component){
  const h={slots:[],index:0,effects:[],cleanups:[],changed:false,tree:null};
  workspace={editors:[{id:'editor',draft:{...structuredClone(full.request),url:'https://draft.test/'},selected:full.id,parentId:null}],activeEditor:'editor',ready:true,message:'',error:'',retry:async()=>true,synchronize:async()=>{},setEditors(update){workspace.editors=typeof update==='function'?update(workspace.editors):update;h.changed=true},setActiveEditor(id){workspace.activeEditor=id;h.changed=true}};
  global.__historyWorkspace=()=>workspace;
  h.render=()=>{let iterations=0;do{assert(iterations++<30,'App effects converge');h.changed=false;current=h;h.index=0;h.tree=Component();h.effects.splice(0).forEach(effect=>effect())}while(h.changed)};
  h.all=predicate=>nodes(h.tree).filter(predicate);h.find=predicate=>h.all(predicate)[0];
  h.component=name=>h.find(node=>typeof node.type==='function'&&node.type.name===name);
  h.tick=async()=>{for(let turn=0;turn<4;turn++){await new Promise(resolve=>setImmediate(resolve));h.render()}};
  h.clickTitle=title=>{const button=h.find(node=>node.type==='button'&&node.props.title===title);assert(button,title);button.props.onClick();h.render()};
  h.historyButton=()=>h.find(node=>node.type==='details'&&node.props.className==='request-history').props.children[1].props.children[0];
  h.openHistory=()=>{h.historyButton().props.onClick({currentTarget:{closest:()=>({removeAttribute(){}})}});h.render()};
  h.dispose=()=>h.cleanups.forEach(cleanup=>cleanup?.());h.render();return h;
}
(async()=>{
  const calls=[],copied=[];let getResult=async()=>structuredClone(full),history=[meta];
  global.__historyCopy=async value=>copied.push(value);
  global.__historyInvoke=async(command,args)=>{
    calls.push([command,args]);
    if(command==='flow_metadata_changes')return{revision:'store:1',reset:true,rows:history.map((flow,index)=>({position:index,flow})),deleted:[]};
    if(command==='flow_metadata')return structuredClone(meta);
    if(command==='get_flow')return getResult();
    if(command==='proxy_status')return status;
    if(command==='tun_status')return {available:false,running:false,helperPath:''};
    if(command==='interception')return {config:{request:false,response:false,scope:'all'},items:[]};
    if(command==='prepare_replay')return 'execution';
    if(command==='replay_request')return structuredClone(full);
    if(command==='cancel_replay')return null;
    throw Error('Unexpected host command '+command);
  };
  const Component=await load();
  const realAtob=global.atob;let decodes=0;global.atob=value=>{decodes++;return realAtob(value)};
  const h=mount(Component);await h.tick();
  assert.equal(h.component('CaptureView').props.flows.length,1);assert.equal(h.component('CaptureView').props.flows[0],meta);
  assert.equal(decodes,0,'browsing metadata and rendering hidden composer does not decode response bytes');
  assert(!h.component('RecordedBody')&&!h.component('ResponseBody')&&!h.component('GrpcResponse'),'hidden replay panels mount no response decoder');
  assert(!calls.some(([command])=>['list_flows','flow_changes','get_flow','read_flow_body'].includes(command)));
  h.clickTitle('https://draft.test/');assert(h.component('RecordedBody'));assert.equal(h.component('RecordedBody').props.flow,meta);assert.equal(h.component('RecordedBody').props.variant,'response');
  h.find(node=>node.type==='button'&&node.props['aria-label']==='收起响应面板').props.onClick();h.render();assert(!h.component('RecordedBody'),'collapsing unmounts body workers/readers');
  h.find(node=>node.type==='button'&&node.props['aria-label']==='展开响应面板').props.onClick();h.render();
  const headers=h.find(node=>node.type==='button'&&node.props.children==='响应头');headers.props.onClick();h.render();assert(!h.component('RecordedBody'));assert.equal(decodes,0);
  const copy=h.find(node=>node.type==='button'&&node.props.title==='复制响应');copy.props.onClick();await h.tick();
  assert.deepEqual(copied,['complete response body']);assert.deepEqual(calls.filter(([command])=>command==='get_flow').at(-1)[1],{id:full.id,expectedVersion:'store:1'});assert.equal(decodes,1,'only the explicit complete copy decodes response bytes');
  assert(h.component('CaptureView').props.flows.every(record=>!Object.hasOwn(record.request,'bodyBase64')&&!Object.hasOwn(record.response,'bodyBase64')),'complete clipboard data does not enter history');
  getResult=async()=>{throw Error('Recorded flow changed; reload its metadata')};copy.props.onClick();await h.tick();assert.equal(copied.length,1,'stale complete copy never falls back to a prefix');
  const count=workspace.editors.length;h.openHistory();await h.tick();assert.equal(workspace.editors.length,count,'a failed pinned history open creates no editor');
  getResult=async()=>structuredClone(full);h.openHistory();await h.tick();assert.equal(workspace.editors.length,count+1);assert.equal(workspace.editors.at(-1).draft.bodyBase64,full.request.bodyBase64);assert.equal(workspace.editors.at(-1).selected,full.id);
  assert(h.component('CaptureView').props.flows.every(record=>!Object.hasOwn(record.response,'bodyBase64')),'opening a complete editor also leaves history metadata-only');
  h.find(node=>node.type==='button'&&String(node.props.className).includes('work-tab')&&text(node).startsWith('调试')).props.onClick();h.render();assert(!h.component('RecordedBody')&&!h.component('GrpcResponse')&&!h.component('ResponseBody'),'leaving replay unmounts response decoders');
  h.component('CaptureView').props.onReplayNow(structuredClone(full));await h.tick();
  assert(calls.some(([command])=>command==='replay_request'),'an explicit CaptureView complete-flow callback still sends');
  assert(calls.some(([command])=>command==='flow_metadata'),'completed sends retrieve an authoritative metadata revision before history insertion');
  assert(h.component('CaptureView').props.flows.every(record=>!Object.hasOwn(record.request,'bodyBase64')&&!Object.hasOwn(record.response,'bodyBase64')));
  h.dispose();global.atob=realAtob;
  console.log('App history UI: metadata polling without decoding, visible-only bounded response mounting, exact copy/history-open reads, conflict rejection, complete editor bodies and metadata-only replay result merging passed.');
})().catch(error=>{console.error(error);process.exitCode=1});
