import {invoke} from './api';
import {assertRecordedBodies, bodyCaptureState, isBodyRecorded, normalizeFlowBodyCapture as snapshot, validateBodyCapture} from './bodyCapture';
import {createFlowChanges, type FlowChanges} from './flowChanges';
import {createFlowRefresh, isMissingFlowEndpoint} from './flowRefresh';
import type {BodyVariant, Flow, FlowMetadata, FlowRecord} from './types';

export {isMissingFlowEndpoint};
type Invoke = typeof invoke;
const variants:BodyVariant[]=['request','response','originalRequest','originalResponse'];
export function isFlowMetadata(record:FlowRecord):record is FlowMetadata {
  return 'bodyRevision' in record;
}
function base64Length(value:string) {
  return value.length/4*3-(value.endsWith('==')?2:value.endsWith('=')?1:0);
}
export function flowBodyLength(record:FlowRecord,variant:BodyVariant):number|undefined {
  if(!isBodyRecorded(record,variant))return undefined;
  if(isFlowMetadata(record))return record.bodyReferences[variant]?.byteLength;
  const body=record[variant]?.bodyBase64;
  return body===undefined?undefined:base64Length(body);
}
function checkedMetadata(value:FlowMetadata):FlowMetadata {
  if(!value||typeof value.id!=='string'||!value.id||typeof value.bodyRevision!=='string'||!value.bodyRevision||!value.request||typeof value.request!=='object'||!value.bodyReferences||typeof value.bodyReferences!=='object'||Array.isArray(value.bodyReferences))throw Error('Invalid recorded flow metadata');
  // Old services projected zero-byte references for LAN placeholders. Keep
  // those rows readable as metadata without granting any body operation.
  if(value.bodyCapture===undefined&&value.trafficOrigin?.entry==='android-lan'){
    const bodyReferences={...value.bodyReferences};
    const bodyCapture:NonNullable<FlowMetadata['bodyCapture']>={};
    for(const variant of variants)if(value[variant]){delete bodyReferences[variant];bodyCapture[variant]='notRecorded';}
    value={...value,bodyReferences,bodyCapture};
  }
  validateBodyCapture(value);
  for(const variant of variants){
    if(value[variant]&&'bodyBase64' in value[variant])throw Error('Recorded flow metadata must not contain body payloads');
    const reference=value.bodyReferences[variant];
    if((!!value[variant]&&isBodyRecorded(value,variant))!==!!reference)throw Error('Recorded entity and body reference differ');
    if(reference&&(typeof reference!=='object'||!Number.isSafeInteger(reference.byteLength)||reference.byteLength<0))throw Error('Invalid recorded body length');
  }
  return value;
}
/** Snapshots use the exact metadata revision and preserve unavailable slots
 * explicitly. Complete actions add their own required-body checks below. */
export async function loadFlowSnapshot(record:FlowRecord,call:Invoke=invoke):Promise<Flow> {
  validateBodyCapture(record);
  if(!isFlowMetadata(record))return snapshot(record);
  const metadata=checkedMetadata(record);
  const full=await call<Flow>('get_flow',{id:metadata.id,expectedVersion:metadata.bodyRevision});
  if(!full||full.id!==record.id||isFlowMetadata(full))throw Error('Incomplete recorded flow result');
  validateBodyCapture(full);
  for(const variant of variants){
    const entity=full[variant],reference=metadata.bodyReferences[variant];
    if(!!entity!==!!metadata[variant])throw Error('Recorded entities differ from their metadata');
    if(entity&&bodyCaptureState(full,variant)!==bodyCaptureState(metadata,variant))throw Error('Body capture state differs from its metadata');
    if(entity&&typeof entity.bodyBase64!=='string')throw Error('Incomplete recorded body result');
    if(reference&&(!entity||base64Length(entity.bodyBase64)!==reference.byteLength))throw Error('Recorded body length differs from its metadata');
  }
  return snapshot(full);
}
/** Sending/copying requires actual bytes; session JSON may instead preserve an
 * explicitly marked metadata-only snapshot with loadFlowSnapshot. */
export async function loadCompleteFlow(record:FlowRecord,call:Invoke=invoke,required:readonly BodyVariant[]=variants):Promise<Flow> {
  validateBodyCapture(record);
  assertRecordedBodies(record,required);
  const full=await loadFlowSnapshot(record,call);
  assertRecordedBodies(full,required);
  return full;
}
/** Replay results remain outside the global history. Read their authoritative
 * lightweight row before merging, instead of inventing a local body revision. */
export async function loadFlowRecord(result:Flow,call:Invoke=invoke):Promise<FlowRecord> {
  try{
    const metadata=await call<FlowMetadata|null>('flow_metadata',{id:result.id});
    if(!metadata)throw Error('Recorded flow not found');
    if(metadata.id!==result.id)throw Error('Recorded flow ID differs');
    return checkedMetadata(metadata);
  }catch(error){if(isMissingFlowEndpoint(error,'flow_metadata'))return result;throw error;}
}
export async function loadHistorySnapshot(call:Invoke=invoke):Promise<FlowRecord[]> {
  try{
    const changes=await call<FlowChanges<FlowMetadata>>('flow_metadata_changes',{since:null});
    return changes.rows.slice().sort((a,b)=>b.position-a.position).map(row=>checkedMetadata(row.flow));
  }catch(error){if(isMissingFlowEndpoint(error,'flow_metadata_changes'))return call<Flow[]>('list_flows');throw error;}
}
/** Every row is retained. Only an unavailable endpoint selects compatibility
 * mode; authentication, storage, timeout and malformed responses never do. */
export function createHistoryRefresh(epoch:()=>number,call:Invoke=invoke) {
  const metadata=createFlowChanges<FlowMetadata>(async since=>{
    const changes=await call<FlowChanges<FlowMetadata>>('flow_metadata_changes',{since});
    return {...changes,rows:changes.rows.map(row=>({...row,flow:checkedMetadata(row.flow)}))};
  });
  const changes=createFlowChanges(since=>call<FlowChanges>('flow_changes',{since}));
  const legacy=createFlowRefresh(async()=>`${epoch()}:${await call<string>('flow_revision')}`,()=>call<Flow[]>('list_flows'));
  let mode:'metadata'|'changes'|'list'='metadata';
  let attempt=0;
  return async(apply:(rows:FlowRecord[])=>void)=>{
    const ticket=++attempt,current=epoch(),applyCurrent=(rows:FlowRecord[])=>{if(ticket===attempt&&current===epoch())apply(rows)};
    if(mode==='metadata'){
      try{await metadata(current,applyCurrent);return}
      catch(error){if(ticket!==attempt||current!==epoch())return;if(!isMissingFlowEndpoint(error,'flow_metadata_changes'))throw error;mode='changes'}
    }
    if(mode==='changes'){
      try{await changes(current,applyCurrent);return}
      catch(error){if(ticket!==attempt||current!==epoch())return;if(!isMissingFlowEndpoint(error,'flow_changes'))throw error;mode='list'}
    }
    await legacy(applyCurrent);
  };
}
