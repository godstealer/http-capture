const assert = require('node:assert/strict');
const {buildSync,transformSync} = require('esbuild');
const moduleValue = {exports:{}};
new Function('module','exports',buildSync({entryPoints:['apps/frontend/src/codeExport.ts'],bundle:true,platform:'node',format:'cjs',write:false}).outputFiles[0].text)(moduleValue,moduleValue.exports);
const {codeTargets,generateRequestCode} = moduleValue.exports;
assert.equal(new Set(codeTargets.map(t=>t.id)).size,codeTargets.length);
assert(codeTargets.some(t=>t.id==='python-requests'));
assert(codeTargets.some(t=>t.id==='node-native'));
const request = {method:'POST',url:'https://example.invalid/a?x=%2F&y=1',tls:{preset:'chrome'},headers:[
 {name:'Host',value:'outdated.invalid'},{name:'Content-Length',value:'1'},
 {name:'X-Disabled',value:'must-not-be-exported',disabled:true},
 {name:'Connection',value:'X-Hop'},{name:'X-Hop',value:'hop-value'},
 {name:'X-Order',value:'first'},{name:'x-order',value:'second'},
 {name:'Cookie',value:'a=1'},{name:'cookie',value:'b=2'},
 {name:'X-Empty',value:''},{name:'X-Quoted',value:'quote " \\ $ </script> : 中文'},
 {name:'Content-Type',value:'application/octet-stream'},
],bodyBase64:Buffer.from([0,255,128,13,10,39,34,92]).toString('base64')};
const original=JSON.stringify(request);
for(const target of codeTargets){
 const result=generateRequestCode(request,target.id);
 assert.equal(result.target,target);
 assert(result.code.length>40,target.id);
 assert(!result.code.includes('must-not-be-exported'),target.id);
 assert(!result.code.includes('outdated.invalid'),target.id);
 assert(!result.code.includes('hop-value'),target.id);
 assert(result.warnings.includes('framing'));
 assert.equal(result.warnings.includes('duplicates'),!target.preservesDuplicates,target.id);
 assert.equal(result.warnings.includes('browser'),!!target.browser,target.id);
 if(target.language==='JavaScript'||target.language==='Node.js')transformSync(result.code,{loader:'js',format:'esm',target:'es2022'});
 assert.equal(JSON.stringify(request),original,target.id);
 const empty=generateRequestCode({...request,method:'GET',bodyBase64:''},target.id);
 assert(empty.code.length>40,target.id);
 assert.equal(JSON.stringify(request),original);
}
const duplicates=generateRequestCode(request,'python-requests').code;
assert(duplicates.includes('a=1; b=2'));
assert(duplicates.includes('first, second'));
assert(!generateRequestCode(request,'python-http-client').warnings.includes('duplicates'));
for(const target of codeTargets.filter(t=>t.browser||/fetch/i.test(t.id)))
 assert.throws(()=>generateRequestCode({...request,method:'GET'},target.id),/GET\/HEAD body unsupported/,target.id);
assert.throws(()=>generateRequestCode(request,'nonexistent'),/Unknown code target/);
assert.throws(()=>generateRequestCode({...request,url:'file:///secret'},'python-requests'),/Invalid HTTP URL/);
assert.throws(()=>generateRequestCode({...request,method:'POST\r\nInjected: true'},'python-requests'),/Invalid HTTP method/);
assert.throws(()=>generateRequestCode({...request,headers:[{name:'X-Test',value:'ok\r\nInjected: true'}]},'python-requests'),/Invalid HTTP header/);
assert.throws(()=>generateRequestCode({...request,headers:[{name:'',value:'orphan'}]},'python-requests'),/Invalid HTTP header/);
assert.throws(()=>generateRequestCode({...request,bodyBase64:'%%%%'},'python-requests'),/Invalid Base64 body/);
const disabledConnection=generateRequestCode({...request,headers:[{name:'Connection',value:'X-Test',disabled:true},{name:'X-Test',value:'retained'}]},'python-requests');
assert(disabledConnection.code.includes('retained'));
for (const base64 of ['/w', '/w==\n', ' / w = = ']) {
 for(const target of codeTargets.filter(t=>t.id!=='shell-curl')) {
  const result=generateRequestCode({...request,bodyBase64:base64},target.id);
  assert(result.code.includes('/w=='),`${target.id}: Base64 must use canonical padding without whitespace`);
 }
}
assert.throws(()=>generateRequestCode({...request,headers:[{name:'X-Test',value:'invalid\x01control'}]},'python-requests'),/Invalid HTTP header/);
assert(generateRequestCode({...request,method:'HEAD',bodyBase64:''},'shell-curl').code.includes('curl --head '));
assert.throws(()=>generateRequestCode({...request,method:'HEAD'},'shell-curl'),/HEAD body unsupported/);
console.log(`Code export: ${codeTargets.length} language/library targets, exact binary inputs, disabled/framing fields, duplicate warnings, empty bodies and injection validation passed.`);
