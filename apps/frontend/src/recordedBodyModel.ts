import type {BodyVariant, FlowMetadata, Header} from './types';
import {invoke} from './api';

export const RECORDED_BODY_PAGE_BYTES=64*1024;
export const RECORDED_BODY_COMPLETE_BYTES=8*1024*1024;
export interface RecordedBodyRead {id:string;variant:BodyVariant;expectedVersion:string;offset:number;length:number}
export interface RecordedBodyPage {offset:number;totalBytes:number;bodyBase64:string;nextOffset:number|null;text:string|null;bytes:Uint8Array;complete:boolean}
function requireValue(condition:unknown,message:string):asserts condition {if(!condition)throw Error(message)}

export function recordedBodyIdentity(flow:FlowMetadata,variant:BodyVariant):string {
  return JSON.stringify([flow.id,flow.bodyRevision,variant]);
}
export function recordedBodyRequest(flow:FlowMetadata,variant:BodyVariant,page:number):RecordedBodyRead {
  requireValue(['request','response','originalRequest','originalResponse'].includes(variant),'Recorded body variant is unavailable');
  requireValue(typeof flow.id==='string'&&!!flow.id&&flow.id.length<=128&&typeof flow.bodyRevision==='string'&&!!flow.bodyRevision&&flow.bodyRevision.length<=128,'Recorded body reference is unavailable');
  const total=flow.bodyReferences[variant]?.byteLength;
  requireValue(typeof total==='number'&&Number.isSafeInteger(total)&&total>=0,'Recorded body is unavailable');
  requireValue(Number.isSafeInteger(page)&&page>=0&&page<Math.max(1,Math.ceil(total/RECORDED_BODY_PAGE_BYTES)),'Recorded body page is unavailable');
  return {id:flow.id,variant,expectedVersion:flow.bodyRevision,offset:page*RECORDED_BODY_PAGE_BYTES,length:RECORDED_BODY_PAGE_BYTES};
}
export function validateRecordedBodyPage(raw:unknown,request:RecordedBodyRead,total:number):RecordedBodyPage {
  requireValue(raw!==null&&typeof raw==='object','Invalid recorded body response');
  const page=raw as Record<string,unknown>;
  requireValue(Number.isSafeInteger(total)&&total>=0&&page.offset===request.offset&&page.totalBytes===total,'Recorded body changed; reload its metadata');
  requireValue(typeof page.bodyBase64==='string'&&page.bodyBase64.length<=Math.ceil(RECORDED_BODY_PAGE_BYTES/3)*4
    &&page.bodyBase64.length%4===0&&/^[A-Za-z0-9+/]*(?:={1,2})?$/.test(page.bodyBase64),'Invalid recorded body range');
  let binary:string;
  try {binary=atob(page.bodyBase64)}catch {throw Error('Invalid recorded body range')}
  requireValue(btoa(binary)===page.bodyBase64,'Invalid recorded body range');
  const expected=Math.min(RECORDED_BODY_PAGE_BYTES,Math.max(0,total-request.offset));
  requireValue(binary.length===expected,'Incomplete recorded body range');
  const next=request.offset+expected<total?request.offset+expected:null;
  requireValue(page.nextOffset===next,'Recorded body page boundaries differ from its metadata');
  const bytes=Uint8Array.from(binary,value=>value.charCodeAt(0));
  let text:string|null=null;
  try {const decoded=new TextDecoder('utf-8',{fatal:true}).decode(bytes);if(!/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(decoded))text=decoded}catch {/* A range can split a UTF-8 character. Preserve the original bytes. */}
  return {offset:request.offset,totalBytes:total,bodyBase64:page.bodyBase64,nextOffset:next,text,bytes,complete:request.offset===0&&next===null};
}
export function recordedBodyHex(bytes:Uint8Array,offset:number):string {
  const lines:string[]=[];
  for(let position=0;position<bytes.length;position+=16) {
    lines.push(`${(offset+position).toString(16).padStart(8,'0')}  ${Array.from(bytes.subarray(position,position+16),value=>value.toString(16).padStart(2,'0')).join(' ')}`);
  }
  return lines.join('\n');
}
export function recordedBodyEncoded(headers:Header[]):boolean {
  return headers.some(header=>!header.disabled&&header.name.toLowerCase()==='content-encoding'
    &&header.value.split(',').some(value=>value.trim()&&value.trim().toLowerCase()!=='identity'));
}

/** Explicit processing budget for one recorded entity. It does not limit
 * capture or downloads and never loads the other three body slots. */
export async function readCompleteRecordedBody(flow:FlowMetadata,variant:BodyVariant,
  options:{signal?:AbortSignal;isCurrent?:()=>boolean}={},call:typeof invoke=invoke):Promise<string> {
  recordedBodyRequest(flow,variant,0);
  const total=flow.bodyReferences[variant]!.byteLength;
  requireValue(total<=RECORDED_BODY_COMPLETE_BYTES,'Complete body processing exceeds 8 MiB');
  const checkCurrent=()=>{if(options.signal?.aborted||options.isCurrent&&!options.isCurrent())throw new DOMException('Recorded body processing cancelled','AbortError')};
  checkCurrent();
  let bytes:Uint8Array|null=new Uint8Array(total);
  const release=()=>{bytes=null};options.signal?.addEventListener('abort',release,{once:true});
  try {
    for(let index=0;index<Math.max(1,Math.ceil(total/RECORDED_BODY_PAGE_BYTES));index++) {
      checkCurrent();const request=recordedBodyRequest(flow,variant,index);
      const raw=await call<unknown>('read_flow_body',{...request});checkCurrent();
      const page=validateRecordedBodyPage(raw,request,total);bytes!.set(page.bytes,request.offset);
    }
    checkCurrent();
    // Each range has its own Base64 padding: combine original bytes first.
    const parts:string[]=[];
    for(let start=0;start<total;start+=8192)parts.push(String.fromCharCode(...bytes!.subarray(start,start+8192)));
    return btoa(parts.join(''));
  } finally {release();options.signal?.removeEventListener('abort',release)}
}
