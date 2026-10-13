import { invoke } from './api';
export async function importSession(file: File, progress:(count:number,total:number)=>void) {
 const worker=new Worker(new URL('./session-import.worker.ts',import.meta.url),{type:'module'});
 let id:string|undefined,done=0;
 try {
  id=await invoke<string>('begin_import');
  await new Promise<void>((resolve,reject)=>{
   worker.onerror=()=>reject(new Error('Unable to parse session file'));
   worker.onmessage=async event=>{
    try {
     if(event.data.error)throw new Error(event.data.error);
     if(event.data.complete){resolve();return;}
     if(event.data.batch){
      done=await invoke<number>('append_import',{id,offset:done,flows:event.data.batch});
      progress(done,event.data.total);worker.postMessage({ack:true});
     } else progress(0,event.data.total);
    } catch(error){reject(error)}
   };worker.postMessage(file);
  });
  return await invoke<number>('finish_import',{id,commit:true});
 } catch(error){
  if(id)await invoke('finish_import',{id,commit:false}).catch(()=>{});
  throw error;
 } finally{worker.terminate()}
}
