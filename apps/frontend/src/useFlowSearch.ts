import {useEffect,useMemo,useRef,useState} from 'react';
import {flowSearchIdentity,flowSearchText,searchFlowResponse,type ResponseSearchResult} from './flowSearch';
import type {FlowRecord} from './types';

interface SearchState {query:string;matches:Set<FlowRecord|string>;scanned:number;total:number;unavailable:number;searching:boolean}
const empty=(query=''):SearchState=>({query,matches:new Set(),scanned:0,total:0,unavailable:0,searching:false});
/** Typing is debounced independently of incoming captures. Scanning stays
 * serial, revisits only changed revisions, and retains booleans, never bodies. */
export function useFlowSearch(flows:FlowRecord[],query:string,active:boolean):SearchState {
  const [state,setState]=useState<SearchState>(()=>empty());
  const entries=useMemo(()=>flows.map(flow=>({flow,key:flowSearchIdentity(flow),text:flowSearchText(flow)})),[flows]);
  const latest=useRef(entries);latest.current=entries;
  const wake=useRef<(()=>void)|null>(null),reading=useRef<{key:FlowRecord|string;controller:AbortController}|null>(null);
  const normalized=query.toLowerCase();
  useEffect(()=>{
    if(!active||!normalized){setState(empty(normalized));return}
    const controller=new AbortController(),results=new Map<FlowRecord|string,ResponseSearchResult>();
    let started=false,running=false,lastPublish=0,publishTimer:ReturnType<typeof setTimeout>|undefined;
    const publish=(force=false)=>{
      if(controller.signal.aborted)return;
      const remaining=100-(Date.now()-lastPublish);
      if(!force&&remaining>0){publishTimer??=setTimeout(()=>{publishTimer=undefined;publish(true)},remaining);return}
      clearTimeout(publishTimer);publishTimer=undefined;
      lastPublish=Date.now();const matches=new Set<FlowRecord|string>();let scanned=0,total=0,unavailable=0;
      for(const entry of latest.current){
        if(entry.text.includes(normalized)||!entry.flow.response)continue;
        total++;const result=results.get(entry.key);
        if(result){scanned++;if(result==='match')matches.add(entry.key);if(result==='unavailable')unavailable++}
      }
      setState({query:normalized,matches,scanned,total,unavailable,searching:scanned<total});
    };
    const run=async()=>{
      if(!started||running||controller.signal.aborted)return;running=true;
      try {
        while(!controller.signal.aborted){
          const snapshot=latest.current,keys=new Set(snapshot.map(entry=>entry.key));
          for(const key of results.keys())if(!keys.has(key))results.delete(key);
          const pending=snapshot.filter(entry=>!entry.text.includes(normalized)&&!!entry.flow.response&&!results.has(entry.key));
          let processed=0;
          for(const entry of pending){
            if(controller.signal.aborted)return;
            const readController=new AbortController(),abort=()=>readController.abort();
            reading.current={key:entry.key,controller:readController};controller.signal.addEventListener('abort',abort,{once:true});
            try {
              const result=await searchFlowResponse(entry.flow,normalized,{signal:readController.signal});
              if(!readController.signal.aborted)results.set(entry.key,result);
            }catch {if(!readController.signal.aborted)results.set(entry.key,'unavailable')}
            finally {controller.signal.removeEventListener('abort',abort);if(reading.current?.controller===readController)reading.current=null}
            if(controller.signal.aborted)return;
            publish();
            if(++processed%10===0)await new Promise(resolve=>setTimeout(resolve,0));
            if(snapshot!==latest.current)break;
          }
          publish(true);
          if(snapshot===latest.current)break;
        }
      }finally {running=false}
    };
    wake.current=()=>{publish();void run()};publish(true);
    const timer=setTimeout(()=>{started=true;void run()},250);
    return()=>{clearTimeout(timer);clearTimeout(publishTimer);controller.abort();wake.current=null};
  },[normalized,active]);
  useEffect(()=>{
    const current=reading.current;
    if(current&&!entries.some(entry=>entry.key===current.key))current.controller.abort();
    wake.current?.();
  },[entries]);
  return state.query===normalized?state:empty(normalized);
}
