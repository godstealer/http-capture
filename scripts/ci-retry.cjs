const MAX_ATTEMPTS = 3;
function isRunnerAllocationFailure(job, annotations) {
  return ['failure', 'cancelled'].includes(job.conclusion)
    && !job.runner_name && (job.steps || []).length === 0
    && annotations.some(a => a.annotation_level === 'failure' && /The job was not acquired by Runner of type hosted even after multiple attempts/i.test(a.message || ''));
}
async function retry({github, context, core}) {
  const run = context.payload.workflow_run;
  if (!run || !['Check','Release'].includes(run.name) || !['failure','cancelled'].includes(run.conclusion)
      || run.head_branch !== 'main' || run.head_repository?.full_name !== `${context.repo.owner}/${context.repo.repo}`
      || !['push','workflow_run'].includes(run.event)) return;
  // Read live state: an operator may have already requested another attempt.
  const {data: current} = await github.rest.actions.getWorkflowRun({...context.repo,run_id:run.id});
  if(current.status !== 'completed' || current.run_attempt !== run.run_attempt || current.run_attempt >= MAX_ATTEMPTS) {
    core.info('No retry: already restarted or maximum attempts reached.');return;
  }
  const {data: branch} = await github.rest.repos.getBranch({...context.repo,branch:'main'});
  if(branch.commit.sha !== run.head_sha){core.info('No retry: main has moved to a newer commit.');return;}
  const jobs=await github.paginate(github.rest.actions.listJobsForWorkflowRunAttempt,{...context.repo,run_id:run.id,attempt_number:run.run_attempt,per_page:100});
  const failed=jobs.filter(j=>['failure','cancelled','timed_out','action_required'].includes(j.conclusion));
  if(!failed.length)return;
  for(const job of failed){
    const checkId=Number(new URL(job.check_run_url).pathname.split('/').pop());
    const annotations=await github.paginate(github.rest.checks.listAnnotations,{...context.repo,check_run_id:checkId,per_page:100});
    if(!isRunnerAllocationFailure(job,annotations)){core.info(`No retry: ${job.name} is not a confirmed runner allocation failure.`);return;}
  }
  await github.rest.actions.reRunWorkflowFailedJobs({...context.repo,run_id:run.id});
  await core.summary.addHeading('Automatic infrastructure retry').addRaw(`Run ${run.id}: requested attempt ${run.run_attempt+1}/${MAX_ATTEMPTS}. Only failed jobs are retried.`).write();
}
module.exports={retry,isRunnerAllocationFailure};
