const assert=require('node:assert/strict');
const {isRunnerAllocationFailure:eligible,retry}=require('./ci-retry.cjs');
const annotations=[{annotation_level:'failure',message:'The job was not acquired by Runner of type hosted even after multiple attempts'}];
const job={conclusion:'cancelled',runner_name:'',steps:[]};
assert(eligible(job,annotations));
assert(!eligible({...job,runner_name:'runner'},annotations));
assert(!eligible({...job,steps:[{name:'Build',conclusion:'failure'}]},annotations));
assert(!eligible(job,[{annotation_level:'notice',message:'capacity constraints'}]));
assert(!eligible(job,[]));
assert(!eligible({...job,conclusion:'timed_out'},annotations));
(async()=>{
 for(const scenario of ['retry','max','newer','running','test_failure']){
  let calls=0;const run={id:1,name:'Check',conclusion:'failure',head_branch:'main',head_repository:{full_name:'a/b'},event:'push',head_sha:'abc',run_attempt:scenario==='max'?3:1};
  const listJobs=()=>{},listAnnotations=()=>{};
  const github={rest:{actions:{getWorkflowRun:async()=>({data:{...run,status:scenario==='running'?'in_progress':'completed'}}),listJobsForWorkflowRunAttempt:listJobs,reRunWorkflowFailedJobs:async()=>{calls++}},repos:{getBranch:async()=>({data:{commit:{sha:scenario==='newer'?'xyz':'abc'}}})},checks:{listAnnotations}},paginate:async fn=>fn===listJobs?[{...job,steps:scenario==='test_failure'?[{}]:[],check_run_url:'https://api.github.com/repos/a/b/check-runs/2'}]:annotations};
  const summary={addHeading(){return this},addRaw(){return this},async write(){}};
  await retry({github,context:{repo:{owner:'a',repo:'b'},payload:{workflow_run:run}},core:{info(){},summary}});
  assert.equal(calls,scenario==='retry'?1:0,scenario);
 }
 console.log('CI retry: allocation-only, retry cap, stale commit, active run and code failure guards passed.');
})().catch(e=>{console.error(e);process.exitCode=1});
