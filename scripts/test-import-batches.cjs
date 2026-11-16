global.File=class File extends require('node:buffer').Blob {};
const {buildSync}=require('esbuild');const assert=require('node:assert/strict');
const messages=[];let handler;
global.self={postMessage(message){messages.push(message);if(message.batch)setImmediate(()=>handler({data:{ack:true}}));}};
const code=buildSync({entryPoints:['apps/frontend/src/session-import.worker.ts'],bundle:true,platform:'node',format:'cjs',write:false,logLevel:'silent'}).outputFiles[0].text;
new Function(code)();handler=self.onmessage;
const flow={id:'',request:{},notes:[]};
(async()=>{await handler({data:new File([JSON.stringify({format:'http-capture-session',version:1,flows:Array.from({length:751},()=>flow)})])});
assert.equal(messages[0].total,751);assert.deepEqual(messages.filter(m=>m.batch).map(m=>m.batch.length),[250,250,250,1]);assert.equal(messages.at(-1).complete,true);
messages.length=0;
const lan={id:'lan',trafficOrigin:{entry:'android-lan'},bodyCapture:{request:'notRecorded',response:'notRecorded'},request:{bodyBase64:Buffer.from('not a captured entity').toString('base64')},response:{bodyBase64:'YWJj'},notes:[]};
await handler({data:new File([JSON.stringify({format:'http-capture-session',version:2,flows:[lan]})])});
assert.equal(messages.at(-1).complete,true);const imported=messages.find(m=>m.batch).batch[0];
assert.equal(imported.request.bodyBase64,'');assert.equal(imported.response.bodyBase64,'');assert.equal(imported.bodyCapture.request,'notRecorded');assert.equal(imported.trafficOrigin.entry,'android-lan');
messages.length=0;const legacy={...lan};delete legacy.bodyCapture;
await handler({data:new File([JSON.stringify({format:'http-capture-session',version:1,flows:[legacy]})])});
assert.equal(messages.find(m=>m.batch).batch[0].bodyCapture.request,'notRecorded','old LAN v1 imports normalize the missing availability flag');assert.equal(messages.find(m=>m.batch).batch[0].request.bodyBase64,'');
for(const payload of [{format:'http-capture-session',version:3,flows:[lan]},{format:'http-capture-session',version:2,flows:[{...lan,bodyCapture:{request:'partial'}}]}]){
 messages.length=0;await handler({data:new File([JSON.stringify(payload)])});assert(messages.some(m=>m.error));assert(!messages.some(m=>m.batch||m.complete),'unsupported or malformed session never enters an import batch');
}
console.log('Import worker: 751 records delivered in acknowledged batches, completion after final acknowledgement.');})().catch(e=>{console.error(e);process.exitCode=1});
