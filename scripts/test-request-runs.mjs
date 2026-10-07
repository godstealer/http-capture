import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
const source=ts.transpileModule(fs.readFileSync(new URL('../apps/frontend/src/requestRuns.ts',import.meta.url),'utf8'),{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText;
const {RequestRuns,applyRequestResult}=await import('data:text/javascript;base64,'+Buffer.from(source).toString('base64'));
const pending=new Map(), cancellations=[], errors=[], results=[];
let next=0, preparations=0;
const invoke=async(command,args)=>{
  if(command==='prepare_replay'){preparations++;return `run-${++next}`;}
  if(command==='cancel_replay'){cancellations.push(args.executionId);return true;}
  return new Promise(resolve=>pending.set(args.executionId,resolve));
};
const runs=new RequestRuns(invoke,()=>{},e=>errors.push(e));
const a=runs.start('A',{url:'a'},null,f=>results.push(['A',f]));
const b=runs.start('B',{url:'b'},null,f=>results.push(['B',f]));
await runs.start('A',{url:'duplicate'},null,()=>assert.fail('duplicate'));
await Promise.resolve(); assert.equal(preparations,2);
await runs.cancel('A');assert.deepEqual(cancellations,['run-1']);assert.equal(runs.state('B').cancelling,false);
pending.get('run-2')({id:'b'});await b;pending.get('run-1')({id:'a',error:'cancelled'});await a;
assert.deepEqual(results.map(x=>x[0]),['B','A']);assert.equal(runs.state('A'),undefined);assert.deepEqual(errors,[]);
let acknowledge;const calls=[];
const early=new RequestRuns(async(c,args)=>{
 calls.push(c);if(c==='prepare_replay')return new Promise(resolve=>acknowledge=resolve);
 if(c==='replay_request')return {id:args.executionId,error:'cancelled'};return true;
},()=>{},e=>assert.fail(e));
const started=early.start('closed',{},null,()=>{});await early.cancel('closed');acknowledge('reserved');await started;
assert.deepEqual(calls,['prepare_replay','cancel_replay','replay_request']);
console.log('Request runs: parallel completion, duplicate prevention, independent cancellation and cancellation before acknowledgement passed.');

const draft={url:'original'}, updated={url:'edited'}, returned={id:'flow-A',request:{url:'script-modified'}};
const editors=[{id:'A',draft:updated,selected:null},{id:'B',draft:{url:'other'},selected:null}];
const applied=applyRequestResult(editors,'A',JSON.stringify(draft),returned);
assert.deepEqual(applied[0].draft,updated);assert.equal(applied[0].selected,'flow-A');assert.equal(applied[1],editors[1]);
assert.deepEqual(applyRequestResult([], 'closed', JSON.stringify(draft), returned),[]);
assert.equal(applyRequestResult([{id:'A',draft}], 'A',JSON.stringify(draft),returned)[0].draft.url,'script-modified');
console.log('Result routing: original tab, edits preserved, script updates and closed tabs passed.');
