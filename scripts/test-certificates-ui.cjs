const assert=require('node:assert/strict');
const {build}=require('esbuild');
let current;
const react={
 useState(initial){const h=current,i=h.index++;if(!(i in h.slots))h.slots[i]=initial;return[h.slots[i],v=>{const next=typeof v==='function'?v(h.slots[i]):v;if(!Object.is(next,h.slots[i])){h.slots[i]=next;h.changed=true}}]},
 useRef(initial){const h=current,i=h.index++;return h.slots[i]??={current:initial}},
 useEffect(fn,deps){const h=current,i=h.index++,old=h.slots[i];if(!old||deps.some((v,j)=>v!==old[j])){h.effects.push(()=>{h.cleanups[i]?.();h.cleanups[i]=fn()});h.slots[i]=deps}},
};
const jsx=(type,props={})=>{if(typeof type==='string'&&props.ref&&!props.ref.current)props.ref.current={value:''};return{type,props}};
const nodes=n=>Array.isArray(n)?n.flatMap(nodes):n&&typeof n==='object'?[n,...nodes(n.props.children)]:[];
const text=n=>Array.isArray(n)?n.map(text).join(''):n==null||n===false?'':typeof n==='object'?text(n.props.children):String(n);
function harness(Component){const h={slots:[],index:0,effects:[],cleanups:[],changed:false,props:{connected:true,active:true}};
 h.render=()=>{let count=0;do{assert(++count<20);h.changed=false;current=h;h.index=0;h.tree=Component(h.props);h.effects.splice(0).forEach(f=>f())}while(h.changed)};
 h.find=p=>nodes(h.tree).find(p);h.button=label=>h.find(n=>n.type==='button'&&text(n)===label);h.field=label=>h.find(n=>n.type==='label'&&text(n).startsWith(label)).props.children.find(n=>n?.type==='input');
 h.tick=async()=>{await new Promise(r=>setImmediate(r));h.render()};h.render();return h;
}
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return{promise,resolve}};
const file=(name,content)=>({name,size:content.length,arrayBuffer:async()=>new TextEncoder().encode(content).buffer});
(async()=>{
 const result=await build({entryPoints:['apps/frontend/src/Certificates.tsx'],bundle:true,platform:'node',format:'cjs',write:false,jsx:'automatic',packages:'external',loader:{'.css':'empty'},plugins:[{name:'certificate-boundaries',setup(b){
  b.onLoad({filter:/[/\\]api\.ts$/},()=>({contents:'export const invoke=(...args)=>globalThis.__certificateInvoke(...args)'}));
  b.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export const t=x=>x'}));
 }}]});const m={exports:{}};new Function('module','exports','require',result.outputFiles[0].text)(m,m.exports,n=>n==='react'?react:n==='react/jsx-runtime'?{jsx,jsxs:jsx}:require(n));
 const calls=[],initial={entries:[],version:'v1',selectedCa:null,activeCa:null,restartRequired:false};
 let response=async()=>structuredClone(initial);global.__certificateInvoke=async(...args)=>{calls.push(args);return response(...args)};
 const h=harness(m.exports.default);await h.tick();assert.equal(calls.length,1);
 h.field('名称').props.onChange({target:{value:'Local CA'}});h.render();
 // Synthetic file contents: the real PEM/key validation is covered in Rust.
 h.field('公钥证书文件').props.onChange({target:{files:[file('ca.pem','certificate')]}});h.field('配套私钥文件').props.onChange({target:{files:[file('key.pem','-----BEGIN TEST MATERIAL-----\nprivate-fixture\n-----END TEST MATERIAL-----')]}});await h.tick();
 assert(!h.button('导入并保存').props.disabled);let pending=deferred();response=()=>pending.promise;
 h.button('导入并保存').props.onClick();h.button('导入并保存').props.onClick();assert.equal(calls.length,2,'double click sends one mutation');assert.equal(calls[1][1].input.expectedVersion,'v1');assert.match(calls[1][1].input.privateKeyPem,/private-fixture/);
 const ca={id:'ca-1',name:'Local CA',kind:'ca',enabled:false,host:'',port:443,certificate:{subject:'Local CA',issuer:'Local CA',sha256:'abc',notAfter:'2030'}};
 pending.resolve({...initial,entries:[ca],version:'v2'});await h.tick();assert(h.button('导入并保存').props.disabled,'successful import clears file contents');assert(!text(h.tree).includes('private-fixture'));
 h.button('切换为此 CA').props.onClick();h.render();assert.equal(calls.length,2,'selection requires confirmation');h.button('取消').props.onClick();h.render();assert.equal(calls.length,2);
 h.button('切换为此 CA').props.onClick();h.render();response=async()=>({...initial,entries:[ca],version:'v3',selectedCa:ca.id,restartRequired:true});h.button('确认').props.onClick();await h.tick();assert.equal(calls[2][1].input.expectedVersion,'v2');assert(text(h.tree).includes('重启内核后生效'));assert(h.button('删除').props.disabled,'selected CA cannot be deleted');
 h.button('客户端证书').props.onClick();h.render();h.field('精确域名或 IP').props.onChange({target:{value:'api.example.com'}});h.field('端口').props.onChange({target:{value:'44a3'}});h.render();assert.equal(h.field('端口').props.value,'443');
 h.field('公钥证书文件').props.onChange({target:{files:[file('client.pem','client')]}});await h.tick();const late=deferred();h.field('配套私钥文件').props.onChange({target:{files:[{name:'late.key',size:20,arrayBuffer:()=>late.promise}]}});
 h.button('服务器信任').props.onClick();h.render();late.resolve(new TextEncoder().encode('-----BEGIN TEST MATERIAL-----private').buffer);await h.tick();assert(!h.find(n=>n.type==='input'&&n.props.accept==='.pem,.key'),'trust form never asks for a private key');
 h.button('客户端证书').props.onClick();h.render();assert(h.button('导入并保存').props.disabled,'late private file does not survive category change');
 h.field('配套私钥文件').props.onChange({target:{files:[file('encrypted.key','-----BEGIN ENCRYPTED TEST MATERIAL-----')]}});await h.tick();assert(text(h.tree).includes('未加密的 PEM'));
 const readCount=calls.length;h.props={...h.props,active:false};h.render();assert.equal(h.tree,null);await h.tick();assert.equal(calls.length,readCount,'hidden settings do not poll certificates');
 response=async()=>initial;h.props={active:true,connected:false};h.render();assert(text(h.tree).includes('连接内核后'));assert.equal(calls.length,readCount);
 console.log('Certificate UI: imports, confirmation, version binding, private file clearing, races and disconnected state passed.');
})().catch(e=>{console.error(e);process.exitCode=1});
