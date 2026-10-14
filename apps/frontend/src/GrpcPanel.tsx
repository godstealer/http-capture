import type {GrpcDraft} from './useRequestWorkspace';
import {grpcScope,resolveGrpcJson,resolveGrpcEndpoint} from './grpcEnvironment';
import {formatProto} from './formatProto';
import {reflect} from './grpcReflection';
import {invoke} from './api';
import {useEffect,useMemo,useState,useRef,lazy,Suspense} from 'react';
import {schema,methodPath,prepareGrpc,decodeMessages,grpcStatus,encodeMessage,base64} from './grpc';
import type {Flow,RequestDraft} from './types';
const BodyCodeEditor=lazy(()=>import('./BodyCodeEditor')); 
import {t} from './i18n';
const key='http-capture.grpc.proto.v1';
export default function GrpcPanel({savedDraft,onDraftChange,draft,flow,onChange,onStarted,flows=[],environment={},environmentValid=true}:{savedDraft?:GrpcDraft;onDraftChange?:(patch:Partial<GrpcDraft>)=>void;environment?:Record<string,string>;environmentValid?:boolean;onStarted?:(id:string)=>void;flows?:Flow[];draft:RequestDraft;flow?:Flow|null;onChange?:(draft:RequestDraft)=>void}){
 const [sources,setSources]=useState<string[]>(()=>{try{return JSON.parse(localStorage.getItem(key)||'[]')}catch{return []}});
 const [proto,setProto]=useState(()=>{const text=sources.join('\n');try{return formatProto(text)}catch{return text}});
 const [session,setSession]=useState('');const [sending,setSending]=useState(false);const [ended,setEnded]=useState(false);
 const sessionScope=useRef<Record<string,string>>({});
 function resolvedEndpoint(){if(!environmentValid)throw Error(t('请选择环境'));return resolveGrpcEndpoint(draft,environment)}
 const sessionRef=useRef('');const mounted=useRef(true);sessionRef.current=session;
 useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;if(sessionRef.current)void invoke('cancel_replay',{executionId:sessionRef.current}).catch(()=>{})}},[]);
 const live=flows.find(f=>f.id===session);const active=!!session&&(!live||['sending','receiving'].includes(live.state??''));
 const [selected,updateSelected]=useState(savedDraft?.method??'');const [json,updateJson]=useState(savedDraft?.json??'{}');
 function setSelected(value:string){updateSelected(value);onDraftChange?.({method:value})}
 function setJson(value:string){updateJson(value);onDraftChange?.({json:value})}
const [notice,setNotice]=useState('');const [decoded,setDecoded]=useState<unknown[]>([]);const [decodeError,setDecodeError]=useState('');
 const compiled=useMemo(()=>{try{return {...schema(sources),error:''}}catch(e){return {methods:[],error:String(e)}}},[sources]);
 let path='';try{path=new URL(draft.url).pathname}catch{}
 const method=compiled.methods.find(m=>methodPath(m)===(selected||path));
 const response=(live??flow)?.response;const status=response?grpcStatus(response.headers,response.trailers):undefined;
 function apply(texts:string[]){try{schema(texts);localStorage.setItem(key,JSON.stringify(texts));window.dispatchEvent(new Event('grpc-schema-changed'));setSources(texts);setProto(formatProto(texts.join('\n')));setNotice(t('已保存'));}catch(e){setNotice(String(e))}}
 useEffect(()=>{let current=true;setDecodeError('');if(response)decodeMessages(response.bodyBase64,method,response.headers,false,active).then(v=>{if(current)setDecoded(v)}).catch(e=>{if(current)setDecodeError(String(e))});else setDecoded([]);return()=>{current=false}},[response,method,active]);

 return <div className="grpc-panel request-details">
 <details open><summary>{t('Protobuf 定义')}</summary><p>{t('导入主 proto 及依赖文件，或粘贴定义。定义保存在本机；无需安装 protoc。')}</p>
 <button disabled={active||sending} onClick={async()=>{setSending(true);setNotice(t('正在获取服务反射…'));try{apply([await reflect(resolvedEndpoint())])}catch(e){setNotice(String(e))}finally{setSending(false)}}}>{t('服务反射')}</button>
 <input disabled={active||sending} type="file" multiple accept=".proto" aria-label={t('导入 proto 文件')} onChange={async e=>{const files=[...(e.target.files??[])];try{apply(await Promise.all(files.map(f=>f.text())))}catch(error){setNotice(String(error))}e.target.value=''}}/>
 <div className="editor-toolbar"><button className="button" disabled={active||sending} onClick={()=>{try{schema([proto]);setProto(formatProto(proto));setNotice('')}catch(e){setNotice(String(e))}}}>{t('格式化')}</button></div>
 <div style={{height:320,overflow:'auto'}}><Suspense fallback={<p>…</p>}><BodyCodeEditor label={t('Protobuf 定义')} disabled={active||sending} value={proto} parser={proto.trim().startsWith('{')?'json':'proto'} onChange={setProto}/></Suspense></div>
 <button disabled={active||sending} className="button" onClick={()=>apply([proto])}>{t('保存定义')}</button></details>
 <label>{t('服务 / 方法')} <select aria-label={t('gRPC 方法')} disabled={active||sending} value={method?methodPath(method):''} onChange={e=>setSelected(e.target.value)}><option value="">{t('选择方法')}</option>{compiled.methods.map(m=><option key={methodPath(m)} value={methodPath(m)}>{methodPath(m)} · {m.requestStream?(m.responseStream?'Bidi':'Client stream'):(m.responseStream?'Server stream':'Unary')}</option>)}</select></label>
 {onChange&&<><div className="editor-toolbar"><button onClick={()=>{if(method){method.resolve();setJson(JSON.stringify(method.resolvedRequestType!.toObject(method.resolvedRequestType!.create(),{defaults:true,longs:String,enums:String,bytes:String}),null,2))}}} disabled={!method}>{t('生成示例')}</button>
 <button onClick={async()=>{try{const values=await decodeMessages(draft.bodyBase64,method,draft.headers,true);setJson(JSON.stringify(values[0]??{},null,2))}catch(e){setNotice(String(e))}}}>{t('载入请求消息')}</button>
 <button disabled={!method||active||sending} onClick={async()=>{setSending(true);try{const endpoint=resolvedEndpoint();const scope=grpcScope(draft,environment);const request=prepareGrpc(endpoint,method!,resolveGrpcJson(json,scope));sessionScope.current=scope;setEnded(!method!.requestStream);const id=await invoke<string>('grpc_start',{request,halfClose:!method!.requestStream});if(!mounted.current){await invoke('cancel_replay',{executionId:id});return;}sessionRef.current=id;setSession(id);onStarted?.(id);setNotice('');}catch(e){setNotice(String(e))}finally{setSending(false)}}}>{t('开始 gRPC 调用')}</button>
 <button disabled={!active||ended||sending||!method} onClick={async()=>{setSending(true);try{await invoke('grpc_send',{id:session,body:base64(encodeMessage(method!,resolveGrpcJson(json,sessionScope.current))),end:false});}catch(e){setNotice(String(e))}finally{setSending(false)}}}>{t('发送消息')}</button>
 <button disabled={!active||ended||sending} onClick={async()=>{setSending(true);try{await invoke('grpc_send',{id:session,body:'',end:true});setEnded(true)}catch(e){setNotice(String(e))}finally{setSending(false)}}}>{t('结束请求流')}</button>
 <button disabled={!active} onClick={()=>void invoke('cancel_replay',{executionId:session}).catch(e=>setNotice(String(e)))}>{t('取消调用')}</button></div>
 <p role="status">{live?.error??(active?t('gRPC 调用中…'):live?.state??'')}</p>
 <Suspense fallback={<p>…</p>}><BodyCodeEditor value={json} parser="json" onChange={setJson}/></Suspense></>}
 {compiled.error&&<p role="alert">{compiled.error}</p>}<p role="status">{notice}</p>

 {status&&!onChange&&<><strong>gRPC {status.code??'—'} · {active&&status.code===undefined?t('接收中…'):status.name}</strong><p>{status.message}</p><details><summary>Metadata</summary><pre>{response!.headers.map(h=>h.name+': '+h.value).join('\n')}</pre></details><details open><summary>Trailers</summary><pre>{(response!.trailers??[]).map(h=>h.name+': '+h.value).join('\n')||'—'}</pre></details>{decodeError&&<p role="alert">{decodeError}</p>}{decoded.map((value,index)=><details className="grpc-message" key={index}><summary>↓ #{index+1} · {JSON.stringify(value).slice(0,140)}</summary><pre>{JSON.stringify(value,null,2)}</pre></details>)}</>}
 {!method&&<p>{t('未选择消息定义时仅按字段编号解析，不推断字段名或业务类型。')}</p>}
 </div>
}
