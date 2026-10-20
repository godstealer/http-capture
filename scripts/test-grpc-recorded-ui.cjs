const assert=require('node:assert/strict');
const {build}=require('esbuild');
let current;
const react={
  useState(initial){const h=current,i=h.index++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return[h.slots[i],value=>{const next=typeof value==='function'?value(h.slots[i]):value;if(!Object.is(next,h.slots[i])){h.slots[i]=next;h.changed=true;}}]},
  useRef(initial){const h=current,i=h.index++;return h.slots[i]??={current:initial}},
  useMemo(calculate,deps){const h=current,i=h.index++,old=h.slots[i];if(!old||deps.some((value,j)=>value!==old.deps[j]))h.slots[i]={value:calculate(),deps};return h.slots[i].value},
  useEffect(effect,deps){const h=current,i=h.index++,old=h.slots[i];if(!old||deps.some((value,j)=>value!==old[j])){h.effects.push(()=>{h.cleanups[i]?.();h.cleanups[i]=effect()});h.slots[i]=deps}},
  lazy(){return function BodyCodeEditor(){}},Suspense:function Suspense(){},
};
const jsx=(type,props={})=>({type,props});
function text(node){if(Array.isArray(node))return node.map(text).join('');if(node===null||node===undefined||node===false)return '';return typeof node==='object'?text(node.props?.children):String(node)}
function nodes(node){if(Array.isArray(node))return node.flatMap(nodes);return node&&typeof node==='object'?[node,...nodes(node.props?.children)]:[]}
const storage=new Map(),listeners=new Map();
global.localStorage={getItem:key=>storage.get(key)??null,setItem:(key,value)=>storage.set(key,String(value))};
global.window={addEventListener(name,fn){const set=listeners.get(name)??new Set();set.add(fn);listeners.set(name,set)},removeEventListener(name,fn){listeners.get(name)?.delete(fn)},dispatchEvent(event){listeners.get(event.type)?.forEach(fn=>fn(event));return true}};
const method={path:'/test.Echo/Call',requestStream:true,responseStream:true,resolve(){},resolvedRequestType:{create:()=>({}),toObject:value=>value}};
global.__grpcMethod=method;
let invokeWork,decodeWork,reflectWork,calls=[],decodes=[],reflections=[];
function reset(){calls=[];decodes=[];reflections=[];invokeWork=async command=>{throw Error('Unexpected command '+command)};decodeWork=async body=>[{message:Buffer.from(body,'base64').toString('utf8')}];reflectWork=async()=> 'syntax="proto3";';global.__grpcMethod=method;storage.clear();}
global.__grpcInvoke=(command,args)=>{calls.push([command,args]);return invokeWork(command,args)};
global.__grpcDecode=(...args)=>{assert.equal(typeof args[0],'string','decode requires a complete body, never absent metadata');decodes.push(args);return decodeWork(...args)};
global.__grpcReflect=request=>{assert.equal(typeof request.bodyBase64,'string','reflection receives a complete request');reflections.push(request);return reflectWork(request)};
async function load(){
  const output=await build({stdin:{contents:"export {default as GrpcPanel} from './GrpcPanel';export {default as GrpcResponse} from './GrpcResponse';export {useRecordedFlow,smallRecordedFlow} from './useRecordedFlow';",resolveDir:'apps/frontend/src',sourcefile:'grpc-recorded-tests.ts'},bundle:true,platform:'node',format:'cjs',write:false,packages:'external',jsx:'automatic',plugins:[{name:'grpc-recorded-ui',setup(builder){
    builder.onLoad({filter:/[/\\]api\.ts$/},()=>({contents:'export const invoke=(...args)=>globalThis.__grpcInvoke(...args);'}));
    builder.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export const t=key=>key;'}));
    builder.onLoad({filter:/[/\\]BodyCodeEditor\.tsx$/},()=>({contents:'export default function BodyCodeEditor(){}'}));
    builder.onLoad({filter:/[/\\]grpcReflection\.ts$/},()=>({contents:'export const reflect=request=>globalThis.__grpcReflect(request);'}));
    builder.onLoad({filter:/[/\\]formatProto\.ts$/},()=>({contents:'export const formatProto=text=>text;'}));
    builder.onLoad({filter:/[/\\]grpc\.ts$/},()=>({contents:`
      export const schema=()=>({methods:[globalThis.__grpcMethod]});
      export const methodPath=method=>method.path;
      export const decodeMessages=(...args)=>globalThis.__grpcDecode(...args);
      export const grpcStatus=()=>({code:0,name:'OK',message:''});
      export const prepareGrpc=(request,method,json)=>({...request,bodyBase64:Buffer.from(json).toString('base64')});
      export const encodeMessage=(method,json)=>Buffer.from(json);
      export const base64=value=>Buffer.from(value).toString('base64');
    `}));
  }}]});
  const module={exports:{}};new Function('module','exports','require',output.outputFiles[0].text)(module,module.exports,name=>name==='react'?react:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:require(name));return module.exports;
}
function harness(Component,props){
  const h={slots:[],index:0,effects:[],cleanups:[],props,tree:null,changed:false};
  h.render=()=>{let limit=0;do{assert(limit++<40,'effects converge');h.changed=false;current=h;h.index=0;h.tree=Component(h.props);h.effects.splice(0).forEach(effect=>effect())}while(h.changed);return h.tree};
  h.all=predicate=>nodes(h.tree).filter(predicate);h.find=predicate=>nodes(h.tree).find(predicate);
  h.button=label=>h.find(node=>node.type==='button'&&text(node)===label);
  h.click=label=>{const button=h.button(label);assert(button,label);assert(!button.props.disabled,label+' must be enabled');const result=button.props.onClick();h.render();return result};
  h.tick=async()=>{await new Promise(resolve=>setImmediate(resolve));h.render()};
  h.update=patch=>{h.props={...h.props,...patch};h.render()};
  h.dispose=()=>h.cleanups.forEach(cleanup=>cleanup?.());h.render();return h;
}
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
const encoded=text=>Buffer.from(text).toString('base64');
function fullFlow(id='flow',lengths={request:4,response:5}){
  const request=(variant,length)=>({method:'POST',url:'https://grpc.test/test.Echo/Call',headers:[{name:'x-entity',value:variant}],bodyBase64:encoded(variant==='request'?'Q'.repeat(length):'O'.repeat(length)),tls:{preset:'native'}});
  const response=(variant,length)=>({status:200,version:'HTTP/2',headers:[{name:'content-type',value:'application/grpc'},{name:'x-entity',value:variant}],trailers:[{name:'grpc-status',value:'0'}],bodyBase64:encoded('R'.repeat(length))});
  const result={id,parentId:null,startedAt:1,durationMs:1,source:'capture',state:'completed',request:request('request',lengths.request??0),response:lengths.response===undefined?null:response('response',lengths.response),error:null,notes:[]};
  if(lengths.originalRequest!==undefined)result.originalRequest=request('originalRequest',lengths.originalRequest);
  if(lengths.originalResponse!==undefined)result.originalResponse=response('originalResponse',lengths.originalResponse);
  return result;
}
function metadata(full,revision='instance:1'){
  const result=structuredClone(full);result.bodyRevision=revision;result.bodyReferences={};
  for(const variant of ['request','response','originalRequest','originalResponse'])if(result[variant]){result.bodyReferences[variant]={byteLength:Buffer.from(result[variant].bodyBase64,'base64').length};delete result[variant].bodyBase64;}
  return result;
}
const getCalls=()=>calls.filter(([command])=>command==='get_flow');
const messageNodes=h=>h.all(node=>node.props?.className==='grpc-message');
const loadButton='加载完整 gRPC 正文';
(async()=>{
  const {GrpcPanel,GrpcResponse,useRecordedFlow,smallRecordedFlow}=await load();
  reset();
  assert(smallRecordedFlow(metadata(fullFlow('empty',{request:0,response:0,originalRequest:0,originalResponse:0}))),'real empty bodies count as zero');
  assert(smallRecordedFlow(metadata(fullFlow('boundary',{request:32768,response:32768}))),'64 KiB total is eligible');
  assert(!smallRecordedFlow(metadata(fullFlow('four',{request:20000,response:20000,originalRequest:20000,originalResponse:20000}))),'all four bodies share the budget');
  const invalid=metadata(fullFlow());delete invalid.bodyReferences.request;assert(!smallRecordedFlow(invalid),'missing references never become an empty body');
  function Hook(props){return useRecordedFlow(props.flow,props.visible,props.connected)}
  const a=fullFlow('a'),am=metadata(a),readA=deferred();invokeWork=()=>readA.promise;
  const hook=harness(Hook,{flow:am,visible:false,connected:true});assert.equal(calls.length,0);assert.equal(hook.tree.flow,null);
  hook.update({visible:true});assert.deepEqual(getCalls(),[['get_flow',{id:'a',expectedVersion:'instance:1'}]]);
  hook.update({visible:false});readA.resolve(a);await hook.tick();assert.equal(hook.tree.flow,null,'hidden result cannot be retained');
  const reread=deferred();invokeWork=()=>reread.promise;hook.update({visible:true});assert.equal(getCalls().length,2,'reopen reloads even the same ID and version');
  hook.update({connected:false});reread.resolve(a);await hook.tick();assert.equal(hook.tree.flow,null,'disconnect invalidates an in-flight full snapshot');
  const b=fullFlow('b'),bm=metadata(b,'instance:2'),readB=deferred();invokeWork=()=>readB.promise;hook.update({flow:bm,connected:true});
  const c=fullFlow('b',{request:8,response:9}),cm=metadata(c,'instance:3');invokeWork=async()=>c;hook.update({flow:cm});await hook.tick();assert.equal(hook.tree.flow,c);
  readB.resolve(b);await hook.tick();assert.equal(hook.tree.flow,c,'an older revision cannot overwrite the latest snapshot');
  hook.update({visible:false});assert.equal(hook.tree.flow,null);assert(!hook.slots.some(value=>value?.flow===c),'hidden state releases full bodies');hook.dispose();

  reset();const small=fullFlow('small'),sm=metadata(small);invokeWork=async()=>small;
  const response=harness(GrpcResponse,{flow:sm,visible:false,connected:true});await response.tick();assert.equal(calls.length,0);assert.equal(decodes.length,0);
  response.update({visible:true});await response.tick();await response.tick();
  assert.deepEqual(getCalls(),[['get_flow',{id:'small',expectedVersion:'instance:1'}]]);assert.equal(decodes.length,1);assert.equal(decodes[0][0],small.response.bodyBase64);assert.equal(messageNodes(response).length,1);
  assert(calls.every(([command])=>command==='get_flow'),'viewing a record never starts or reflects an RPC');
  response.update({visible:false});assert.equal(response.tree,null);assert(!response.slots.some(value=>value?.source===small||value?.flow===small));response.dispose();

  reset();const big=fullFlow('big',{request:50000,response:50000}),bigMeta=metadata(big);const bigRead=deferred();invokeWork=()=>bigRead.promise;
  const large=harness(GrpcResponse,{flow:bigMeta,connected:true});assert.equal(calls.length,0);assert.equal(decodes.length,0);assert(text(large.tree).includes('读入内存'));
  const button=large.button(loadButton);assert(button);button.props.onClick();button.props.onClick();large.render();assert.equal(getCalls().length,1,'duplicate explicit clicks share one pending load');
  bigRead.resolve(big);await large.tick();await large.tick();assert.equal(decodes.length,1);assert.equal(decodes[0][0],big.response.bodyBase64,'the complete body reaches the decoder, never a prefix');
  large.update({connected:false});assert.equal(messageNodes(large).length,0);assert(large.button(loadButton).props.disabled);assert(!large.slots.some(value=>value?.flow===big||value?.source===big));large.dispose();

  reset();const same=fullFlow('same'),sameMeta=metadata(same);invokeWork=async()=>same;
  const firstDecode=deferred();decodeWork=()=>firstDecode.promise;
  const stale=harness(GrpcResponse,{flow:sameMeta});await stale.tick();assert.equal(decodes.length,1);
  stale.update({visible:false});firstDecode.resolve([{sentinel:'old hidden message'}]);await stale.tick();assert(!text(stale.tree).includes('old hidden message'));
  const secondDecode=deferred();decodeWork=()=>secondDecode.promise;stale.update({visible:true});await stale.tick();assert.equal(decodes.length,2);
  global.__grpcMethod={...method};decodeWork=async()=>[{sentinel:'new schema message'}];window.dispatchEvent(new Event('grpc-schema-changed'));stale.render();await stale.tick();assert(text(stale.tree).includes('new schema message'));
  secondDecode.reject(Error('old schema failure'));await stale.tick();assert(!text(stale.tree).includes('old schema failure'));assert(text(stale.tree).includes('new schema message'));stale.dispose();

  reset();const broken=metadata(fullFlow('broken'));invokeWork=async()=>{throw Error('Recorded flow version changed')};
  const failed=harness(GrpcResponse,{flow:broken});await failed.tick();assert(text(failed.tree).includes('Recorded flow version changed'));assert.equal(decodes.length,0);failed.render();await failed.tick();assert.equal(getCalls().length,1,'failed automatic loading never loops or silently uses an older body');
  const retry=fullFlow('broken');invokeWork=async()=>retry;failed.click(loadButton);await failed.tick();await failed.tick();assert.equal(decodes.length,1);failed.dispose();

  reset();const captured=fullFlow('captured',{request:6,response:7,originalRequest:9});const captureMeta=metadata(captured);invokeWork=async()=>captured;
  const panel=harness(GrpcPanel,{flow:captureMeta,draft:captureMeta.originalRequest,visible:false});assert(panel.tree.props.hidden);assert.equal(calls.length,0);assert.equal(decodes.length,0);
  panel.update({visible:true});await panel.tick();await panel.tick();assert.equal(getCalls().length,1);assert.equal(decodes.length,1);assert.equal(reflections.length,0,'record selection does not invoke reflection');
  await panel.click('服务反射');await panel.tick();assert.equal(reflections.length,1);assert.equal(reflections[0].bodyBase64,captured.originalRequest.bodyBase64);assert.equal(reflections[0].headers[0].value,'originalRequest','reflection uses the complete selected original request');
  const stableReflection=deferred();reflectWork=()=>stableReflection.promise;const stableAction=panel.click('服务反射');await new Promise(resolve=>setImmediate(resolve));panel.update({environment:{}});stableReflection.resolve('same environment proto');await stableAction;panel.render();assert(storage.get('http-capture.grpc.proto.v1').includes('same environment proto'),'equivalent environment objects from parent polling do not invalidate a user action');
  const reflection=deferred();reflectWork=()=>reflection.promise;const reflectionAction=panel.click('服务反射');await new Promise(resolve=>setImmediate(resolve));panel.update({visible:false});reflection.resolve('late proto definition');await reflectionAction;await panel.tick();assert(!storage.get('http-capture.grpc.proto.v1')?.includes('late proto definition'),'hidden reflection results cannot update saved schema');assert.equal(calls.filter(([command])=>command==='cancel_replay').length,0);panel.dispose();

  reset();const requestFull=fullFlow('request',{request:15,response:3});const requestMeta=metadata(requestFull);const requestChanges=[];invokeWork=async()=>requestFull;decodeWork=async(body,_method,_headers,isRequest)=>[{value:isRequest?'complete request':'response'}];
  const requestPanel=harness(GrpcPanel,{flow:requestMeta,draft:requestMeta.request,onChange:()=>{},onDraftChange:patch=>requestChanges.push(patch)});await requestPanel.tick();assert.equal(decodes.length,0,'composer panel leaves response decoding to its visible response component');
  await requestPanel.click('载入请求消息');requestPanel.render();assert.equal(decodes[0][0],requestFull.request.bodyBase64);assert.equal(decodes[0][3],true);assert(requestChanges.some(patch=>patch.json?.includes('complete request')));
  const lateRequest=deferred();decodeWork=()=>lateRequest.promise;const loadRequest=requestPanel.click('载入请求消息');await new Promise(resolve=>setImmediate(resolve));requestPanel.update({visible:false});const changesBefore=requestChanges.length;lateRequest.resolve([{value:'late request'}]);await loadRequest;requestPanel.render();assert.equal(requestChanges.length,changesBefore,'hidden request decoding cannot overwrite draft JSON');requestPanel.dispose();

  reset();const bigCapture=fullFlow('bigCapture',{request:40000,response:40000}),bigCaptureMeta=metadata(bigCapture);invokeWork=async()=>bigCapture;
  const bigPanel=harness(GrpcPanel,{flow:bigCaptureMeta,draft:bigCaptureMeta.request});assert.equal(calls.length,0);assert(bigPanel.button('服务反射').props.disabled,'large metadata requires the explicit memory choice before reflection');
  bigPanel.click(loadButton);await bigPanel.tick();await bigPanel.tick();assert.equal(getCalls().length,1);await bigPanel.click('服务反射');assert.equal(reflections[0].bodyBase64,bigCapture.request.bodyBase64);bigPanel.dispose();

  reset();const composerRequest=fullFlow('composer').request;const started=[];invokeWork=async(command)=>{if(command==='grpc_start')return 'session-1';if(command==='grpc_send'||command==='cancel_replay')return null;throw Error(command)};
  const composer=harness(GrpcPanel,{draft:composerRequest,onChange:()=>{},onStarted:id=>started.push(id)});assert.equal(calls.length,0,'full composer does not hydrate or send until explicit action');
  await composer.click('开始 gRPC 调用');composer.render();assert.deepEqual(started,['session-1']);assert.equal(calls.filter(([command])=>command==='grpc_start').length,1);
  await composer.click('发送消息');composer.render();await composer.click('结束请求流');composer.render();assert.equal(calls.filter(([command])=>command==='grpc_send').length,2);
  composer.update({visible:false});assert.equal(calls.filter(([command])=>command==='cancel_replay').length,0,'hiding a mounted panel preserves the active session');
  composer.update({visible:true});assert.equal(calls.filter(([command])=>command==='grpc_start').length,1,'reopening never restarts an RPC');composer.dispose();assert.equal(calls.filter(([command])=>command==='cancel_replay').length,1,'actual unmount still cancels the owned session once');
  reset();const starting=deferred();invokeWork=async(command)=>command==='grpc_start'?starting.promise:null;
  const unmounted=harness(GrpcPanel,{draft:composerRequest,onChange:()=>{}});const startWork=unmounted.click('开始 gRPC 调用');await new Promise(resolve=>setImmediate(resolve));unmounted.dispose();starting.resolve('late-session');await startWork;assert.deepEqual(calls.filter(([command])=>command==='cancel_replay'),[['cancel_replay',{executionId:'late-session'}]],'an RPC created after actual unmount is cancelled too');
  console.log('Recorded gRPC UI: four-body budget, explicit complete loading, pinned versions, hidden/disconnected cleanup, stale hydration/decode/reflection guards and session preservation passed.');
})().catch(error=>{console.error(error);process.exitCode=1});
