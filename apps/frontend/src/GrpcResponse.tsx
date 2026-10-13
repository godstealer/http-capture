import {useEffect,useMemo,useState} from 'react';
import type {Flow} from './types';
import {schema,methodPath,decodeMessages,grpcStatus} from './grpc';
import {t} from './i18n';
export default function GrpcResponse({flow}:{flow:Flow}){
 const [messages,setMessages]=useState<unknown[]>([]),[error,setError]=useState('');
 const [revision,setRevision]=useState(0);
 useEffect(()=>{const refresh=()=>setRevision(v=>v+1);window.addEventListener('grpc-schema-changed',refresh);return()=>window.removeEventListener('grpc-schema-changed',refresh)},[]);
 const method=useMemo(()=>{try{const sources=JSON.parse(localStorage.getItem('http-capture.grpc.proto.v1')||'[]');return schema(sources).methods.find(m=>methodPath(m)===new URL(flow.request.url).pathname)}catch{return undefined}},[flow.request.url,revision]);
 const response=flow.response;const active=['sending','receiving'].includes(flow.state??'');
 useEffect(()=>{let current=true;if(response)decodeMessages(response.bodyBase64,method,response.headers,false,active).then(v=>{if(current){setMessages(v);setError('')}}).catch(e=>{if(current)setError(String(e))});return()=>{current=false}},[response,method,active]);
 if(!response)return <p role="status">{active?t('gRPC 调用中…'):t('尚无响应')}</p>;
 const status=grpcStatus(response.headers,response.trailers);
 return <div className="grpc-panel request-details"><strong>gRPC {status.code??'—'} · {active&&status.code===undefined?t('接收中…'):status.name}</strong><p>{status.message}</p>{!method&&<p>{t('未选择消息定义时仅按字段编号解析，不推断字段名或业务类型。')}</p>}<details><summary>Metadata</summary><pre>{response.headers.map(h=>h.name+': '+h.value).join('\n')}</pre></details><details open><summary>Trailers</summary><pre>{(response.trailers??[]).map(h=>h.name+': '+h.value).join('\n')||'—'}</pre></details>{error&&<p role="alert">{error}</p>}{messages.map((value,index)=><details className="grpc-message" key={index}><summary>↓ #{index+1} · {JSON.stringify(value).slice(0,140)}</summary><pre>{JSON.stringify(value,null,2)}</pre></details>)}</div>
}
