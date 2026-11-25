const assert = require('node:assert/strict');
const {build} = require('esbuild');
let current;
const react = {
  useState(initial) {
    const h=current, index=h.index++;
    if (!(index in h.slots)) h.slots[index]=typeof initial==='function'?initial():initial;
    return [h.slots[index], value=>{
      if (h.disposed) h.lateWrites++;
      const next=typeof value==='function'?value(h.slots[index]):value;
      if (!Object.is(next,h.slots[index])) { h.slots[index]=next; h.changed=true; }
    }];
  },
  useRef(initial) { const h=current,index=h.index++; return h.slots[index]??={current:initial}; },
  useMemo(fn,deps) {
    const h=current,index=h.index++,old=h.slots[index];
    if (!old||deps.length!==old.deps.length||deps.some((value,i)=>!Object.is(value,old.deps[i]))) h.slots[index]={value:fn(),deps};
    return h.slots[index].value;
  },
  useEffect(fn,deps) {
    const h=current,index=h.index++,old=h.slots[index];
    if (!old||deps.length!==old.length||deps.some((value,i)=>!Object.is(value,old[i]))) {
      h.effects.push(()=>{h.cleanups[index]?.();h.cleanups[index]=fn();}); h.slots[index]=deps;
    }
  },
};
function deferred() { let resolve,reject; const promise=new Promise((a,b)=>{resolve=a;reject=b;}); return {promise,resolve,reject}; }
function harness(hook,props) {
  const h={props,slots:[],index:0,effects:[],cleanups:[],changed:false,disposed:false,lateWrites:0};
  h.render=()=>{let limit=0;do{assert(++limit<50,'effects converge');h.changed=false;current=h;h.index=0;h.value=hook(h.props.flows,h.props.query,h.props.active);h.effects.splice(0).forEach(fn=>fn());}while(h.changed);return h.value;};
  h.update=props=>{h.props={...h.props,...props};return h.render();};
  h.flush=async()=>{await new Promise(resolve=>setImmediate(resolve));if(!h.disposed)h.render();};
  h.dispose=()=>{h.disposed=true;h.cleanups.forEach(fn=>fn?.());}; h.render();return h;
}
function fakeClock() {
  let now=0,sequence=0;const tasks=new Map();
  const real={setTimeout,clearTimeout,dateNow:Date.now};
  global.setTimeout=(callback,delay=0)=>{const id=++sequence;tasks.set(id,{at:now+delay,callback});return id;};
  global.clearTimeout=id=>tasks.delete(id); Date.now=()=>now;
  return {
    async advance(ms,h) {
      const end=now+ms;let iterations=0;
      for (;;) {
        const first=[...tasks].sort((a,b)=>a[1].at-b[1].at||a[0]-b[0]).find(([,task])=>task.at<=end);
        if(!first)break;assert(++iterations<1000,'timer loop converges');now=first[1].at;tasks.delete(first[0]);first[1].callback();await h.flush();
      }
      now=end;await h.flush();
    },
    restore(){global.setTimeout=real.setTimeout;global.clearTimeout=real.clearTimeout;Date.now=real.dateNow;},
  };
}
const key=flow=>`${flow.id}:${flow.bodyRevision}`;
const flow=(id,revision='rev1',meta='')=>({id,bodyRevision:revision,response:{status:200,headers:[]},meta});
(async()=>{
  const output=await build({entryPoints:['apps/frontend/src/useFlowSearch.ts'],bundle:true,platform:'node',format:'cjs',write:false,packages:'external',plugins:[{name:'search-hook-fixture',setup(builder){builder.onLoad({filter:/[/\\]flowSearch\.ts$/},()=>({contents:`export const flowSearchIdentity=flow=>flow.id+':'+flow.bodyRevision;export const flowSearchText=flow=>flow.meta||'';export const searchFlowResponse=(...args)=>globalThis.__searchFlowResponse(...args);`}));}}]});
  const moduleShim={exports:{}};new Function('module','exports','require',output.outputFiles[0].text)(moduleShim,moduleShim.exports,name=>name==='react'?react:require(name));
  const hook=moduleShim.exports.useFlowSearch;
  const reads=[];global.__searchFlowResponse=(record,query,{signal})=>{const result=deferred();reads.push({record,query,signal,...result});return result.promise;};
  const clock=fakeClock();
  try {
    const first=flow('first');let items=[first];
    const h=harness(hook,{flows:items,query:'needle',active:true});
    for(let index=1;index<=4;index++){await clock.advance(50,h);items=[...items,flow('incoming'+index)];h.update({flows:items});}
    assert.equal(reads.length,0,'typing waits for the query debounce');
    await clock.advance(50,h);assert.equal(reads.length,1,'continuous incoming captures cannot restart the query debounce');
    const initial=reads[0];assert.equal(initial.record.id,'first');
    items=[...items.map(item=>structuredClone(item)),flow('incoming5')];h.update({flows:items});
    assert.equal(initial.signal.aborted,false,'adding unrelated captures preserves the existing body read');assert.equal(reads.length,1,'response scans remain serial');
    initial.resolve('match');await h.flush();
    assert(h.value.matches.has(key(first)),'completed reads publish matches while new captures continue');
    assert.equal(reads.length,2);const oldPending=reads.at(-1);
    h.update({query:'next'});assert(oldPending.signal.aborted,'editing the query cancels its old pending read');
    assert.equal(h.value.matches.size,0,'old-query matches disappear immediately');
    const beforeNext=reads.length;await clock.advance(249,h);assert.equal(reads.length,beforeNext,'new query typing is independently debounced');await clock.advance(1,h);const nextRead=reads.at(-1);assert.equal(nextRead.query,'next');assert.equal(nextRead.record.id,'first');
    oldPending.resolve('match');await h.flush();assert.equal(h.value.matches.size,0,'late old-query reads cannot publish into the new query');
    const changed=flow('first','rev2');items=items.map(item=>item.id==='first'?changed:item);h.update({flows:items});
    assert(nextRead.signal.aborted,'a late old-query finally must not erase the newer read cancellation handle');
    nextRead.resolve('match');await h.flush();
    const revisedRead=reads.at(-1);assert.equal(revisedRead.record.bodyRevision,'rev2');assert.equal(h.value.matches.size,0,'old-revision results are never shown');
    revisedRead.resolve('match');await h.flush();await clock.advance(100,h);assert(h.value.matches.has(key(changed)),'completed matches publish within the progress interval even when the next read is stalled');
    const leaving=reads.at(-1);assert.notEqual(leaving,revisedRead);
    h.update({active:false});assert(leaving.signal.aborted,'leaving capture cancels its body read');assert.equal(h.value.matches.size,0);
    leaving.resolve('match');await h.flush();assert.equal(h.value.matches.size,0,'late reads cannot repopulate an inactive capture view');h.dispose();

    const before=reads.length,disabled=harness(hook,{flows:[flow('disabled')],query:'needle',active:false});
    await clock.advance(300,disabled);assert.equal(reads.length,before);disabled.update({active:true,query:''});await clock.advance(300,disabled);assert.equal(reads.length,before,'empty and inactive queries do not read response bodies');disabled.dispose();
    const header=harness(hook,{flows:[flow('headers','rev1','needle in response-header')],query:'needle',active:true});await clock.advance(300,header);assert.equal(reads.length,before,'metadata matches never fetch response bodies');assert.equal(header.value.total,0);header.dispose();
    const unmount=harness(hook,{flows:[flow('unmount')],query:'needle',active:true});await clock.advance(250,unmount);const unmounted=reads.at(-1);unmount.dispose();assert(unmounted.signal.aborted);unmounted.resolve('match');await unmount.flush();assert.equal(unmount.lateWrites,0,'unmount cleanup prevents late state writes');
    console.log('Capture response search hook: query-only debounce, live capture progress, serial reads, query/revision cancellation, inactive/unmounted guards and header fast path passed.');
  } finally {clock.restore();delete global.__searchFlowResponse;}
})().catch(error=>{console.error(error);process.exitCode=1;});
