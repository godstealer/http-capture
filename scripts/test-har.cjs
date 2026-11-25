const {buildSync}=require('esbuild');
const assert=require('node:assert/strict');
function load(path){const r=buildSync({entryPoints:[path],bundle:true,platform:'node',format:'cjs',write:false,logLevel:'silent',define:{'import.meta.url':JSON.stringify('file:///fixture/har.ts')}});const m={exports:{}};new Function('module','exports',r.outputFiles[0].text)(m,m.exports);return m.exports;}
const {createHar,serializeHar}=load('apps/frontend/src/har.ts');const {readSession}=load('apps/frontend/src/sessions.ts');
(async()=>{
const flow={id:'test',startedAt:1700000000000,durationMs:13,clientProtocol:'HTTP/2',request:{method:'POST',url:'https://example.invalid/?x=1&x=2',headers:[{name:'X-A',value:'1'},{name:'X-A',value:'2'}],bodyBase64:'/wAB'},response:{status:200,version:'HTTP/1.1',headers:[{name:'content-encoding',value:'gzip'},{name:'content-type',value:'application/octet-stream'}],bodyBase64:'eA=='}};
const har=await createHar([flow],async()=>Uint8Array.from([0,255,12]));const entry=har.log.entries[0];assert.equal(har.log.version,'1.2');assert.equal(entry.response.content.text,'AP8M');assert.equal(entry.response.content.size,3);assert.equal(entry.response.bodySize,1);assert.deepEqual(entry.request.headers,flow.request.headers);assert.equal(entry.request.queryString.length,2);assert.equal(entry.request.postData._bodyBase64,'/wAB');const imported=readSession(JSON.stringify(har))[0];assert.equal(imported.request.bodyBase64,'/wAB');assert.equal(imported.response.bodyBase64,'AP8M');assert(!imported.response.headers.some(h=>h.name==='content-encoding'));
for(const raw of [false,true])await assert.rejects(()=>createHar([flow],async()=>{throw Error('broken compression')},raw),/HAR .*broken compression/,'failed decoding must abort instead of silently exporting a response without standard content');
const empty=await createHar([{...flow,response:null,error:'failed'}],async()=>{throw Error('unexpected')});assert.equal(empty.log.entries[0].response.status,0);assert.equal(empty.log.entries[0]._error,'failed');
const text=await createHar([{...flow,request:{...flow.request,bodyBase64:Buffer.from('\ufeff中文').toString('base64')},response:null}],async()=>new Uint8Array());assert.equal(readSession(JSON.stringify(text))[0].request.bodyBase64,Buffer.from('\ufeff中文').toString('base64'));
const raw=await createHar([flow],async()=>Uint8Array.from([0,255,12]),true);
assert.equal(raw.log.entries[0].response.content.text,'AP8M','standard HAR readers receive the decoded response in the lossless mode too');
assert.equal(raw.log.entries[0].response.content.encoding,'base64');
assert.equal(raw.log.entries[0].response.content.size,3);
assert.equal(raw.log.entries[0].response.content._rawBodyBase64,flow.response.bodyBase64);
const restored=readSession(JSON.stringify(raw))[0];assert.equal(restored.response.bodyBase64,flow.response.bodyBase64);
// Exercise genuine compression and Unicode in both modes, independently of
// our importer: ordinary HAR consumers ignore all underscore extensions.
const {gzipSync,gunzipSync}=require('node:zlib');
const decoded=Buffer.from('{"response":"正文可见","ok":true}');
const compressed={...flow,response:{...flow.response,bodyBase64:gzipSync(decoded).toString('base64'),headers:[{name:'Content-Encoding',value:'gzip'},{name:'Content-Type',value:'application/json'},{name:'Set-Cookie',value:'a=1'},{name:'Set-Cookie',value:'b=2'}]}};
for(const includeOriginal of [false,true]){
 let calls=0;
 const result=await createHar([compressed],async(body,encoding)=>{calls++;assert.equal(encoding,'gzip');return gunzipSync(Buffer.from(body,'base64'))},includeOriginal);
 const output=result.log.entries[0].response;
 assert.equal(calls,1);assert.equal(Buffer.from(output.content.text,output.content.encoding).toString(),decoded.toString());
 assert.equal(output.content.size,decoded.length);assert.equal(output.bodySize,Buffer.from(compressed.response.bodyBase64,'base64').length);
 assert.deepEqual(output.headers,compressed.response.headers,'duplicate headers retain their order');
 assert.equal(output.content._rawBodyBase64,includeOriginal?compressed.response.bodyBase64:undefined);
 const roundtrip=readSession(JSON.stringify(result))[0].response;
 assert.equal(roundtrip.bodyBase64,includeOriginal?compressed.response.bodyBase64:decoded.toString('base64'));
}
// Exercise the default serializer path: compressed responses must actually
// use the decoder worker and the worker is released after success/failure.
let workers=0,terminations=0,workerFailure=false;
global.Worker=class {
 constructor(url,options){workers++;assert.equal(options.type,'module');assert.match(String(url),/response\.worker\.ts$/)}
 postMessage(message){assert.equal(message.parser,'bytes');assert.equal(message.encoding,'gzip');queueMicrotask(()=>this.onmessage({data:workerFailure?{error:'fixture decode failed'}:{decodedBlob:new Blob([gunzipSync(Buffer.from(message.body,'base64'))])}}))}
 terminate(){terminations++}
};
const defaultOutput=JSON.parse(await serializeHar([compressed,compressed]));
assert.equal(workers,1);assert.equal(terminations,1);
for(const entry of defaultOutput.log.entries){assert.equal(entry.response.content.text,decoded.toString('base64'));assert.equal(entry.response.content._rawBodyBase64,compressed.response.bodyBase64)}
workerFailure=true;await assert.rejects(()=>serializeHar([compressed]),/HAR .*fixture decode failed/);assert.equal(terminations,2);
delete global.Worker;
const emptyEncoded=await createHar([{...flow,response:{...flow.response,bodyBase64:''}}],()=>assert.fail('empty HEAD/204 entities do not decode an absent gzip stream'),true);
assert.equal(emptyEncoded.log.entries[0].response.content.text,'');assert.equal(emptyEncoded.log.entries[0].response.content.size,0);
const oldRawHar=structuredClone(raw);delete oldRawHar.log.entries[0].response.content.text;delete oldRawHar.log.entries[0].response.content.encoding;
assert.equal(readSession(JSON.stringify(oldRawHar))[0].response.bodyBase64,flow.response.bodyBase64,'legacy raw-only HAR remains importable');


for(const candidate of [
 {...flow,bodyCapture:{request:'notRecorded'}},
 {...flow,bodyCapture:{response:'notRecorded'}},
 {...flow,trafficOrigin:{entry:'android-lan'}},
])await assert.rejects(()=>createHar([candidate],()=>assert.fail('unrecorded bytes must not reach the HAR decoder')),/not recorded/);
const actuallyEmpty={...flow,trafficOrigin:{entry:'android-lan'},bodyCapture:{request:'recorded',response:'recorded'},request:{...flow.request,bodyBase64:''},response:{...flow.response,headers:[],bodyBase64:''}};
const emptyRecorded=await createHar([actuallyEmpty],()=>assert.fail('real empty bytes do not decode'));assert.equal(emptyRecorded.log.entries[0].request.bodySize,0);assert.equal(emptyRecorded.log.entries[0].response.content.size,0);
console.log('HAR export: duplicate headers/query, binary and BOM request round-trip, standard decoded responses in both modes, raw-byte preservation, decode failure rejection and missing responses passed');
})().catch(e=>{console.error(e);process.exitCode=1});
