import type {GrpcDraft} from './useRequestWorkspace';
import {grpcScope,resolveGrpcJson,resolveGrpcEndpoint} from './grpcEnvironment';
import {formatProto} from './formatProto';
import {reflect} from './grpcReflection';
import {invoke} from './api';
import {useEffect,useMemo,useState,useRef,lazy,Suspense} from 'react';
import {schema,methodPath,prepareGrpc,decodeMessages,grpcStatus,encodeMessage,base64} from './grpc';
import type {Flow,FlowRecord,RequestDraft,RequestMetadata} from './types';
import {isFlowMetadata,loadCompleteFlow} from './flowRecords';
import {useRecordedFlow} from './useRecordedFlow';
const BodyCodeEditor=lazy(()=>import('./BodyCodeEditor')); 
import {t} from './i18n';
const key='http-capture.grpc.proto.v1';
const emptyEnvironment:Record<string,string>={};
const sameEnvironment=(a:Record<string,string>,b:Record<string,string>)=>a===b||(Object.keys(a).length===Object.keys(b).length&&Object.keys(a).every(key=>a[key]===b[key]));
export default function GrpcPanel({savedDraft,onDraftChange,draft,flow,onChange,onStarted,flows=[],environment=emptyEnvironment,environmentValid=true,visible=true,connected=true}:{savedDraft?:GrpcDraft;onDraftChange?:(patch:Partial<GrpcDraft>)=>void;environment?:Record<string,string>;environmentValid?:boolean;onStarted?:(id:string)=>void;flows?:FlowRecord[];draft:RequestDraft|RequestMetadata;flow?:FlowRecord|null;onChange?:(draft:RequestDraft)=>void;visible?:boolean;connected?:boolean}){
 const [sources,setSources]=useState<string[]>(()=>{try{return JSON.parse(localStorage.getItem(key)||'[]')}catch{return []}});
 const [proto,setProto]=useState(()=>{const text=sources.join('\n');try{return formatProto(text)}catch{return text}});
 const [session,setSession]=useState('');const [sending,setSending]=useState(false);const [ended,setEnded]=useState(false);
 const sessionScope=useRef<Record<string,string>>({});
 const sessionRef=useRef('');const mounted=useRef(true);sessionRef.current=session;
 useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;if(sessionRef.current)void invoke('cancel_replay',{executionId:sessionRef.current}).catch(()=>{})}},[]);
 const live=flows.find(f=>f.id===session);const active=!!session&&(!live||['sending','receiving'].includes(live.state??''));
 const [selected,updateSelected]=useState(savedDraft?.method??'');const [json,updateJson]=useState(savedDraft?.json??'{}');
 function setSelected(value:string){updateSelected(value);onDraftChange?.({method:value})}
 function setJson(value:string){updateJson(value);onDraftChange?.({json:value})}
 const [notice,setNotice]=useState('');
 const [decoded,setDecoded]=useState<{source:Flow|null;method:unknown;messages:unknown[];error:string}>({source:null,method:null,messages:[],error:''});
 const compiled=useMemo(()=>{try{return {...schema(sources),error:''}}catch(e){return {methods:[],error:String(e)}}},[sources]);
 let path='';try{path=new URL(draft.url).pathname}catch{}
 const method=compiled.methods.find(m=>methodPath(m)===(selected||path));
 const source=live??flow;
 const loaded=useRecordedFlow(source,visible,connected),full=loaded.flow;
 const response=source?.response;const status=response?grpcStatus(response.headers,response.trailers):undefined;
 const requestIsComplete='bodyBase64' in draft&&typeof draft.bodyBase64==='string';
 const requestSlot=source?.request===draft?'request':source?.originalRequest===draft?'originalRequest':null;
 const sourceIdentity=source?(isFlowMetadata(source)?JSON.stringify([source.id,source.bodyRevision]):source):null;
 const draftIdentity=requestIsComplete?draft:requestSlot;
 const actionContext=useRef({sourceIdentity,draftIdentity,visible,connected,method,json,environment,environmentValid});
 const previous=actionContext.current;
 if(previous.sourceIdentity!==sourceIdentity||previous.draftIdentity!==draftIdentity||previous.visible!==visible||previous.connected!==connected||previous.method!==method||previous.json!==json||!sameEnvironment(previous.environment,environment)||previous.environmentValid!==environmentValid){actionContext.current={sourceIdentity,draftIdentity,visible,connected,method,json,environment,environmentValid};}
 const renderedAction=actionContext.current;
 const currentAction=()=>mounted.current&&visible&&connected&&actionContext.current===renderedAction;
 async function completeRequest():Promise<RequestDraft>{
   if(requestIsComplete)return draft as RequestDraft;
   if(!source||!requestSlot)throw Error(t('无法读取此记录的完整请求，请重新选择记录。'));
   if(!full&&!loaded.automatic)throw Error(t('请先加载完整 gRPC 正文。'));
   const complete=full??await loadCompleteFlow(source);
   const request=complete[requestSlot];
   if(!request)throw Error(t('无法读取此记录的完整请求，请重新选择记录。'));
   return request;
 }
 async function resolvedEndpoint(){if(!environmentValid)throw Error(t('请选择环境'));return resolveGrpcEndpoint(await completeRequest(),environment)}
 const needsExplicitLoad=!requestIsComplete&&!full&&!loaded.automatic;
 function apply(texts:string[]){try{schema(texts);localStorage.setItem(key,JSON.stringify(texts));window.dispatchEvent(new Event('grpc-schema-changed'));setSources(texts);setProto(formatProto(texts.join('\n')));setNotice(t('已保存'));}catch(e){setNotice(String(e))}}
 const receiving=active||['sending','receiving'].includes(source?.state??'');
 useEffect(()=>{
   let current=true;
   setDecoded({source:null,method:null,messages:[],error:''});
   if(visible&&connected&&!onChange&&full?.response)decodeMessages(full.response.bodyBase64,method,full.response.headers,false,receiving)
     .then(messages=>{if(current)setDecoded({source:full,method,messages,error:''})})
     .catch(error=>{if(current)setDecoded({source:full,method,messages:[],error:String(error)})});
   return()=>{current=false};
 },[full,method,receiving,visible,connected,onChange]);
 const result=visible&&connected&&decoded.source===full&&decoded.method===method?decoded:null;
 if(!visible)return <div className="grpc-panel request-details" hidden/>;

 return <div className="grpc-panel request-details">
 <details open><summary>{t('Protobuf 定义')}</summary><p>{t('导入主 proto 及依赖文件，或粘贴定义。定义保存在本机；无需安装 protoc。')}</p>
 <button disabled={!connected||active||sending||loaded.loading||needsExplicitLoad} onClick={async()=>{if(!currentAction())return;setSending(true);setNotice(t('正在获取服务反射…'));try{const endpoint=await resolvedEndpoint();if(!currentAction())return;const definition=await reflect(endpoint);if(currentAction())apply([definition]);}catch(e){if(currentAction())setNotice(String(e))}finally{if(mounted.current)setSending(false)}}}>{t('服务反射')}</button>
 <input disabled={active||sending} type="file" multiple accept=".proto" aria-label={t('导入 proto 文件')} onChange={async e=>{const files=[...(e.target.files??[])];try{apply(await Promise.all(files.map(f=>f.text())))}catch(error){setNotice(String(error))}e.target.value=''}}/>
 <div className="editor-toolbar"><button className="button" disabled={active||sending} onClick={()=>{try{schema([proto]);setProto(formatProto(proto));setNotice('')}catch(e){setNotice(String(e))}}}>{t('格式化')}</button></div>
 <div style={{height:320,overflow:'auto'}}><Suspense fallback={<p>…</p>}><BodyCodeEditor label={t('Protobuf 定义')} disabled={active||sending} value={proto} parser={proto.trim().startsWith('{')?'json':'proto'} onChange={setProto}/></Suspense></div>
 <button disabled={active||sending} className="button" onClick={()=>apply([proto])}>{t('保存定义')}</button></details>
 <label>{t('服务 / 方法')} <select aria-label={t('gRPC 方法')} disabled={active||sending} value={method?methodPath(method):''} onChange={e=>setSelected(e.target.value)}><option value="">{t('选择方法')}</option>{compiled.methods.map(m=><option key={methodPath(m)} value={methodPath(m)}>{methodPath(m)} · {m.requestStream?(m.responseStream?'Bidi':'Client stream'):(m.responseStream?'Server stream':'Unary')}</option>)}</select></label>
 {onChange&&<><div className="editor-toolbar"><button onClick={()=>{if(method){method.resolve();setJson(JSON.stringify(method.resolvedRequestType!.toObject(method.resolvedRequestType!.create(),{defaults:true,longs:String,enums:String,bytes:String}),null,2))}}} disabled={!method}>{t('生成示例')}</button>
 <button disabled={!connected||sending||loaded.loading||needsExplicitLoad} onClick={async()=>{if(!currentAction())return;setSending(true);try{const request=await completeRequest();if(!currentAction())return;const values=await decodeMessages(request.bodyBase64,method,request.headers,true);if(currentAction())setJson(JSON.stringify(values[0]??{},null,2));}catch(e){if(currentAction())setNotice(String(e))}finally{if(mounted.current)setSending(false)}}}>{t('载入请求消息')}</button>
 <button disabled={!connected||!method||active||sending||loaded.loading||needsExplicitLoad} onClick={async()=>{if(!currentAction())return;setSending(true);try{const endpoint=await resolvedEndpoint();if(!currentAction())return;const scope=grpcScope(endpoint,environment);const request=prepareGrpc(endpoint,method!,resolveGrpcJson(json,scope));sessionScope.current=scope;setEnded(!method!.requestStream);const id=await invoke<string>('grpc_start',{request,halfClose:!method!.requestStream});if(!mounted.current){await invoke('cancel_replay',{executionId:id});return;}sessionRef.current=id;setSession(id);onStarted?.(id);setNotice('');}catch(e){if(currentAction())setNotice(String(e))}finally{if(mounted.current)setSending(false)}}}>{t('开始 gRPC 调用')}</button>
 <button disabled={!connected||!active||ended||sending||!method} onClick={async()=>{if(!currentAction())return;setSending(true);try{await invoke('grpc_send',{id:session,body:base64(encodeMessage(method!,resolveGrpcJson(json,sessionScope.current))),end:false});}catch(e){if(mounted.current)setNotice(String(e))}finally{if(mounted.current)setSending(false)}}}>{t('发送消息')}</button>
 <button disabled={!connected||!active||ended||sending} onClick={async()=>{if(!currentAction())return;setSending(true);try{await invoke('grpc_send',{id:session,body:'',end:true});if(mounted.current)setEnded(true)}catch(e){if(mounted.current)setNotice(String(e))}finally{if(mounted.current)setSending(false)}}}>{t('结束请求流')}</button>
 <button disabled={!connected||!active} onClick={()=>{if(currentAction())void invoke('cancel_replay',{executionId:session}).catch(e=>{if(mounted.current)setNotice(String(e))})}}>{t('取消调用')}</button></div>
 <p role="status">{live?.error??(active?t('gRPC 调用中…'):live?.state??'')}</p>
 <Suspense fallback={<p>…</p>}><BodyCodeEditor value={json} parser="json" onChange={setJson}/></Suspense></>}
 {compiled.error&&<p role="alert">{compiled.error}</p>}<p role="status">{notice}</p>

 {!connected&&<p role="status">{t('连接内核后才能读取记录的正文。')}</p>}
 {loaded.loading&&<p role="status">{t('正在加载完整 gRPC 正文…')}</p>}
 {source&&!full&&!loaded.loading&&<>{!loaded.automatic&&<p>{t('此操作会将完整请求和响应正文读入内存后解析；大正文可能占用较多内存。')}</p>}<button disabled={!connected} onClick={()=>void loaded.load()}>{t('加载完整 gRPC 正文')}</button></>}
 {loaded.error&&<p role="alert">{loaded.error}</p>}
 {status&&!onChange&&<><strong>gRPC {status.code??'—'} · {receiving&&status.code===undefined?t('接收中…'):status.name}</strong><p>{status.message}</p><details><summary>Metadata</summary><pre>{response!.headers.map(h=>h.name+': '+h.value).join('\n')}</pre></details><details open><summary>Trailers</summary><pre>{(response!.trailers??[]).map(h=>h.name+': '+h.value).join('\n')||'—'}</pre></details>{result?.error&&<p role="alert">{result.error}</p>}{result?.messages.map((value,index)=><details className="grpc-message" key={index}><summary>↓ #{index+1} · {JSON.stringify(value).slice(0,140)}</summary><pre>{JSON.stringify(value,null,2)}</pre></details>)}</>}
 {!method&&<p>{t('未选择消息定义时仅按字段编号解析，不推断字段名或业务类型。')}</p>}
 </div>
}
