const assert=require('node:assert/strict');
const {build}=require('esbuild');
let current;
const react={
  useState(initial){const h=current,i=h.index++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return [h.slots[i],value=>{h.slots[i]=typeof value==='function'?value(h.slots[i]):value}]},
  useRef(initial){const h=current,i=h.index++;return h.slots[i]??={current:initial}},
  useId(){const h=current,i=h.index++;return h.slots[i]??=('audit-test-'+i)},
  useEffect(effect,deps){const h=current,i=h.index++,previous=h.slots[i];if(!previous||deps.some((value,index)=>value!==previous[index])){h.effects.push(()=>{h.cleanups[i]?.();h.cleanups[i]=effect()});h.slots[i]=deps}},
};
const jsx=(type,props)=>({type,props:props??{}});
async function load(file){
  const result=await build({entryPoints:[file],bundle:true,platform:'node',format:'cjs',jsx:'automatic',write:false,packages:'external',loader:{'.css':'empty'},plugins:[{name:'audit-ui-contracts',setup(build){
    build.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export const t=(key,args={})=>key.replace(/\\{(v[0-9]+)\\}/g,(_,name)=>String(args[name]??""));'}));
    build.onLoad({filter:/[/\\]api\.ts$/},()=>({contents:'export const invoke=(...args)=>globalThis.__auditInvoke(...args);'}));
    build.onLoad({filter:/[/\\](?:McpClients|McpHelp|McpSetup)\.tsx$/},args=>({contents:'export default function '+args.path.split(/[/\\]/).at(-1).replace('.tsx','')+'(){}'}));
    build.onLoad({filter:/[/\\]clipboard\.ts$/},()=>({contents:'export const writeClipboard=()=>{throw Error("audit must not use clipboard")};'}));
  }}]});
  const module={exports:{}},intervals=[];
  new Function('module','exports','require','setInterval','clearInterval',result.outputFiles[0].text)(module,module.exports,name=>name==='react'?react:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:name==='lucide-react'?new Proxy({},{get:()=>()=>null}):require(name),fn=>{intervals.push(fn);return intervals.length},()=>{});
  return {Component:module.exports.default,intervals};
}
function text(node){if(node==null||node===false)return '';if(typeof node==='string'||typeof node==='number')return String(node);if(Array.isArray(node))return node.map(text).join('');return text(node.props?.children)}
function find(tree,predicate){const result=[];function walk(node){if(!node||typeof node!=='object')return;if(Array.isArray(node)){node.forEach(walk);return}if(predicate(node))result.push(node);walk(node.props?.children)}walk(tree);return result}
function harness(Component,props={}){
  const h={slots:[],index:0,effects:[],cleanups:[],tree:null,props};
  h.render=()=>{current=h;h.index=0;h.tree=Component(h.props);for(const effect of h.effects.splice(0))effect();return h.tree};
  h.find=predicate=>find(h.tree,predicate);h.button=name=>h.find(node=>node.type==='button'&&text(node)===name)[0];
  h.tick=async()=>{await new Promise(resolve=>setImmediate(resolve));h.render()};
  h.dispose=()=>{for(const cleanup of h.cleanups)cleanup?.()};h.render();return h;
}
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return {promise,resolve,reject}};
const entry=(id,overrides={})=>({id,startedAt:1792425600000,finishedAt:1792425600100,actor:'mcpTokenHolder',entryPoint:'mcpHttp',clientFamily:'codex',clientRef:'a'.repeat(64),operation:'update_request',targetKind:'requestEditor',targetId:'a95d9377-944d-48df-ae9a-aa18625d4d3c',outcome:'succeeded',reason:null,...overrides});
const page=(entries,nextCursor=null)=>({entries,nextCursor,retainedCount:4,retentionLimit:10000});
(async()=>{
  const calls=[],first=deferred();let respond=()=>first.promise;
  global.__auditInvoke=async(command,args)=>{calls.push([command,args]);assert.equal(command,'mcp_audit');return respond(args)};
  const auditModule=await load('apps/frontend/src/AuditPage.tsx');assert.equal(calls.length,0,'loading the module does not query the core');
  const audit=harness(auditModule.Component);audit.render();
  assert.deepEqual(calls,[['mcp_audit',{limit:50}]]);assert.equal(auditModule.intervals.length,0,'audit does not poll in the background');
  assert(audit.button('读取中…').props.disabled);assert(!text(audit.tree).includes('暂无 MCP 写操作记录'));
  first.resolve(page([entry('4'),entry('3',{outcome:'pending',finishedAt:null})],'3'));await audit.tick();
  assert(text(audit.tree).includes('结果未确认'));assert(text(audit.tree).includes('先确认实际状态，再决定是否重试。'));
  assert(text(audit.tree).includes('权限设置变更、读取工具和认证前拒绝不在本页范围内'));
  assert(text(audit.tree).includes('不代表已认证的个人身份'));assert(text(audit.tree).includes('不保存请求参数、正文、凭据、Token、变量或脚本源码'));
  assert.equal(audit.find(node=>node.type==='tbody')[0].props.children.length,2);
  assert.equal(audit.find(node=>node.type==='input'||node.type==='textarea').length,0,'audit is read-only');
  assert(!audit.find(node=>node.type==='button'&&/清空|删除|重试操作/.test(text(node))).length);
  const moreFailure=deferred();respond=()=>moreFailure.promise;
  audit.button('加载更早记录').props.onClick();audit.render();
  assert.deepEqual(calls.at(-1),['mcp_audit',{cursor:'3',limit:50}]);
  const pendingCalls=calls.length;audit.button('读取中…').props.onClick();assert.equal(calls.length,pendingCalls,'duplicate clicks cannot send overlapping queries');
  moreFailure.reject(Error('credential-in-transport-error'));await audit.tick();
  assert.equal(audit.find(node=>node.type==='tbody')[0].props.children.length,2,'failed pagination preserves existing entries');
  assert(audit.find(node=>node.props?.role==='alert').length);assert(!text(audit.tree).includes('credential-in-transport-error'),'raw transport errors are not echoed');
  respond=async()=>page([entry('3'),entry('2',{outcome:'rejected',reason:'permissionDenied'})],'2');
  audit.button('加载更早记录').props.onClick();await audit.tick();
  assert.deepEqual(calls.at(-1),['mcp_audit',{cursor:'3',limit:50}],'retry uses the unchanged cursor');
  assert.equal(audit.find(node=>node.type==='tbody')[0].props.children.length,3,'a duplicated entry is not rendered twice');
  assert(text(audit.tree).includes('没有所需操作权限'));
  respond=async()=>{throw Error('temporary offline')};audit.button('刷新审计').props.onClick();await audit.tick();
  assert.deepEqual(calls.at(-1),['mcp_audit',{limit:50}]);assert.equal(audit.find(node=>node.type==='tbody')[0].props.children.length,3,'failed refresh preserves the previous page');
  respond=async()=>page([entry('1',{clientFamily:'other',operation:'unknown-secret-source',targetKind:'unknown',outcome:'failed',reason:'conflict'})]);
  audit.button('刷新审计').props.onClick();await audit.tick();
  assert.equal(audit.find(node=>node.type==='tbody')[0].props.children.length,1,'successful refresh replaces prior pages');
  assert(text(audit.tree).includes('未知操作'));assert(!text(audit.tree).includes('unknown-secret-source'));
  assert(!audit.button('加载更早记录'));assert(!audit.find(node=>node.props?.role==='alert').length);
  const late=deferred();respond=()=>late.promise;audit.button('刷新审计').props.onClick();audit.dispose();
  const savedSlots=JSON.stringify(audit.slots);late.resolve(page([]));await new Promise(resolve=>setImmediate(resolve));assert.equal(JSON.stringify(audit.slots),savedSlots,'a late response does not update an unmounted page');

  global.__auditInvoke=async()=>{throw Error('HTTP 404: old core')};const old=harness(auditModule.Component);await old.tick();assert(text(old.tree).includes('当前内核不支持操作审计'));old.dispose();
  global.__auditInvoke=async()=>page([]);const empty=harness(auditModule.Component);await empty.tick();assert(text(empty.tree).includes('暂无 MCP 写操作记录'));empty.dispose();

  const parentCalls=[];global.__auditInvoke=async(command)=>{parentCalls.push(command);assert.equal(command,'mcp_status');return {config:{enabled:false,port:1422},running:false,address:null,lastError:null,toolNames:[]}};
  const parentModule=await load('apps/frontend/src/McpPage.tsx'),parent=harness(parentModule.Component);await parent.tick();
  assert(!parentCalls.includes('mcp_audit'),'the MCP server page does not load audit records');
  parent.button('操作审计').props.onClick();parent.render();
  assert(parent.find(node=>typeof node.type==='function'&&node.type.name==='AuditPage').length,'audit is an explicit MCP tab');
  assert(!parent.find(node=>node.props?.['aria-label']==='MCP 快速入门').length,'connection setup instructions are hidden on the audit tab');
  parent.button('AI 接入').props.onClick();parent.render();assert(!parent.find(node=>typeof node.type==='function'&&node.type.name==='AuditPage').length);parent.dispose();
  console.log('Audit UI: explicit tab/read-only cursor pages, safe status labels, failure preservation, no background polling, unmount guards and old-core hints passed');

  const faultCalls=[],initialFault=deferred();
  const faultConfig={enabled:false,scope:'capture',rules:[{id:'fault-rule',enabled:true,matchType:'urlContains',pattern:'/api',method:'',delayMs:1000,rateLimit:null}]};
  let readFault=()=>initialFault.promise,saveFault=async()=>{throw Error('configuration conflict')};
  global.__auditInvoke=async(command,args)=>{faultCalls.push([command,args]);if(command==='fault_rules')return readFault();if(command==='configure_fault_rules')return saveFault(args);throw Error('unexpected '+command)};
  const faultModule=await load('apps/frontend/src/FaultRulesPage.tsx'),fault=harness(faultModule.Component,{connected:false});
  assert.equal(faultCalls.length,0,'disconnected fault settings do not query or configure');
  assert(fault.find(node=>node.type==='fieldset')[0].props.disabled);
  fault.props={connected:true};fault.render();assert.deepEqual(faultCalls,[['fault_rules',undefined]]);
  initialFault.resolve({config:faultConfig,version:'version-1'});await fault.tick();
  const change=(node,value)=>{node.props.onChange({target:{value}});fault.render()};
  change(fault.find(node=>node.type==='input'&&node.props.value==='/api')[0],'/edited');
  change(fault.find(node=>node.type==='input'&&node.props.type==='number'&&node.props.value===1000)[0],'250');
  const limitSwitch=fault.find(node=>node.type==='label'&&text(node)==='限制请求频率')[0];
  find(limitSwitch,node=>node.type==='input')[0].props.onChange({target:{checked:true}});fault.render();
  change(fault.find(node=>node.type==='input'&&node.props.type==='number'&&node.props.value===5)[0],'7');
  change(fault.find(node=>node.type==='input'&&node.props.type==='number'&&node.props.value===1000)[0],'2000');
  assert.equal(faultCalls.filter(([command])=>command==='configure_fault_rules').length,0,'typing never saves configuration');
  assert(fault.button('重新加载').props.disabled,'reload cannot discard a dirty draft');
  fault.props={connected:false};fault.render();
  readFault=async()=>({config:{...faultConfig,rules:[{...faultConfig.rules[0],pattern:'/remote'}]},version:'version-2'});
  fault.props={connected:true};fault.render();await fault.tick();
  assert(fault.find(node=>node.type==='input'&&node.props.value==='/edited').length,'reconnection must preserve the dirty draft');
  fault.find(node=>node.type==='form')[0].props.onSubmit({preventDefault(){}});await fault.tick();
  const saved=faultCalls.find(([command])=>command==='configure_fault_rules')[1];
  assert.equal(saved.expectedVersion,'version-1','a dirty reconnect keeps its original CAS version');
  assert.equal(saved.config.rules[0].delayMs,250);assert.deepEqual(saved.config.rules[0].rateLimit,{requests:7,windowMs:2000});
  assert(fault.find(node=>node.type==='input'&&node.props.value==='/edited').length,'conflict keeps all typed fields');assert(text(fault.tree).includes('configuration conflict'));
  fault.button('取消修改').props.onClick();fault.render();fault.button('重新加载').props.onClick();fault.render();await fault.tick();
  assert(fault.find(node=>node.type==='input'&&node.props.value==='/remote').length,'explicit cancel and reload accepts the current remote configuration');
  change(fault.find(node=>node.type==='input'&&node.props.value==='/remote')[0],'/saved');
  saveFault=async args=>({config:structuredClone(args.config),version:'version-3'});
  fault.find(node=>node.type==='form')[0].props.onSubmit({preventDefault(){}});await fault.tick();
  assert.equal(faultCalls.filter(([command])=>command==='configure_fault_rules').at(-1)[1].expectedVersion,'version-2');
  assert(text(fault.tree).includes('模拟规则已保存；计数已重置，下一条请求生效。'));assert(fault.button('保存规则').props.disabled);fault.dispose();
  console.log('Fault UI: disconnected loading, explicit CAS saves, numeric options, conflict preservation and dirty reconnect guards passed');
})().catch(error=>{console.error(error);process.exit(1)});
