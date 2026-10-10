const assert = require('node:assert/strict');
const { buildSync } = require('esbuild');
const bundle = buildSync({entryPoints:['apps/frontend/src/flowRefresh.ts'], bundle:true, platform:'node',format:'cjs',write:false});
const m={exports:{}};new Function('module','exports',bundle.outputFiles[0].text)(m,m.exports);
(async () => {
  let revision='a:0', reads=0, applies=0, fail=false, race=false;
  const refresh=m.exports.createFlowRefresh(async()=>revision,async()=>{
    reads++;if(fail)throw Error('disconnected');if(race){revision='a:2';race=false;}return Array.from({length:12001},(_,i)=>i);
  });
  const apply=rows=>{applies++;assert.equal(rows.length,12001);};
  await refresh(apply); await refresh(apply);assert.equal(reads,1);assert.equal(applies,1);
  revision='a:1';fail=true;await assert.rejects(()=>refresh(apply));fail=false;race=true;
  await refresh(apply);await refresh(apply);assert.equal(reads,4); // concurrent write triggers another read
  await refresh(apply);assert.equal(reads,4);
  revision='b:2';await refresh(apply);assert.equal(reads,5); // service restart
  revision='b:3';await assert.rejects(()=>refresh(()=>{throw Error('apply failed');}));await refresh(apply);assert.equal(reads,7);
  let legacyReads=0;
  const legacy=m.exports.createFlowRefresh(async()=>{throw Error('404');},async()=>{legacyReads++;return [];});
  await legacy(()=>{});await legacy(()=>{});assert.equal(legacyReads,2);
  console.log('Flow refresh: full history, idle reads skipped, failed read/apply retries, concurrent writes, restart and legacy fallback passed.');
})().catch(e=>{console.error(e);process.exitCode=1;});
