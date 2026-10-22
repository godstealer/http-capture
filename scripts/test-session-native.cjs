const assert=require('node:assert/strict');
const {build}=require('esbuild');
(async()=>{
const result=await build({entryPoints:['apps/frontend/src/sessions.ts'],bundle:true,platform:'node',format:'cjs',write:false,plugins:[{
 name:'native-invoke-fixture',setup(build){build.onResolve({filter:/^@tauri-apps\/api\/core$/},()=>({path:'tauri-core',namespace:'fixture'}));build.onLoad({filter:/.*/,namespace:'fixture'},()=>({contents:'export const isTauri=()=>true; export const invoke=(name,args)=>global.__nativeInvoke(name,args);',loader:'js'}));}
}]});
const moduleShim={exports:{}};new Function('module','exports',result.outputFiles[0].text)(moduleShim,moduleShim.exports);
const flow={id:'one',startedAt:1,durationMs:2,request:{method:'GET',url:'https://example.test/',headers:[],bodyBase64:'',tls:{preset:'native'},engine:'auto'},response:{status:200,version:'HTTP/2',headers:[],bodyBase64:''},clientProtocol:'HTTP/2',error:null};
 for(const format of ['json','har']){
  let offset=0,buffer=Buffer.alloc(0),finished=false;
  global.__nativeInvoke=async(name,args)=>{
   if(name==='begin_session_export'){assert.equal(args.format,format);return 'export-id';}
   if(name==='append_session_export'){assert.equal(args.id,'export-id');assert.equal(args.offset,offset);assert(args.bytes.length<=65536);buffer=Buffer.concat([buffer,Buffer.from(args.bytes)]);offset+=args.bytes.length;return offset;}
   if(name==='finish_session_export'){assert.equal(args.commit,true);finished=true;return 'saved.'+format;}
   throw Error('Unexpected invoke: '+name);
  };
  assert.match(await moduleShim.exports.saveSession([flow,flow],format),new RegExp('saved\\.'+format));
  assert.equal(finished,true);assert.equal(moduleShim.exports.readSession(buffer.toString()).length,2);
 }
 let aborted=false;
 global.__nativeInvoke=async(name,args)=>{
  if(name==='begin_session_export')return 'export-id';
  if(name==='append_session_export')throw Error('disk full');
  if(name==='finish_session_export'){assert.equal(args.commit,false);aborted=true;return null;}
  throw Error(name);
 };
 await assert.rejects(moduleShim.exports.saveSession([flow],'json'),/disk full/);assert.equal(aborted,true);
 console.log('Native session export: JSON/HAR chunks preserve format and failed writes abort.');
})().catch(error=>{console.error(error);process.exitCode=1});
