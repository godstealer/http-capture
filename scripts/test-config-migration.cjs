const assert=require('node:assert/strict');
const {build}=require('esbuild');
let current;
class Element {value='';clicks=0;click(){this.clicks++}}
const react={
  useState(initial){const h=current,i=h.index++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return [h.slots[i],value=>{h.slots[i]=typeof value==='function'?value(h.slots[i]):value}]},
  useRef(initial){const h=current,i=h.index++;return h.slots[i]??={current:initial}},
  useEffect(effect,deps){const h=current,i=h.index++,previous=h.slots[i];if(!previous||deps.some((value,index)=>value!==previous[index])){h.effects.push(()=>{h.cleanups[i]?.();h.cleanups[i]=effect()});h.slots[i]=deps}},
};
const jsx=(type,props)=>{if(typeof type==='string'&&props?.ref&&!props.ref.current)props.ref.current=new Element();return {type,props:props??{}}};
async function load(file){
  const result=await build({entryPoints:[file],bundle:true,platform:'node',format:'cjs',jsx:'automatic',write:false,packages:'external',loader:{'.css':'empty'},plugins:[{name:'configuration-contracts',setup(builder){
    builder.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export const t=(key)=>key;'}));
    builder.onLoad({filter:/[/\\]api\.ts$/},()=>({contents:'export const invoke=(...args)=>globalThis.__configInvoke(...args);'}));
    builder.onLoad({filter:/[/\\]saveFile\.ts$/},()=>({contents:'export const chooseSave=(...args)=>globalThis.__configChooseSave(...args); export const saveText=(...args)=>globalThis.__configSaveText(...args);'}));
  }}]});
  const module={exports:{}};
  new Function('module','exports','require',result.outputFiles[0].text)(module,module.exports,name=>name==='react'?react:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:name==='@tauri-apps/api/core'?{isTauri:()=>!!global.__configDesktop}:name==='lucide-react'?new Proxy({},{get:()=>()=>null}):require(name));
  return module.exports;
}
function text(node){if(node==null||node===false)return '';if(typeof node==='string'||typeof node==='number')return String(node);if(Array.isArray(node))return node.map(text).join('');return text(node.props?.children)}
function find(tree,predicate){const values=[];function walk(node){if(!node||typeof node!=='object')return;if(Array.isArray(node)){node.forEach(walk);return}if(predicate(node))values.push(node);walk(node.props?.children)}walk(tree);return values}
function harness(Component,props){
  const h={slots:[],index:0,tree:null,effects:[],cleanups:[]};h.render=()=>{current=h;h.index=0;h.tree=Component(props);for(const effect of h.effects.splice(0))effect();return h.tree};h.find=predicate=>find(h.tree,predicate);h.button=name=>h.find(node=>node.type==='button'&&text(node)===name)[0];h.textarea=()=>h.find(node=>node.type==='textarea')[0];h.inputFor=label=>find(h.find(node=>node.type==='label'&&text(node).startsWith(label))[0],node=>node.type==='input')[0];h.tick=async()=>{await new Promise(resolve=>setImmediate(resolve));h.render()};h.dispose=()=>{for(const cleanup of h.cleanups)cleanup?.()};h.render();return h;
}
const change=(node,value)=>node.props.onChange({target:{value}});
const check=(node,checked)=>node.props.onChange({target:{checked}});
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return {promise,resolve,reject}};

(async()=>{
  const {configurationPreferenceKeys:keys,readConfigurationPreferences:read,applyConfigurationPreferences:apply}=await load('apps/frontend/src/configPreferences.ts');
  const stored=new Map([['http-capture.theme','dark'],['http-capture.language','en'],['http-capture.zoom','125'],['http-capture.app-update.skipped-version','0.1.23'],['http-capture.mcp.token','never-export'],['unrelated','preserve']]);
  let failKey=null;
  const storage={getItem:key=>stored.get(key)??null,setItem(key,value){if(key===failKey){failKey=null;throw Error('quota fixture')}stored.set(key,value)},removeItem:key=>stored.delete(key)};
  assert.deepEqual(read(storage),{'http-capture.theme':'dark','http-capture.language':'en','http-capture.zoom':'125'});
  assert(!keys.some(key=>/token|update|password/i.test(key)),'preference whitelist excludes credentials and update decisions');
  const original=new Map(stored);assert.throws(()=>apply({'http-capture.mcp.token':'injected'},storage));assert.deepEqual(stored,original,'unknown keys are rejected before any mutation');
  assert.throws(()=>apply({'http-capture.theme':5},storage));assert.deepEqual(stored,original);
  apply({'http-capture.theme':'light'},storage);assert.equal(stored.get('http-capture.theme'),'light');assert(!stored.has('http-capture.zoom'),'a selected preferences category replaces missing keys with defaults');assert.equal(stored.get('http-capture.mcp.token'),'never-export');assert.equal(stored.get('http-capture.app-update.skipped-version'),'0.1.23');
  stored.set('http-capture.theme','dark');stored.set('http-capture.language','en');const beforeFailure=new Map(stored);failKey='http-capture.language';
  assert.throws(()=>apply({'http-capture.theme':'paper','http-capture.language':'zh-CN'},storage),/quota fixture/);assert.deepEqual(stored,beforeFailure,'a partially applied preference write is rolled back');
  const optional={'http-capture.request-modes':'{}','http-capture.grpc.proto.v1':'syntax = "proto3";','http-capture.clienthello-templates.v1':'[]'};
  for(const [key,value] of Object.entries(optional))stored.set(key,value);
  apply({},storage);for(const key of keys)if(Object.hasOwn(optional,key))assert.equal(stored.get(key),optional[key],'missing optional protocol/template preferences keep their local values');else assert(!stored.has(key));assert.equal(stored.get('unrelated'),'preserve');
  const optionalBefore=new Map(stored);failKey='http-capture.grpc.proto.v1';assert.throws(()=>apply({'http-capture.clienthello-templates.v1':'[{}]','http-capture.grpc.proto.v1':'updated'},storage),/quota fixture/);assert.deepEqual(stored,optionalBefore,'rollback also preserves optional local schema/templates');
  for(const key of Object.keys(optional))stored.delete(key);
  global.localStorage=storage;stored.set('http-capture.theme','dark');

  const calls=[],saves=[],saveChoices=[],callbacks=[];let beforeCalls=0,beforeError=false,beforeWait=null,apiError=null,chooseCancelled=false;
  const preview={expectedRevision:'b'.repeat(64),sections:[{id:'preferences',count:1},{id:'library',count:2},{id:'workspace',count:3}],warnings:['fixture preview warning'],requiresRestart:true,sourceVersion:'0.1.test',exportedAt:Date.UTC(2026,9,19)};
  const result={importedSections:['preferences','library'],preferences:{'http-capture.theme':'paper'},requiresRestart:true,recoveryAvailable:true,warnings:['MCP permissions reset fixture']};
  global.__configDesktop=false;
  global.__configChooseSave=async name=>{saveChoices.push(name);return chooseCancelled?null:{name}};
  global.__configSaveText=async(...args)=>{saves.push(args);return 'fixture saved'};
  global.__configInvoke=async(command,args)=>{calls.push([command,structuredClone(args)]);if(apiError===command)throw Error('fixture '+command+' failed');if(command==='config_preview')return structuredClone(preview);if(command==='config_import')return structuredClone(result);if(command==='config_export'||command==='config_recovery')return {document:'{"format":"http-capture-config"}',summary:{sections:[],warnings:['fixture export warning']}};if(command==='config_save_file')return 'C:/Downloads/config.json';throw Error('Unexpected API '+command)};
  const Component=(await load('apps/frontend/src/ConfigMigration.tsx')).default;
  const props={connected:false,beforeTransfer:async()=>{beforeCalls++;if(beforeWait)await beforeWait.promise;if(beforeError)throw Error('unsaved changes fixture')},onImported:value=>callbacks.push(value)};
  const h=harness(Component,props);assert.equal(calls.length,0,'opening configuration transfer does not read or export settings');
  assert(h.button('导出全局配置').props.disabled);change(h.textarea(),'{}');h.render();h.button('预览导入内容').props.onClick();await h.tick();assert.equal(calls.length,0,'disconnected controls cannot call the API');
  props.connected=true;h.render();for(const label of ['包含请求内容和变量值','包含脚本与插件代码','包含请求工作区草稿','包含代理用户名'])assert.equal(h.inputFor(label).props.checked,false);
  chooseCancelled=true;h.button('导出全局配置').props.onClick();await h.tick();assert.equal(calls.length,0);assert.equal(beforeCalls,0,'cancelled save never flushes or exports configuration');chooseCancelled=false;
  beforeError=true;h.button('导出全局配置').props.onClick();await h.tick();assert.equal(calls.length,0,'unsaved-change protection prevents export');assert(h.find(node=>node.props?.role==='alert').length);beforeError=false;
  h.button('导出全局配置').props.onClick();await h.tick();let exported=calls.at(-1);assert.equal(exported[0],'config_export');assert.deepEqual(exported[1].options,{includeRequestData:false,includeCode:false,includeWorkspace:false,includeProxyUsernames:false});assert.deepEqual(exported[1].preferences,{'http-capture.theme':'dark'});assert.equal(saves.length,1);assert.equal(saves[0][0],'{"format":"http-capture-config"}');assert(/http-capture-config-\d{4}-\d{2}-\d{2}\.json/.test(saveChoices.at(-1)));assert(text(h.tree).includes('fixture export warning'));
  const callsBeforeOptionExport=calls.length;for(const label of ['包含请求内容和变量值','包含脚本与插件代码','包含请求工作区草稿','包含代理用户名']){check(h.inputFor(label),true);h.render()}
  assert.equal(calls.length,callsBeforeOptionExport,'changing export options does not call APIs');h.button('导出全局配置').props.onClick();await h.tick();assert.deepEqual(calls.at(-1)[1].options,{includeRequestData:true,includeCode:true,includeWorkspace:true,includeProxyUsernames:true});
  h.button('导出上次导入前恢复点').props.onClick();await h.tick();assert.equal(calls.at(-1)[0],'config_recovery');assert.deepEqual(calls.at(-1)[1],{},'recovery export is a distinct explicit operation');

  const raw='{"format":"http-capture-config","schemaVersion":1}';change(h.textarea(),raw);h.render();assert(!h.button('导入所选配置并停止网络'));const callCount=calls.length;
  h.button('预览导入内容').props.onClick();await h.tick();assert.equal(calls.length,callCount+1);assert.deepEqual(calls.at(-1),['config_preview',{document:raw}]);assert(h.button('导入所选配置并停止网络'));assert(text(h.tree).includes('fixture preview warning'));
  check(h.inputFor('请求草稿'),false);h.render();assert.equal(calls.length,callCount+1,'choosing categories does not import');
  beforeWait=deferred();h.button('导入所选配置并停止网络').props.onClick();h.render();assert(h.textarea().props.disabled);assert.equal(calls.at(-1)[0],'config_preview','import waits for the unsaved-change guard');assert.equal(callbacks.length,0);
  beforeWait.resolve();beforeWait=null;await h.tick();assert.deepEqual(calls.at(-1),['config_import',{document:raw,sections:['preferences','library'],expectedRevision:preview.expectedRevision,preferences:{'http-capture.theme':'dark'}}]);assert.deepEqual(callbacks,[result],'successful import informs the parent to freeze the old UI and require restart');

  // Fresh component: a failed guard or preview never destroys pasted content.
  const guarded=harness(Component,props);change(guarded.textarea(),raw);guarded.render();guarded.button('预览导入内容').props.onClick();await guarded.tick();beforeError=true;const beforeBlocked=calls.filter(([command])=>command==='config_import').length;guarded.button('导入所选配置并停止网络').props.onClick();await guarded.tick();assert.equal(calls.filter(([command])=>command==='config_import').length,beforeBlocked);assert.equal(guarded.textarea().props.value,raw);assert(!guarded.button('导入所选配置并停止网络'),'guard failure invalidates the old preview');assert(guarded.find(node=>node.props?.role==='alert').length);beforeError=false;
  guarded.button('预览导入内容').props.onClick();await guarded.tick();change(guarded.textarea(),raw+' ');guarded.render();assert(!guarded.button('导入所选配置并停止网络'),'editing pasted JSON invalidates its preview and selected categories');
  guarded.button('预览导入内容').props.onClick();await guarded.tick();let fileReads=0;const fileInput=()=>guarded.find(node=>node.type==='input'&&node.props.type==='file')[0],beforeFileRead=calls.length;
  fileInput().props.onChange({target:{files:[{size:5,text:async()=>{fileReads++;return 'file content'}}]}});await guarded.tick();assert.equal(fileReads,1);assert.equal(guarded.textarea().props.value,'file content');assert(!guarded.button('导入所选配置并停止网络'),'a new file invalidates the old preview');assert.equal(calls.length,beforeFileRead,'choosing a file never automatically previews or imports it');
  const fileBeforeCalls=calls.length;fileInput().props.onChange({target:{files:[{size:16*1024*1024+1,text:async()=>{throw Error('must not read oversized files')}}]}});await guarded.tick();assert.equal(guarded.textarea().props.value,'file content');assert.equal(calls.length,fileBeforeCalls);assert(guarded.find(node=>node.props?.role==='alert').length);
  apiError='config_preview';guarded.button('预览导入内容').props.onClick();await guarded.tick();assert.equal(guarded.textarea().props.value,'file content');assert(!guarded.button('导入所选配置并停止网络'));apiError=null;
  guarded.button('预览导入内容').props.onClick();await guarded.tick();apiError='config_import';guarded.button('导入所选配置并停止网络').props.onClick();await guarded.tick();assert.equal(guarded.textarea().props.value,'file content');assert(!guarded.button('导入所选配置并停止网络'));assert.equal(callbacks.length,1,'failed import never reports success');apiError=null;

  global.__configDesktop=true;guarded.button('导出全局配置').props.onClick();await guarded.tick();assert.equal(calls.at(-1)[0],'config_save_file');assert.equal(calls.at(-1)[1].content,'{"format":"http-capture-config"}');assert.equal(saves.length,3,'native saving does not also trigger a browser download');
  const oversizedText='你'.repeat(Math.floor(16*1024*1024/3)+1),beforeOversized=calls.length;change(guarded.textarea(),oversizedText);guarded.render();guarded.button('预览导入内容').props.onClick();await guarded.tick();assert.equal(calls.length,beforeOversized,'pasted JSON is limited by UTF-8 byte length, not character count');assert.equal(guarded.textarea().props.value,oversizedText);assert(guarded.find(node=>node.props?.role==='alert').length);

  const recovery=await load('apps/frontend/src/configImportRecovery.ts');
  const recoveryValues=new Map([['http-capture.theme','dark'],['http-capture.grpc.proto.v1','["local schema"]']]);let failAck=false;
  const recoveryStorage={getItem:key=>recoveryValues.get(key)??null,setItem(key,value){if(failAck&&key===recovery.configurationImportAckKey){failAck=false;throw Error('ack unavailable')}recoveryValues.set(key,value)},removeItem:key=>recoveryValues.delete(key)};
  const committed={epoch:'11111111-1111-4111-8111-111111111111',frozen:false,importedSections:['preferences'],preferences:{'http-capture.theme':'paper','http-capture.language':'en'},warnings:[]};
  failAck=true;assert.throws(()=>recovery.acknowledgeConfigurationImport(committed,recoveryStorage),/ack unavailable/);assert(!recoveryValues.has(recovery.configurationImportAckKey),'failed ack cannot mark an import as restored');
  assert.equal(recovery.acknowledgeConfigurationImport(committed,recoveryStorage),true);assert.equal(recoveryValues.get('http-capture.theme'),'paper');assert.equal(recoveryValues.get('http-capture.grpc.proto.v1'),'["local schema"]');
  recoveryValues.set('http-capture.theme','cyberpunk');assert.equal(recovery.acknowledgeConfigurationImport(committed,recoveryStorage),false);assert.equal(recoveryValues.get('http-capture.theme'),'cyberpunk','acknowledged imports never overwrite a later user preference');assert(!Object.hasOwn(read(recoveryStorage),recovery.configurationImportAckKey),'local acknowledgements are never portable');
  assert.equal(recovery.acknowledgeConfigurationImport({importedSections:[]},{getItem(){throw Error('must not access storage')}}),false,'temporary frozen metadata with no preference category does not acknowledge an unknown epoch');
  const next={...committed,epoch:'22222222-2222-4222-8222-222222222222',preferences:{'http-capture.theme':'light'}};
  assert.equal((await recovery.recoverConfigurationStartup(async()=>next,recoveryStorage)).epoch,next.epoch);assert.equal(recoveryValues.get('http-capture.theme'),'light','a new committed epoch recovers a lost response at startup');
  const beforeStale=new Map(recoveryValues),pending=deferred();let live=true;const staleRecovery=recovery.recoverConfigurationStartup(()=>pending.promise,recoveryStorage,()=>live);live=false;pending.resolve(committed);assert.equal(await staleRecovery,undefined);assert.deepEqual(recoveryValues,beforeStale,'an unmounted or superseded startup cannot reapply stale preferences');
  assert.equal(await recovery.recoverConfigurationStartup(async()=>{throw Error('HTTP 404')},recoveryStorage),undefined);assert.equal(await recovery.recoverConfigurationStartup(async()=>{throw Error('offline')},recoveryStorage),undefined);

  const Startup=(await load('apps/frontend/src/ConfigurationStartup.tsx')).default;let mounted=0,prepared=0;const startupCalls=[];
  global.__configInvoke=async command=>{startupCalls.push(command);assert.equal(command,'config_status');return structuredClone({...committed,frozen:true})};
  failKey='http-capture.language';
  const startup=harness(Startup,{onReady:()=>prepared++,children:status=>{mounted++;return {type:'div',props:{children:status?.frozen?'frozen workbench':'ready workbench'}}}});
  assert.equal(mounted,0,'workbench initialization waits until committed preferences are projected');await startup.tick();assert.equal(mounted,0);assert(startup.find(node=>node.props?.role==='alert').length);assert(startup.button('重试恢复配置'));
  startup.button('重试恢复配置').props.onClick();startup.render();await startup.tick();assert.equal(mounted,1);assert.equal(prepared,1);assert.equal(text(startup.tree),'frozen workbench','startup preserves the backend freeze so refreshing only the UI cannot bypass restart');assert.equal(stored.get('http-capture.theme'),'paper');assert.deepEqual(startupCalls,['config_status','config_status']);startup.dispose();
  global.__configInvoke=async command=>{assert.equal(command,'config_status');throw Error('HTTP 404: old backend')};
  const oldCore=harness(Startup,{children:status=>({type:'div',props:{children:status?'unexpected status':'legacy workbench'}})});await oldCore.tick();assert.equal(text(oldCore.tree),'legacy workbench');oldCore.dispose();
  const pendingStatus=deferred();global.__configInvoke=()=>pendingStatus.promise;let unmountedChildren=0;
  const abandoned=harness(Startup,{children:()=>{unmountedChildren++;return 'must not mount'}});abandoned.dispose();pendingStatus.resolve(next);await abandoned.tick();assert.equal(unmountedChildren,0);
  const bodyPreferences=await load('apps/frontend/src/bodyEditorPreferences.ts');assert.equal(bodyPreferences.autoFormatPasteEnabled(),false);stored.set('http-capture.body-editor.auto-format-paste.v1','true');const beforeCacheRefresh=new Map(stored);bodyPreferences.reloadBodyEditorPreferences();assert.equal(bodyPreferences.autoFormatPasteEnabled(),true);assert.deepEqual(stored,beforeCacheRefresh,'startup refreshes the eager paste-format cache without another storage write');
  console.log('Configuration transfer: preference whitelist/default replacement/rollback, explicit APIs, export privacy options, category selection and CAS, preview invalidation, limits, error preservation, unsaved-change guard, import callback and native/browser save separation passed.');
  console.log('Configuration startup: lost-response recovery before mounting, once-per-epoch acknowledgement, later user changes, optional local preferences, failed write/ack retry, legacy/offline fallback, stale unmount guard, freeze propagation and eager preference caches passed.');
})().catch(error=>{console.error(error);process.exitCode=1});
