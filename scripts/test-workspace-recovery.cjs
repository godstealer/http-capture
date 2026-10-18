const assert=require('node:assert/strict');
const {buildSync}=require('esbuild');
global.crypto??=require('node:crypto').webcrypto;
function load(path,external=[],resolve=require,timers={setTimeout,clearTimeout},window={addEventListener(){},removeEventListener(){}}){
  const bundle=buildSync({entryPoints:[path],bundle:true,platform:'node',format:'cjs',write:false,external});
  const module={exports:{}};new Function('module','exports','require','setTimeout','clearTimeout','window',bundle.outputFiles[0].text)(module,module.exports,resolve,timers.setTimeout,timers.clearTimeout,window);return module.exports;
}
const {reconcileWorkspace,isWorkspaceConflict}=load('apps/frontend/src/workspaceRecovery.ts');
const draft=(value)=>({method:'POST',url:'https://example.invalid/'+value,headers:[{name:'X-Same',value:'first'},{name:'X-Same',value:'disabled',disabled:true}],bodyBase64:Buffer.from(value+' 中文\0').toString('base64'),tls:{preset:'native'},engine:'auto'});
const editor=(id,value=id)=>({id,draft:draft(value),selected:null,parentId:null,savedRequestId:'saved-'+id,grpcDraft:{method:'/Echo/Call',json:'{"unfinished":'}});
const snapshot=(editors,activeEditor=editors[0]?.id??'')=>({editors,activeEditor});
const changed=(item,value)=>({...structuredClone(item),draft:draft(value)});

{
  const base=snapshot([editor('a'),editor('b')]);
  const local=snapshot([changed(base.editors[0],'local-a'),base.editors[1]]);
  const remote=snapshot([base.editors[0],changed(base.editors[1],'remote-b'),editor('remote-new')],'remote-new');
  const inputs=JSON.stringify([base,local,remote]);
  const result=reconcileWorkspace(base,local,remote);
  assert.equal(result.copied,0);assert.equal(result.updated,1);assert.equal(result.added,0);
  assert.deepEqual(result.workspace.editors,[local.editors[0],remote.editors[1],remote.editors[2]]);
  assert.equal(result.workspace.activeEditor,'a');
  assert.equal(JSON.stringify([base,local,remote]),inputs,'pure recovery must not mutate any input');
  result.workspace.editors[0].draft.headers[0].value='later edit';
  assert.equal(local.editors[0].draft.headers[0].value,'first','results must be independent snapshots');
}
{
  const original=editor('a'),local=changed(original,'local'),remote=changed(original,'remote');
  const ids=['a','a','recovered-a'];
  const result=reconcileWorkspace(snapshot([original]),snapshot([local]),snapshot([remote]),()=>ids.shift());
  assert.equal(result.copied,1);assert.equal(result.workspace.activeEditor,'recovered-a');
  assert.deepEqual(result.workspace.editors,[remote,{...local,id:'recovered-a'}]);
  assert.deepEqual(result.workspace.editors[1].draft.headers,local.draft.headers);
  assert.equal(result.workspace.editors[1].grpcDraft.json,'{"unfinished":');
  assert.equal(result.workspace.editors[1].savedRequestId,'saved-a');
  const unchanged=reconcileWorkspace(snapshot([original]),snapshot([original]),snapshot([remote]));
  assert.deepEqual(unchanged.workspace.editors,[remote]);assert.equal(unchanged.copied,0);
  const sameChange=reconcileWorkspace(snapshot([original]),snapshot([local]),snapshot([local]));
  assert.equal(sameChange.copied,0);assert.equal(sameChange.workspace.editors.length,1);
}
{
  const original=editor('a'),local=changed(original,'local');
  const restored=reconcileWorkspace(snapshot([original]),snapshot([local]),snapshot([]),()=> 'new-a');
  assert.deepEqual(restored.workspace.editors,[{...local,id:'new-a'}]);
  assert.equal(restored.workspace.activeEditor,'new-a');
  const deleted=reconcileWorkspace(snapshot([original]),snapshot([original]),snapshot([]));
  assert.deepEqual(deleted.workspace,{editors:[],activeEditor:''});
  const closed=reconcileWorkspace(snapshot([original]),snapshot([]),snapshot([original]));
  assert.deepEqual(closed.workspace.editors,[original],'explicit recovery preserves every remote tab, including locally closed tabs');
  const added=reconcileWorkspace(snapshot([]),snapshot([local]),snapshot([editor('remote')]));
  assert.equal(added.added,1);assert.deepEqual(added.workspace.editors,[editor('remote'),local]);
  const collision=reconcileWorkspace(snapshot([]),snapshot([local]),snapshot([original]),()=> 'local-copy');
  assert.equal(collision.copied,1);assert.equal(collision.workspace.editors[1].id,'local-copy');
}
{
  const original=editor('a');
  const reordered={grpcDraft:original.grpcDraft,parentId:null,selected:null,draft:{tls:original.draft.tls,bodyBase64:original.draft.bodyBase64,headers:original.draft.headers,url:original.draft.url,method:'POST',engine:'auto'},id:'a',savedRequestId:'saved-a',extra:undefined};
  assert.equal(reconcileWorkspace(snapshot([original]),snapshot([reordered]),snapshot([changed(original,'remote')])).copied,0);
  const headers=structuredClone(original);headers.draft.headers.reverse();
  assert.equal(reconcileWorkspace(snapshot([original]),snapshot([headers]),snapshot([changed(original,'remote')]),()=> 'copy').copied,1,'header order is a real draft edit');
  const full=snapshot(Array.from({length:100},(_,i)=>editor('r'+i)));
  assert.throws(()=>reconcileWorkspace(snapshot([]),snapshot([editor('new')]),full),/100/);
  assert.equal(full.editors.length,100,'overflow does not truncate remote tabs');
  assert.throws(()=>reconcileWorkspace(snapshot([original]),snapshot([changed(original,'local')]),snapshot([changed(original,'remote')]),()=> 'a'),/ID/);
  assert(isWorkspaceConflict('HTTP 400: 请求工作区已在其他窗口修改；请保留当前内容后刷新'));
  assert(isWorkspaceConflict('Workspace changed in another window'));
  assert(!isWorkspaceConflict('connection refused'));
}

// A minimal deterministic hook host exercises the real async hook and refs;
// effects/timers are controlled so no DOM, UI sleeps or external service is used.
function hookHost(invoke){
  const slots=[],timers=new Map(),listeners=new Map();let index=0,dirty=true,value,layout=[],effects=[],nextTimer=0;
  const sameDeps=(a,b)=>a&&b&&a.length===b.length&&a.every((v,i)=>Object.is(v,b[i]));
  const react={
    useRef(initial){const i=index++;return slots[i]??=( {current:initial} )},
    useState(initial){const i=index++;if(!slots[i])slots[i]={value:typeof initial==='function'?initial():initial};return [slots[i].value,next=>{const result=typeof next==='function'?next(slots[i].value):next;if(!Object.is(result,slots[i].value)){slots[i].value=result;dirty=true}}]},
    useCallback(fn,deps){const i=index++;if(!slots[i]||!sameDeps(slots[i].deps,deps))slots[i]={value:fn,deps};return slots[i].value},
    useEffect(fn,deps){effect(fn,deps,effects)},useLayoutEffect(fn,deps){effect(fn,deps,layout)},
  };
  function effect(fn,deps,queue){const i=index++;if(!slots[i]||!sameDeps(slots[i].deps,deps)){const previous=slots[i];slots[i]={deps,cleanup:previous?.cleanup};queue.push(()=>{slots[i].cleanup?.();slots[i].cleanup=fn()})}}
  const hooks=load('apps/frontend/src/useRequestWorkspace.ts',['react','./api'],name=>name==='react'?react:name==='./api'?{invoke}:require(name),{setTimeout(fn){const id=++nextTimer;timers.set(id,fn);return id},clearTimeout(id){timers.delete(id)}},{addEventListener(name,fn){listeners.set(name,fn)},removeEventListener(name){listeners.delete(name)}});
  function render(){let rounds=0;do{assert(++rounds<30,'unexpected render loop');dirty=false;index=0;layout=[];effects=[];value=hooks.useRequestWorkspace();for(const run of layout)run();for(const run of effects)run()}while(dirty);return value}
  return {render,get current(){return value},async settle(){for(let i=0;i<8;i++){await Promise.resolve();if(dirty)render()}return value},dispose(){for(const slot of slots)slot?.cleanup?.();timers.clear()},beforeUnload(){let prevented=false;listeners.get('beforeunload')?.({preventDefault(){prevented=true},returnValue:undefined});return prevented}};
}
function deferred(){let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return {promise,resolve,reject}}

(async()=>{
  const base=snapshot([editor('a')]);
  const reads=[],writes=[];let read=async()=>({...structuredClone(base),revision:1});let write=async()=>{throw Error('请求工作区已在其他窗口修改')};
  const host=hookHost(async(command,args)=>{if(command==='request_workspace'){reads.push(command);return read()}if(command==='save_request_workspace'){writes.push(structuredClone(args.workspace));return write(args.workspace)}throw Error('unexpected command')});
  host.render();await host.settle();assert(host.current.ready);
  host.current.setEditors(items=>items.map(item=>changed(item,'local-1')));host.render();
  assert.equal(await host.current.retry(),false);await host.settle();assert(host.current.conflict);assert(host.beforeUnload());
  const beforeRetry=writes.length;assert.equal(await host.current.retry(),false);assert.equal(writes.length,beforeRetry,'blocked CAS saves cannot force-overwrite the remote revision');
  const remoteRead=deferred(),firstWrite=deferred();read=()=>remoteRead.promise;let savedRevision=7;
  write=async(workspace)=>{assert.equal(workspace.revision,savedRevision);if(savedRevision===7){await firstWrite.promise}return ++savedRevision};
  const recovering=host.current.recover();assert.equal(recovering,host.current.recover(),'duplicate recovery shares one operation');assert.equal(recovering,host.current.retry(),'close/save waits for recovery');
  // New local input lands while the latest remote workspace is still loading.
  host.current.setEditors(items=>items.map(item=>changed(item,'local-during-read')));host.render();
  remoteRead.resolve({...snapshot([changed(base.editors[0],'remote-a'),editor('remote-new')],'remote-new'),revision:7});
  await host.settle();assert(host.current.recovering);assert.equal(writes.at(-1).editors.length,3);
  const copy=host.current.editors.find(item=>item.id!=='a'&&item.id!=='remote-new');assert(copy);assert.equal(copy.draft.url,'https://example.invalid/local-during-read');assert.equal(host.current.activeEditor,copy.id);
  // Apply the setter without committing a React render before the RPC settles.
  // The hook must still see this input through its synchronous local snapshot.
  host.current.setEditors(items=>items.map(item=>item.id===copy.id?changed(item,'local-during-save'):item));
  firstWrite.resolve();assert.equal(await recovering,true);await host.settle();
  assert.equal(savedRevision,9);assert.equal(writes.at(-1).editors.find(item=>item.id===copy.id).draft.url,'https://example.invalid/local-during-save');
  assert.equal(host.current.editors.find(item=>item.id==='a').draft.url,'https://example.invalid/remote-a');
  assert(!host.current.conflict&&!host.current.error&&!host.current.recovering);assert(!host.beforeUnload());

  // An unavailable reload keeps the original local draft and conflict guard.
  host.current.setEditors(items=>items.map(item=>item.id===copy.id?changed(item,'keep-on-failure'):item));host.render();write=async()=>{throw Error('请求工作区已在其他窗口修改')};
  await host.current.retry();await host.settle();const before=JSON.stringify(host.current.editors);
  read=async()=>{throw Error('service offline')};assert.equal(await host.current.recover(),false);await host.settle();
  assert.equal(JSON.stringify(host.current.editors),before);assert(host.current.conflict&&host.current.error);assert(host.beforeUnload());

  // A second writer racing the recovery CAS cannot be overwritten either.
  read=async()=>({...snapshot([changed(base.editors[0],'new-remote'),editor('remote-new')]),revision:12});
  assert.equal(await host.current.recover(),false);await host.settle();assert.equal(writes.at(-1).revision,12);assert(host.current.conflict);
  assert(host.current.editors.some(item=>item.draft.url==='https://example.invalid/keep-on-failure'));
  assert(host.current.editors.some(item=>item.draft.url==='https://example.invalid/new-remote'));
  assert(host.beforeUnload());host.dispose();
  console.log('Workspace recovery: three-way edits, conflict copies, remote deletions/additions, ordered duplicate headers, gRPC bytes, ID collisions, limits, CAS races and typing during read/save passed.');
})().catch(error=>{console.error(error);process.exitCode=1});
