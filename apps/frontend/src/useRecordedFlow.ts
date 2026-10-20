import {useEffect,useRef,useState} from 'react';
import {flowBodyLength,isFlowMetadata,loadCompleteFlow} from './flowRecords';
import type {BodyVariant,Flow,FlowRecord} from './types';

export const AUTO_RECORDED_FLOW_BYTES=64*1024;
const variants:BodyVariant[]=['request','response','originalRequest','originalResponse'];
type Identity=string|Flow|null;
interface Context {identity:Identity;visible:boolean;connected:boolean}
interface Loaded {context:Context;flow:Flow|null;loading:boolean;error:string}

/** Budget the complete four-entity snapshot, not only the displayed response.
 * Empty entities are real zero-byte bodies; missing/invalid references never
 * become permission to fabricate an empty RequestDraft. */
export function smallRecordedFlow(record:FlowRecord):boolean {
  let total=0;
  for(const variant of variants){
    if(!record[variant])continue;
    const length=flowBodyLength(record,variant);
    if(length===undefined||!Number.isSafeInteger(length)||length<0||length>AUTO_RECORDED_FLOW_BYTES-total)return false;
    total+=length;
  }
  return true;
}

/** A visible, connected small snapshot may load automatically. Larger ones
 * require a separate explicit load action. No cache survives a hidden panel,
 * disconnection or changed ID/version, including a hide/reopen at the same ID. */
export function useRecordedFlow(record:FlowRecord|null|undefined,visible=true,connected=true){
  const identity:Identity=record?(isFlowMetadata(record)?JSON.stringify([record.id,record.bodyRevision]):record):null;
  const context=useRef<Context>({identity,visible,connected});
  if(context.current.identity!==identity||context.current.visible!==visible||context.current.connected!==connected){
    context.current={identity,visible,connected};
  }
  const rendered=context.current;
  const [loaded,setLoaded]=useState<Loaded>({context:rendered,flow:null,loading:false,error:''});
  const pending=useRef<Context|null>(null);
  const automatic=!!record&&smallRecordedFlow(record);
  async function load(expected:Context=rendered,source=record){
    if(!source||!expected.visible||!expected.connected||context.current!==expected||pending.current===expected)return;
    pending.current=expected;
    setLoaded({context:expected,flow:null,loading:true,error:''});
    try{
      const full=await loadCompleteFlow(source);
      if(context.current===expected)setLoaded({context:expected,flow:full,loading:false,error:''});
    }catch(error){
      if(context.current===expected)setLoaded({context:expected,flow:null,loading:false,error:String(error)});
    }finally{if(pending.current===expected)pending.current=null;}
  }
  useEffect(()=>{
    const expected=context.current;
    setLoaded({context:expected,flow:null,loading:false,error:''});
    if(automatic&&visible&&connected)void load(expected,record);
    return()=>{if(context.current===expected)context.current={...expected};};
  },[identity,visible,connected,automatic]);
  const current=visible&&connected&&loaded.context===rendered?loaded:null;
  return {flow:current?.flow??null,loading:current?.loading??false,error:current?.error??'',automatic,
    load:()=>load(rendered,record)};
}
