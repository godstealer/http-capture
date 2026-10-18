const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const {build}=require('esbuild');
// Exercise component event/state contracts without a browser or a network request.
// CodeMirror rendering and native dialog geometry remain browser acceptance checks.
let current;
class Element {
  isConnected=true;open=false;
  focus(){} select(){} scrollIntoView(){} showModal(){this.open=true;} close(){this.open=false;}
  querySelector(){return new Element();}
}
global.HTMLElement=Element;global.document={body:new Element(),activeElement:new Element()};
global.crypto??=require('node:crypto').webcrypto;
const react={
  useState(initial){const h=current,i=h.index++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return [h.slots[i],value=>{h.slots[i]=typeof value==='function'?value(h.slots[i]):value;}];},
  useRef(initial){const h=current,i=h.index++;return h.slots[i]??=( {current:initial} );},
  useId(){const h=current,i=h.index++;return h.slots[i]??=('test-'+i);},
  useEffect(effect,deps){const h=current,i=h.index++,previous=h.slots[i];if(!previous||deps.some((v,j)=>v!==previous[j])){h.effects.push(effect);h.slots[i]=deps;}},
  lazy:()=>function Lazy(){},Suspense:Symbol('Suspense'),
};
const jsx=(type,props)=>{if(typeof type==='string'&&props?.ref&&!props.ref.current)props.ref.current=new Element();return {type,props:props??{}};};
async function load(file){
  const output=await build({entryPoints:[file],bundle:true,platform:'node',format:'cjs',write:false,jsx:'automatic',packages:'external',loader:{'.css':'empty'},plugins:[{name:'ui-contract-boundaries',setup(build){
    build.onResolve({filter:/\?raw$/},args=>({path:path.resolve(args.resolveDir,args.path.slice(0,-4)),namespace:'raw'}));
    build.onLoad({filter:/.*/,namespace:'raw'},args=>({contents:'export default '+JSON.stringify(fs.readFileSync(args.path,'utf8'))}));
    build.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:`export const t=(key,args={})=>key.replace(/\\{(\\w+)\\}/g,(_,name)=>String(args[name]??'{'+name+'}')); export const useLanguage=()=>({language:'zh-CN'});`}));
    build.onLoad({filter:/[/\\]api\.ts$/},()=>({contents:'export const invoke=(...args)=>globalThis.__uiInvoke(...args);'}));
    build.onLoad({filter:/[/\\](?:BodyCodeEditor|ScriptCodeEditor|ScriptsEditor)\.tsx$/},()=>({contents:'export default function Editor(){};export const emptyScripts={enabled:false,before:"",after:"",variables:{}};'}));
  }}]});
  const module={exports:{}};
  new Function('module','exports','require',output.outputFiles[0].text)(module,module.exports,name=>name==='react'?react:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:name==='react-dom'?{createPortal:node=>node}:name==='lucide-react'?new Proxy({},{get:()=>()=>null}):require(name));
  return module.exports.default;
}
function harness(Component,props={}){
  const h={slots:[],effects:[],index:0,tree:null,props};
  h.render=()=>{current=h;h.index=0;h.tree=Component(h.props);for(const effect of h.effects.splice(0))effect();return h.tree;};
  h.find=predicate=>{const result=[];function walk(node){if(!node||typeof node!=='object')return;if(Array.isArray(node)){node.forEach(walk);return;}if(predicate(node))result.push(node);walk(node.props?.children);}walk(h.tree);return result;};
  h.button=text=>h.find(node=>node.type==='button'&&node.props.children===text)[0];
  h.tick=async()=>{await new Promise(resolve=>setImmediate(resolve));h.render();};
  h.render();return h;
}
const input=(h,suffix)=>h.find(node=>node.type==='input'&&node.props.id?.endsWith(suffix))[0];
const change=(node,value)=>node.props.onChange({target:{value}});
const submit=h=>h.find(node=>node.type==='form')[0].props.onSubmit({preventDefault(){}});

(async()=>{
  const SaveRequest=await load('apps/frontend/src/SaveRequestDialog.tsx');
  const saved=[];let closed=0;
  const props={collections:[{id:'c1',name:'One'},{id:'c2',name:'Two'}],initialName:'Request',initialCollectionId:'c2',defaultCollectionName:'我的请求',busy:false,error:'',onSave:value=>saved.push(value),onClose:()=>closed++};
  const dialog=harness(SaveRequest,props);
  assert.equal(dialog.find(node=>node.type==='select')[0].props.value,'c2');
  assert(dialog.find(node=>node.type==='option').every(node=>node.props.value),'collection dropdown contains only real IDs');
  dialog.button('新建集合').props.onClick();dialog.render();
  assert.equal(input(dialog,'-collection-name').props.value,'我的请求');
  change(input(dialog,'-name'),'   ');dialog.render();submit(dialog);dialog.render();assert.equal(saved.length,0);assert(dialog.find(node=>node.props?.role==='alert').length);
  change(input(dialog,'-name'),'  请求  ');change(input(dialog,'-collection-name'),' 新集合 ');dialog.render();submit(dialog);
  assert.deepEqual(saved.pop(),{name:'请求',collectionName:'新集合'});
  dialog.button('使用已有集合').props.onClick();dialog.render();submit(dialog);assert.deepEqual(saved.pop(),{name:'请求',collectionId:'c2'});
  dialog.props={...props,busy:true};dialog.render();submit(dialog);dialog.tree.props.onCancel({preventDefault(){}});assert.equal(closed,0);assert.equal(saved.length,0);
  const first=harness(SaveRequest,{...props,collections:[]});submit(first);assert.deepEqual(saved.pop(),{name:'Request',collectionName:'我的请求'});

  const Library=await load('apps/frontend/src/RequestLibrary.tsx');
  const empty={revision:7,collections:[],requests:[],environments:[],scripts:[]};
  const draft={method:'POST',url:'https://example.invalid/api',headers:[],bodyBase64:'',scripts:{enabled:false,before:'original',after:'',variables:{}}};
  const updates=[],context={requestMode:'grpc',grpcDraft:{method:'/test.Service/Call',json:'{"original":true}'}};
  const library=harness(Library,{value:empty,onChange:value=>updates.push(value),onSave(){},onReload(){},onOpen(){},draft,draftContext:context,message:'',busy:false});
  assert(!library.find(node=>node.type==='input'&&node.props.placeholder==='名称').length);
  library.button('收藏当前请求').props.onClick();library.render();
  const requestDialog=library.find(node=>typeof node.type==='function'&&node.props.initialName)[0];
  draft.url='https://changed.invalid/';context.grpcDraft.json='changed';
  requestDialog.props.onSave({name:'Saved',collectionName:'我的请求'});
  assert.equal(updates[0].requests[0].draft.url,'https://example.invalid/api');assert.equal(updates[0].requests[0].grpcDraft.json,'{"original":true}');assert.equal(updates[0].revision,7);
  let finish;const before=updates.length;
  const importing=library.find(node=>node.type==='input'&&node.props.type==='file')[0].props.onChange({target:{files:[{text:()=>new Promise(resolve=>finish=resolve)}],value:''}});
  library.props={...library.props,value:{...empty,revision:8}};library.render();
  finish(JSON.stringify({format:'http-capture-library',version:1,library:empty}));await importing;library.render();assert.equal(updates.length,before);assert(library.find(node=>node.props?.role==='alert').length);

  const Plugins=await load('apps/frontend/src/Plugins.tsx');
  const installed=[{id:'one',name:'First',version:'1',apiVersion:1,enabled:true,scope:'all',urlPattern:'^http://localhost/',before:'',after:''},{id:'two',name:'Second',version:'1',apiVersion:1,enabled:false,scope:'replay',urlPattern:'^http://localhost/',before:'',after:''}];
  const calls=[];let failSave=false,failCheck=false;
  global.__uiInvoke=async(command,args)=>{calls.push([command,args]);if(command==='plugins_list')return installed;if(command==='plugin_logs')return [];if(command==='plugins_validate'&&failCheck)throw Error('before SyntaxError: line 2');if(command==='plugins_save'&&failSave)throw Error('save failed');};
  const plugins=harness(Plugins);await plugins.tick();
  const rows=()=>plugins.find(node=>node.props?.className==='plugin-list-row');assert.equal(rows().length,2);
  change(plugins.find(node=>node.props?.['aria-label']==='按生效范围筛选')[0],'both');plugins.render();assert.equal(rows().length,1);assert.equal(rows()[0].props.children[1].props.children[0].props.children,'First');
  change(plugins.find(node=>node.props?.['aria-label']==='搜索插件')[0],'no-match');plugins.render();assert.equal(rows().length,0);assert(plugins.find(node=>node.props?.className==='plugin-list-empty').length);
  change(plugins.find(node=>node.props?.['aria-label']==='搜索插件')[0],'');change(plugins.find(node=>node.props?.['aria-label']==='按生效范围筛选')[0],'all');plugins.render();
  failSave=true;plugins.find(node=>node.type==='input'&&node.props.type==='checkbox')[0].props.onChange({target:{checked:false}});await plugins.tick();assert.equal(plugins.find(node=>node.type==='input'&&node.props.type==='checkbox')[0].props.checked,true);failSave=false;
  plugins.find(node=>node.props?.['aria-label']==='编辑插件：First')[0].props.onClick();plugins.render();assert.equal(input(plugins,'-id').props.readOnly,true);
  change(input(plugins,'-name'),'Edited');plugins.render();failCheck=true;plugins.button('检查并保存插件').props.onClick();await plugins.tick();assert.equal(input(plugins,'-name').props.value,'Edited');assert(!calls.some(([command,args])=>command==='plugins_save'&&args.plugins[0].name==='Edited'));
  failCheck=false;plugins.button('检查并保存插件').props.onClick();await plugins.tick();const persisted=calls.filter(([command])=>command==='plugins_save').at(-1)[1].plugins;assert.equal(persisted[0].enabled,false);assert.equal(persisted[0].name,'Edited');
  plugins.button('预览此示例').props.onClick();plugins.render();assert(!calls.some(([command,args])=>command==='plugins_save'&&args.plugins.some(plugin=>plugin.id==='local-request-id')));plugins.button('安装插件').props.onClick();await plugins.tick();assert.equal(calls.filter(([command])=>command==='plugins_save').at(-1)[1].plugins.at(-1).enabled,false);
  await plugins.find(node=>node.type==='input'&&node.props.type==='file')[0].props.onChange({target:{files:[{size:10,text:async()=>JSON.stringify({...installed[0],version:{invalid:'object'}})}],value:''}});plugins.render();assert(!plugins.button('安装插件'));assert(plugins.find(node=>node.props?.role==='alert').length);
  console.log('Management UI: explicit collection creation, first-save defaults, validation/busy protection, gRPC snapshots, stale import rejection, plugin filters, failed-write rollback, compile-before-save and disabled example preview/install passed.');
})().catch(error=>{console.error(error);process.exitCode=1;});
