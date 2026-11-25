import {invoke} from './api';
import {isBodyRecorded} from './bodyCapture';
import {flowBodyLength,isFlowMetadata} from './flowRecords';
import {readCompleteRecordedBody,recordedBodyRequest,validateRecordedBodyPage,RECORDED_BODY_PAGE_BYTES,RECORDED_BODY_COMPLETE_BYTES} from './recordedBodyModel';
import {trafficOriginSearchText} from './trafficOrigin';
import type {FlowRecord,Header} from './types';

export type ResponseSearchResult='match'|'miss'|'unavailable';
export function flowSearchText(flow:FlowRecord):string {
  const headers=(items:Header[])=>items.filter(h=>!h.disabled).map(h=>`${h.name}: ${h.value}`).join('\n');
  return `${flow.request.method} ${flow.request.url} ${flow.response?.status??''} ${trafficOriginSearchText(flow)}\n${headers(flow.request.headers)}\n${headers(flow.response?.headers??[])}`.toLowerCase();
}
export function flowSearchIdentity(flow:FlowRecord):FlowRecord|string {
  return isFlowMetadata(flow)?JSON.stringify([flow.id,flow.bodyRevision,isBodyRecorded(flow,'response'),flow.response?.headers]):flow;
}
const header=(headers:Header[],name:string)=>headers.filter(h=>!h.disabled&&h.name.toLowerCase()===name).map(h=>h.value).join(', ');
function check(signal?:AbortSignal){if(signal?.aborted)throw new DOMException('Response search cancelled','AbortError')}

/** Only compressed bodies need full decoding. The existing decoder worker owns
 * its 8 MiB processing budget; no formatted/decoded bodies are cached by search. */
export function searchEncodedResponse(body:string,headers:Header[],query:string,signal?:AbortSignal):Promise<boolean> {
  check(signal);
  return new Promise((resolve,reject)=>{
    const worker=new Worker(new URL('./response.worker.ts',import.meta.url),{type:'module'});
    const finish=(value?:boolean,error?:unknown)=>{clearTimeout(timer);signal?.removeEventListener('abort',abort);worker.terminate();error?reject(error):resolve(value!)};
    const abort=()=>finish(undefined,new DOMException('Response search cancelled','AbortError'));
    const timer=setTimeout(()=>finish(undefined,Error('Response search decode timeout')),30000);
    signal?.addEventListener('abort',abort,{once:true});
    worker.onmessage=event=>{
      if(event.data.binary||typeof event.data.raw!=='string')finish(undefined,Error('Response text cannot be decoded'));
      else finish(event.data.raw.toLowerCase().includes(query));
    };
    worker.onerror=()=>finish(undefined,Error('Response search decoder failed'));
    worker.postMessage({body,contentType:header(headers,'content-type'),encoding:header(headers,'content-encoding'),parser:'text'});
  });
}

/** Scan plain bodies by validated ranges, with UTF-8/charset state and a tail
 * spanning range boundaries. An unread/unsupported body is never a clean miss. */
export async function searchFlowResponse(flow:FlowRecord,query:string,options:{signal?:AbortSignal}={},call:typeof invoke=invoke,
  decode:typeof searchEncodedResponse=searchEncodedResponse):Promise<ResponseSearchResult> {
  const signal=options.signal;check(signal);
  if(!query||!flow.response)return 'miss';
  if(!isBodyRecorded(flow,'response'))return 'unavailable';
  query=query.toLowerCase();
  try {
    const total=flowBodyLength(flow,'response');
    if(total===undefined)return 'unavailable';
    if(total===0)return 'miss';
    const encoding=header(flow.response.headers,'content-encoding').split(',').map(v=>v.trim().toLowerCase()).filter(v=>v&&v!=='identity');
    if(encoding.length){
      if(total>RECORDED_BODY_COMPLETE_BYTES)return 'unavailable';
      const body=isFlowMetadata(flow)?await readCompleteRecordedBody(flow,'response',{signal},call):flow.response.bodyBase64;
      check(signal);const found=await decode(body,flow.response.headers,query,signal);check(signal);
      return found?'match':'miss';
    }
    const charset=/charset\s*=\s*["']?([^\s;"']+)/i.exec(header(flow.response.headers,'content-type'))?.[1]??'utf-8';
    const decoder=new TextDecoder(charset,{fatal:true});
    let tail='',pages=0;
    const contains=(bytes:Uint8Array,more:boolean)=>{
      const text=tail+decoder.decode(bytes,{stream:more}).toLowerCase();
      if(text.includes(query))return true;
      tail=query.length>1?text.slice(1-query.length):'';return false;
    };
    if(isFlowMetadata(flow)){
      for(let page=0;page<Math.ceil(total/RECORDED_BODY_PAGE_BYTES);page++){
        check(signal);const request=recordedBodyRequest(flow,'response',page);
        const raw=await call<unknown>('read_flow_body',{...request});check(signal);
        const result=validateRecordedBodyPage(raw,request,total);
        if(contains(result.bytes,result.nextOffset!==null))return 'match';
      }
    }else{
      // A 64 KiB Base64 chunk represents an integral number of original bytes.
      const body=flow.response.bodyBase64;
      for(let start=0;start<body.length;start+=65536){
        check(signal);const binary=atob(body.slice(start,start+65536));
        if(contains(Uint8Array.from(binary,c=>c.charCodeAt(0)),start+65536<body.length))return 'match';
        if(++pages%16===0)await new Promise(resolve=>setTimeout(resolve,0));
      }
    }
    check(signal);return 'miss';
  }catch(error){check(signal);return 'unavailable'}
}
