const assert=require('node:assert/strict');
const {buildSync}=require('esbuild');
const bundled=buildSync({entryPoints:['apps/frontend/src/flowRecords.ts'],bundle:true,platform:'node',format:'cjs',write:false}).outputFiles[0].text;
const moduleUnderTest={exports:{}};new Function('module','exports','require',bundled)(moduleUnderTest,moduleUnderTest.exports,require);
const {isFlowMetadata,flowBodyLength,loadCompleteFlow,loadFlowRecord,loadHistorySnapshot,createHistoryRefresh,isMissingFlowEndpoint}=moduleUnderTest.exports;
const encoded=text=>Buffer.from(text).toString('base64');
const makeFull=(id='one')=>({id,parentId:null,source:'replay',startedAt:1,durationMs:2,state:'complete',request:{method:'POST',url:'https://example.test/',headers:[],bodyBase64:encoded('request'),tls:{preset:'native'}},response:{status:200,version:'HTTP/2',headers:[],bodyBase64:encoded('response')},originalRequest:{method:'POST',url:'https://example.test/',headers:[],bodyBase64:'',tls:{preset:'native'}},originalResponse:{status:201,version:'HTTP/2',headers:[],bodyBase64:encoded('original')},notes:[],error:null});
function metadata(full=makeFull(),revision='store:1'){
  const result=structuredClone(full);result.bodyRevision=revision;result.bodyReferences={};
  for(const variant of ['request','response','originalRequest','originalResponse'])if(result[variant]){
    result.bodyReferences[variant]={byteLength:Buffer.from(result[variant].bodyBase64,'base64').length};delete result[variant].bodyBase64;
  }
  return result;
}
const row=(flow,position=1)=>({flow,position});
const snapshot=(revision,rows,reset=true,deleted=[])=>({revision,rows,reset,deleted});
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return {promise,resolve,reject}};
(async()=>{
  const full=makeFull(),meta=metadata(full),before=structuredClone(meta),calls=[];
  assert(isFlowMetadata(meta));assert(!isFlowMetadata(full));
  for(const variant of ['request','response','originalRequest','originalResponse']){
    assert(!Object.hasOwn(meta[variant],'bodyBase64'));
    assert.equal(flowBodyLength(meta,variant),Buffer.from(full[variant].bodyBase64,'base64').length);
    assert.equal(flowBodyLength(full,variant),flowBodyLength(meta,variant));
  }
  assert.equal(flowBodyLength(meta,'originalRequest'),0,'an empty recorded body has a real zero length');
  const noResponse=metadata({...full,response:null,originalResponse:undefined});assert.equal(flowBodyLength(noResponse,'response'),undefined,'an absent entity is distinct from empty bytes');
  const complete=await loadCompleteFlow(meta,async(command,args)=>{calls.push([command,args]);return full});
  assert.equal(complete,full);assert.deepEqual(calls,[['get_flow',{id:'one',expectedVersion:'store:1'}]]);assert.deepEqual(meta,before);
  assert.equal(await loadCompleteFlow(full,()=>assert.fail('legacy complete records need no network read')),full);
  for(const error of [Error('Recorded flow changed; reload its metadata'),Error('HTTP 404: endpoint unavailable'),Error('Recorded flow not found'),Error('connection lost')]){
    let count=0;await assert.rejects(()=>loadCompleteFlow(meta,async command=>{count++;assert.equal(command,'get_flow');throw error}),candidate=>candidate===error);assert.equal(count,1,'complete actions never retry or fall back to empty/old bodies');
  }
  for(const incomplete of [meta,{...full,id:'wrong'}, {...full,response:{...full.response,bodyBase64:encoded('prefix')}},{...full,originalResponse:null},{...full,originalRequest:{...full.originalRequest,bodyBase64:undefined}}])await assert.rejects(()=>loadCompleteFlow(meta,async()=>incomplete),/Incomplete|differ/);
  const missingRef=structuredClone(meta);delete missingRef.bodyReferences.response;
  const extraRef=structuredClone(noResponse);extraRef.bodyReferences.response={byteLength:0};
  const missingRequest=structuredClone(meta);delete missingRequest.request;
  const payloadInMetadata={...meta,response:{...meta.response,bodyBase64:''}};
  for(const malformed of [missingRef,extraRef,missingRequest,payloadInMetadata,{...meta,bodyReferences:{...meta.bodyReferences,response:{byteLength:1.5}}}]){
    await assert.rejects(()=>loadCompleteFlow(malformed,()=>assert.fail('invalid metadata must fail before complete reads')),/metadata|reference|payload|length/);
    await assert.rejects(()=>loadHistorySnapshot(async()=>snapshot('store:1',[row(malformed)])),/metadata|reference|payload|length/);
  }
  await assert.rejects(()=>loadCompleteFlow(noResponse,async()=>full),/entities differ/,'unadvertised entities cannot silently appear in a complete operation');
  const resultCalls=[];assert.equal(await loadFlowRecord(full,async(command,args)=>{resultCalls.push([command,args]);return meta}),meta);assert.deepEqual(resultCalls,[['flow_metadata',{id:'one'}]]);
  await assert.rejects(()=>loadFlowRecord(full,async()=>null),/not found/);
  assert.equal(await loadFlowRecord(full,async()=>{throw Error('Unknown command: flow_metadata')}),full,'only an absent metadata endpoint selects complete legacy compatibility');
  await assert.rejects(()=>loadFlowRecord(full,async()=>{throw Error('HTTP 500: stored flow not found')}),/500/);

  let epoch=0,revision=1,applies=0,seen=[],stage=0;
  const initial=Array.from({length:15003},(_,index)=>row(metadata(makeFull(String(index)),`store:${index+1}`),index));
  const refresh=createHistoryRefresh(()=>epoch,async(command,args)=>{
    assert.equal(command,'flow_metadata_changes','modern history must never call full Flow routes');seen.push(args.since);
    if(stage===0)return snapshot(`store:${revision}`,initial);
    if(stage===1)return snapshot('store:2',[row(metadata(makeFull('7'),'store:20000'),7)],false,['8']);
    if(stage===2)return snapshot('store:2',[],false);
    if(stage===3)return snapshot('reopened:0',[]);
    if(stage===4)return snapshot('reopened:1',[row(metadata(makeFull('new'),'reopened:1'),1)]);
    throw Error('bad stage');
  });
  let history=[];const apply=rows=>{applies++;history=rows};
  await refresh(apply);assert.equal(history.length,15003);assert.equal(history[0].id,'15002');assert.equal(seen[0],null);
  assert(history.every(isFlowMetadata));assert(!JSON.stringify(history).includes('bodyBase64'),'four entity payload fields are absent from the entire history');
  stage=1;await refresh(apply);assert.equal(history.length,15002);assert(!history.some(item=>item.id==='8'));assert.equal(history.find(item=>item.id==='7').bodyRevision,'store:20000');assert.equal(seen.at(-1),'store:1');
  stage=2;await refresh(apply);const applied=applies;await refresh(apply);assert.equal(applies,applied,'idle polls do not rebuild history');
  stage=3;await refresh(apply);assert.deepEqual(history,[],'database reopen/reset clears every obsolete row');
  stage=4;epoch++;await refresh(apply);assert.equal(seen.at(-1),null,'external replacement/import requests a complete metadata reset');assert.deepEqual(history.map(item=>item.id),['new']);

  const old=deferred(),newer=deferred();let requests=0,raceHistory=[];
  const racing=createHistoryRefresh(()=>epoch,()=>++requests===1?old.promise:newer.promise);
  const first=racing(rows=>{raceHistory=rows});epoch++;const second=racing(rows=>{raceHistory=rows});
  newer.resolve(snapshot('new:1',[row(metadata(makeFull('new'),'new:1'))]));await second;
  old.resolve(snapshot('old:1',[row(metadata(makeFull('old'),'old:1'))]));await first;assert.deepEqual(raceHistory.map(item=>item.id),['new'],'late history cannot overwrite a new epoch');
  const late404=deferred();let call=0;const raceCommands=[];
  const crossMode=createHistoryRefresh(()=>0,async(command)=>{raceCommands.push(command);return ++call===1?late404.promise:snapshot('modern:2',[row(metadata(makeFull('modern'),'modern:2'))])});
  let crossHistory=[];const ancient=crossMode(rows=>{crossHistory=rows});await crossMode(rows=>{crossHistory=rows});
  late404.reject(Error('HTTP 404: endpoint absent'));await ancient;await crossMode(rows=>{crossHistory=rows});
  assert(raceCommands.every(command=>command==='flow_metadata_changes'),'a superseded missing-endpoint response cannot downgrade a newer modern session');assert.equal(crossHistory[0].id,'modern');
  let retryCalls=[],throwApply=true;
  const retry=createHistoryRefresh(()=>0,async(command,args)=>{retryCalls.push(args.since);return snapshot('retry:1',[row(meta)])});
  await assert.rejects(()=>retry(()=>{if(throwApply)throw Error('render failed')}),/render failed/);throwApply=false;await retry(()=>{});assert.deepEqual(retryCalls,[null,null],'failed apply does not advance the revision');
  let mutableEpoch=0,hold=deferred(),nextSince=[];
  const invalidated=createHistoryRefresh(()=>mutableEpoch,async(command,args)=>{nextSince.push(args.since);return nextSince.length===1?hold.promise:snapshot('new-epoch:1',[])});
  let staleApply=0;const pending=invalidated(()=>staleApply++);mutableEpoch++;hold.resolve(snapshot('old-epoch:1',[row(meta)]));await pending;assert.equal(staleApply,0);await invalidated(()=>{});assert.deepEqual(nextSince,[null,null]);

  for(const error of ['HTTP 401: unauthorized','HTTP 403: denied','HTTP 400: bad JSON','HTTP 500: not found','request timeout','Recorded flow not found','Unknown command: other_command','body mentions HTTP 404']){
    const attempted=[];const unavailable=createHistoryRefresh(()=>0,async command=>{attempted.push(command);throw Error(error)});
    await assert.rejects(()=>unavailable(()=>assert.fail('failed polls preserve prior history')),candidate=>String(candidate).includes(error));assert.deepEqual(attempted,['flow_metadata_changes']);
  }
  assert(isMissingFlowEndpoint(Error('HTTP 404: route absent'),'flow_metadata_changes'));assert(isMissingFlowEndpoint('Command flow_metadata_changes not found','flow_metadata_changes'));
  const legacyCalls=[];
  const legacy=createHistoryRefresh(()=>0,async command=>{
    legacyCalls.push(command);
    if(command==='flow_metadata_changes')throw Error('Unknown command: flow_metadata_changes');
    if(command==='flow_changes')return snapshot('legacy:1',[row(full)]);
    assert.fail('unexpected legacy route '+command);
  });
  let legacyHistory;await legacy(rows=>{legacyHistory=rows});assert.equal(legacyHistory[0],full);assert.deepEqual(legacyCalls,['flow_metadata_changes','flow_changes']);
  const oldestCalls=[];const oldest=createHistoryRefresh(()=>0,async command=>{oldestCalls.push(command);if(command==='list_flows')return [full];throw Error('HTTP 404: unavailable')});
  await oldest(rows=>assert.deepEqual(rows,[full]));await oldest(()=>{});assert.deepEqual(oldestCalls,['flow_metadata_changes','flow_changes','flow_revision','list_flows','flow_revision','list_flows']);
  const snapshotCalls=[];assert.deepEqual(await loadHistorySnapshot(async command=>{snapshotCalls.push(command);if(command==='list_flows')return [full];throw Error('HTTP 404: absent')}),[full]);assert.deepEqual(snapshotCalls,['flow_metadata_changes','list_flows']);
  console.log('Flow records: all 15,003 metadata rows, four absent body slots, exact pinned actions, prefix/missing/malformed rejection, delta/delete/reset/reopen, apply/epoch/concurrency races and endpoint-only legacy fallback passed.');
})().catch(error=>{console.error(error);process.exitCode=1});
