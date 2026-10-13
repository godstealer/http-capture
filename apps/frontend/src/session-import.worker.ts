import { readSession } from './sessions';
let resume: (()=>void) | undefined;
self.onmessage = async (event: MessageEvent<File | {ack:true}>) => {
 if (!(event.data instanceof File)) { resume?.(); resume=undefined; return; }
 try {
  const flows=readSession(await event.data.text());
  self.postMessage({total:flows.length});
  let batch:typeof flows=[],size=0;
  const flush=async()=>{
   if(!batch.length)return;
   const acknowledgement=new Promise<void>(resolve=>{resume=resolve});
   self.postMessage({batch,total:flows.length}); batch=[];size=0;
   await acknowledgement;
  };
  for(let i=0;i<flows.length;i++) {
   const flow=flows[i];
   const length=new TextEncoder().encode(JSON.stringify(flow)).length;
   if(length>60*1024*1024)throw new Error('A single record exceeds the 60 MiB import transport limit');
   if(batch.length&&(size+length>4*1024*1024||batch.length>=250))await flush();
   batch.push(flow);size+=length;
   delete flows[i];
  }
  await flush();self.postMessage({complete:true});
 } catch(error){self.postMessage({error:String(error)})}
};
