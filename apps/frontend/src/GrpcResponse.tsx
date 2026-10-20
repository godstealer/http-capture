import {useEffect,useMemo,useState} from 'react';
import type {Flow,FlowRecord} from './types';
import {schema,methodPath,decodeMessages,grpcStatus} from './grpc';
import {useRecordedFlow} from './useRecordedFlow';
import {t} from './i18n';
export default function GrpcResponse({flow,visible=true,connected=true}:{flow:FlowRecord;visible?:boolean;connected?:boolean}){
 const [decoded,setDecoded]=useState<{source:Flow|null;method:unknown;messages:unknown[];error:string}>({source:null,method:null,messages:[],error:''});
 const [revision,setRevision]=useState(0);
 useEffect(()=>{const refresh=()=>setRevision(v=>v+1);window.addEventListener('grpc-schema-changed',refresh);return()=>window.removeEventListener('grpc-schema-changed',refresh)},[]);
 const method=useMemo(()=>{try{const sources=JSON.parse(localStorage.getItem('http-capture.grpc.proto.v1')||'[]');return schema(sources).methods.find(m=>methodPath(m)===new URL(flow.request.url).pathname)}catch{return undefined}},[flow.request.url,revision]);
 const response=flow.response;const active=['sending','receiving'].includes(flow.state??'');
 const loaded=useRecordedFlow(response?flow:null,visible,connected),full=loaded.flow;
 useEffect(()=>{
   let current=true;
   setDecoded({source:null,method:null,messages:[],error:''});
   if(visible&&connected&&full?.response)decodeMessages(full.response.bodyBase64,method,full.response.headers,false,active)
     .then(messages=>{if(current)setDecoded({source:full,method,messages,error:''})})
     .catch(error=>{if(current)setDecoded({source:full,method,messages:[],error:String(error)})});
   return()=>{current=false};
 },[full,method,active,visible,connected]);
 if(!visible)return null;
 if(!response)return <p role="status">{active?t('gRPC 调用中…'):t('尚无响应')}</p>;
 const status=grpcStatus(response.headers,response.trailers);
 const result=connected&&decoded.source===full&&decoded.method===method?decoded:null;
 return <div className="grpc-panel request-details"><strong>gRPC {status.code??'—'} · {active&&status.code===undefined?t('接收中…'):status.name}</strong><p>{status.message}</p>{!method&&<p>{t('未选择消息定义时仅按字段编号解析，不推断字段名或业务类型。')}</p>}<details><summary>Metadata</summary><pre>{response.headers.map(h=>h.name+': '+h.value).join('\n')}</pre></details><details open><summary>Trailers</summary><pre>{(response.trailers??[]).map(h=>h.name+': '+h.value).join('\n')||'—'}</pre></details>
 {!connected&&<p role="status">{t('连接内核后才能读取记录的正文。')}</p>}
 {loaded.loading&&<p role="status">{t('正在加载完整 gRPC 正文…')}</p>}
 {!full&&!loaded.loading&&<>{!loaded.automatic&&<p>{t('此操作会将完整请求和响应正文读入内存后解析；大正文可能占用较多内存。')}</p>}<button disabled={!connected} onClick={()=>void loaded.load()}>{t('加载完整 gRPC 正文')}</button></>}
 {loaded.error&&<p role="alert">{loaded.error}</p>}{result?.error&&<p role="alert">{result.error}</p>}{result?.messages.map((value,index)=><details className="grpc-message" key={index}><summary>↓ #{index+1} · {JSON.stringify(value).slice(0,140)}</summary><pre>{JSON.stringify(value,null,2)}</pre></details>)}</div>
}
