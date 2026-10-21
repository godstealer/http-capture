const groups={
 'windows-linux':{checks:['frontend','capture (windows-latest)','capture (ubuntu-latest)'],matrix:[{os:'windows-latest',label:'windows-x64',bundles:'nsis'},{os:'ubuntu-22.04',label:'linux-x64',bundles:'deb,appimage'}]},
 macos:{checks:['frontend','capture (macos-26)'],matrix:[{os:'macos-26',label:'macos-arm64',bundles:'app,dmg'},{os:'macos-15-intel',label:'macos-x64',bundles:'app,dmg'}]},
};
function platformReleaseSource(run,jobs,group,repository){
 const selected=Object.hasOwn(groups,group)?groups[group]:null;
 if(!selected)throw Error('Unknown platform release group');
 if(run.name!=='Check'||run.path!=='.github/workflows/check.yml'||run.event!=='push'||run.head_branch!=='main'||run.head_repository?.full_name!==repository)throw Error('Expected the trusted main push Check workflow');
 if(!/^[a-f0-9]{40}$/.test(run.head_sha)||!Number.isSafeInteger(run.run_number)||run.run_number<1||!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(run.created_at))throw Error('Invalid source identity');
 for(const name of selected.checks){const matches=jobs.filter(job=>job.name===name);if(matches.length!==1||matches[0].status!=='completed'||matches[0].conclusion!=='success')throw Error(`Required platform source check has not succeeded: ${name}`)}
 return{sha:run.head_sha,number:run.run_number,createdAt:run.created_at,matrix:selected.matrix};
}
module.exports={platformReleaseSource};
