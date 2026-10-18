const assert=require('node:assert/strict');const {buildSync}=require('esbuild');
function load(p){const r=buildSync({entryPoints:[p],bundle:true,platform:'node',format:'cjs',write:false});const m={exports:{}};new Function('module','exports',r.outputFiles[0].text)(m,m.exports);return m.exports;}
const {resolveEnvironment,parseLibrary,withSavedRequest,suggestedRequestName}=load('apps/frontend/src/library.ts');const {createFlowChanges}=load('apps/frontend/src/flowChanges.ts');
global.crypto ??= require('node:crypto').webcrypto;
const draft={url:'https://{{host}}/{{path}}',method:'POST',headers:[{name:'X-A',value:'{{token}}'},{name:'X-A',value:'literal'}],bodyBase64:Buffer.from('{{token}}').toString('base64'),tls:{preset:'native'},scripts:{variables:{token:'local'}}};const original=JSON.stringify(draft);const resolved=resolveEnvironment(draft,{host:'example.invalid',path:'api',token:'global'});assert.equal(resolved.url,'https://example.invalid/api');assert.equal(resolved.headers[0].value,'local');assert.equal(Buffer.from(resolved.bodyBase64,'base64').toString(),'local');assert.equal(JSON.stringify(draft),original);assert.throws(()=>resolveEnvironment(draft,{}),/Undefined variable/);assert.throws(()=>parseLibrary('{}',0));
(async()=>{let result;const changes=[{revision:'a:1',reset:true,rows:[{position:1,flow:{id:'1'}},{position:2,flow:{id:'2'}}],deleted:[]},{revision:'a:2',reset:false,rows:[{position:1,flow:{id:'1',updated:true}},{position:3,flow:{id:'3'}}],deleted:['2']}];const refresh=createFlowChanges(async()=>changes.shift());await refresh(0,r=>result=r);assert.deepEqual(result.map(r=>r.id),['2','1']);await refresh(0,r=>result=r);assert.deepEqual(result.map(r=>r.id),['3','1']);assert(result[1].updated);console.log('Library variable isolation, overrides, missing variables and incremental ordering/deletion passed');})().catch(e=>{console.error(e);process.exitCode=1});

const valid={format:'http-capture-library',version:1,library:{revision:88,collections:[{id:'c',name:'Collection'}],requests:[],environments:[{id:'e',name:'Environment',variables:{host:'example.invalid'}}],scripts:[]}};
assert.equal(parseLibrary(JSON.stringify(valid),2).revision,2);
for(const mutate of [v=>v.library.environments[0].variables={host:3},v=>v.library.collections.push({id:'c',name:'Duplicate'}),v=>v.library.requests.push({id:'r',name:'Bad',collectionId:'missing',draft:{}})]){const bad=structuredClone(valid);mutate(bad);assert.throws(()=>parseLibrary(JSON.stringify(bad),0));}

const disabledDraft={...draft,url:'https://example.invalid/api',scripts:{...draft.scripts,enabled:false,before:'',after:''},headers:[
 {name:'X-A',value:'{{token}}'},
 {name:'{{undefinedHeaderName}}',value:'{{undefinedHeaderValue}}',disabled:true},
 {name:'X-A',value:'{{token}}',disabled:false},
 {name:'X-A',value:'last'},
]};
const disabledBefore=JSON.stringify(disabledDraft);
const resolvedDisabled=resolveEnvironment(disabledDraft,{});
assert.deepEqual(resolvedDisabled.headers,[
 {name:'X-A',value:'local'},
 {name:'{{undefinedHeaderName}}',value:'{{undefinedHeaderValue}}',disabled:true},
 {name:'X-A',value:'local',disabled:false},
 {name:'X-A',value:'last'},
]);
assert.equal(JSON.stringify(disabledDraft),disabledBefore);
const reenabled=structuredClone(disabledDraft);reenabled.headers[1].disabled=false;
assert.throws(()=>resolveEnvironment(reenabled,{}),/Undefined variable: undefinedHeaderName/);

// The reusable library JSON keeps disabled rows and their positions so reloading
// a saved request does not silently re-enable or lose them.
const saved=structuredClone(valid);
saved.library.requests=[{id:'r-disabled',name:'Saved headers',collectionId:'c',draft:disabledDraft}];
const reloaded=parseLibrary(JSON.stringify(saved),9);
assert.equal(reloaded.revision,9);
assert.deepEqual(reloaded.requests[0].draft.headers,disabledDraft.headers);
assert.deepEqual(resolveEnvironment(reloaded.requests[0].draft,{}).headers,resolvedDisabled.headers);
for(const invalid of ['true',1,null]){
 const bad=structuredClone(saved);bad.library.requests[0].draft.headers[1].disabled=invalid;
 assert.throws(()=>parseLibrary(JSON.stringify(bad),0),/Invalid saved request/);
}
console.log('Disabled headers: unresolved variables stay editable, enabled variables resolve, saved library state/order survive reload and invalid flags are rejected.');

// Saving from the request page preserves local library edits and header/body data.
const beforeQuickSave = structuredClone(reloaded);
beforeQuickSave.environments[0].variables.pending = 'local edit';
const beforeQuickSaveJson = JSON.stringify(beforeQuickSave);
const quick = withSavedRequest(beforeQuickSave, disabledDraft, {name:'  My request  ', collectionName:'  My requests  '});
assert.equal(JSON.stringify(beforeQuickSave), beforeQuickSaveJson);
assert.equal(quick.library.revision, beforeQuickSave.revision, 'CAS revision belongs to the backend');
assert.equal(quick.library.collections.length, beforeQuickSave.collections.length + 1);
const firstSaved = quick.library.requests.find(r => r.id === quick.requestId);
assert.equal(firstSaved.name, 'My request');
assert.equal(quick.collectionName, 'My requests');
assert.deepEqual(firstSaved.draft, disabledDraft);
assert.deepEqual(quick.library.environments, beforeQuickSave.environments);
assert.deepEqual(quick.library.scripts, beforeQuickSave.scripts);
const changedDraft = structuredClone(disabledDraft);
changedDraft.headers[0].value = 'changed';
const updated = withSavedRequest(quick.library, changedDraft, {name:firstSaved.name,collectionId:firstSaved.collectionId}, quick.requestId);
assert.equal(updated.requestId, quick.requestId);
assert.equal(updated.library.requests.length, quick.library.requests.length);
assert.equal(updated.library.collections.length, quick.library.collections.length);
assert.equal(updated.library.requests.find(r => r.id === quick.requestId).draft.headers[0].value, 'changed');
assert.equal(firstSaved.draft.headers[0].value, '{{token}}', 'saved snapshots must stay isolated');
const copied = withSavedRequest(updated.library, changedDraft, {name:'Copy',collectionId:firstSaved.collectionId});
assert.notEqual(copied.requestId, quick.requestId);
assert.equal(copied.library.requests.length, updated.library.requests.length + 1);
assert.throws(()=>withSavedRequest(quick.library, changedDraft,{name:'Missing',collectionId:'removed'}),/Collection not found/);
assert.throws(()=>withSavedRequest(quick.library, changedDraft,{name:'Deleted',collectionId:firstSaved.collectionId},'removed'),/Saved request not found/);
assert.throws(()=>withSavedRequest(quick.library, changedDraft,{name:'  ',collectionId:firstSaved.collectionId}),/Invalid request name/);
assert.throws(()=>withSavedRequest(quick.library, changedDraft,{name:'x',collectionName:' '}),/Invalid collection name/);
assert.throws(()=>withSavedRequest(quick.library, changedDraft,{name:'测'.repeat(171),collectionId:firstSaved.collectionId}),/Invalid request name/);
assert.throws(()=>withSavedRequest(quick.library, changedDraft,{name:'x',collectionName:'测'.repeat(171)}),/Invalid collection name/);
assert.equal(suggestedRequestName({...changedDraft,url:'https://example.invalid/search?token=private#secret'}),'POST example.invalid/search', 'names exclude query credentials and fragments');
const longName = suggestedRequestName({...changedDraft,url:'{host}/'+'a'.repeat(116)+'🚀'});
assert(!/[\uD800-\uDBFF]$/.test(longName));
assert.equal(parseLibrary(JSON.stringify({format:'http-capture-library',version:1,library:updated.library}),12).requests.length,updated.library.requests.length);
console.log('Direct save: default collection, update without duplicates, save as, snapshots, local edits, deleted targets, byte limits and suggested names passed.');

const grpcContext={requestMode:'grpc',grpcDraft:{method:'/test.Service/Call',json:'{\n  "name": "{{user}}"\n'}};
const grpcSaved=withSavedRequest(quick.library,changedDraft,{name:'gRPC message',collectionId:firstSaved.collectionId},undefined,grpcContext);
grpcContext.grpcDraft.json='changed afterwards';
const grpcReloaded=parseLibrary(JSON.stringify({format:'http-capture-library',version:1,library:grpcSaved.library}),15);
const savedGrpc=grpcReloaded.requests.find(r=>r.id===grpcSaved.requestId);
assert.equal(savedGrpc.requestMode,'grpc');
assert.equal(savedGrpc.grpcDraft.method,'/test.Service/Call');
assert.equal(savedGrpc.grpcDraft.json,'{\n  "name": "{{user}}"\n');
for(const invalidContext of [{requestMode:'ftp'},{grpcDraft:{method:3,json:'{}'}},{grpcDraft:null}]){
 const bad=structuredClone(grpcReloaded);Object.assign(bad.requests.find(r=>r.id===grpcSaved.requestId),invalidContext);
 assert.throws(()=>parseLibrary(JSON.stringify({format:'http-capture-library',version:1,library:bad}),0),/Invalid saved request/);
}
console.log('Saved gRPC messages/methods and request modes survive library export/import; invalid metadata is rejected.');
