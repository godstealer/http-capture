const assert=require('node:assert/strict');
const {build}=require('esbuild');
let current;
class Element{value='';clicks=0;click(){this.clicks++}}
global.crypto??=require('node:crypto').webcrypto;
const react={
  useState(initial){const h=current,i=h.index++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return[h.slots[i],value=>{const next=typeof value==='function'?value(h.slots[i]):value;if(!Object.is(next,h.slots[i])){h.slots[i]=next;h.changed=true;}}]},
  useRef(initial){const h=current,i=h.index++;return h.slots[i]??={current:initial}},
  useId(){const h=current,i=h.index++;return h.slots[i]??=`rewrite-ui-${i}`},
  useMemo(calculate,deps){const h=current,i=h.index++,old=h.slots[i];if(!old||deps.some((v,j)=>v!==old.deps[j]))h.slots[i]={value:calculate(),deps};return h.slots[i].value},
  useEffect(effect,deps){const h=current,i=h.index++,old=h.slots[i];if(!old||deps.some((v,j)=>v!==old[j])){h.effects.push(()=>{h.cleanups[i]?.();h.cleanups[i]=effect()});h.slots[i]=deps}},
};
const jsx=(type,props={})=>{if(typeof type==='string'&&props.ref&&!props.ref.current)props.ref.current=new Element();return {type,props}};
function text(node){if(Array.isArray(node))return node.map(text).join('');if(node===null||node===undefined||node===false)return '';return typeof node==='object'?text(node.props?.children):String(node)}
function nodes(node){if(Array.isArray(node))return node.flatMap(nodes);return node&&typeof node==='object'?[node,...nodes(node.props?.children)]:[]}
async function load(){
  const output=await build({entryPoints:['apps/frontend/src/ResponseRewrite.tsx'],bundle:true,platform:'node',format:'cjs',write:false,packages:'external',jsx:'automatic',loader:{'.css':'empty'},plugins:[{name:'response-rewrite-ui',setup(builder){
    builder.onLoad({filter:/[/\\]api\.ts$/},()=>({contents:'export const invoke=(...args)=>globalThis.__rewriteInvoke(...args);'}));
    builder.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export const t=(key,args={})=>key.replace(/\\{(v[0-9]+)\\}/g,(_,name)=>String(args[name]??""));'}));
    builder.onLoad({filter:/[/\\]BodyCodeEditor\.tsx$/},()=>({contents:'export default function BodyCodeEditor(){}'}));
    builder.onLoad({filter:/[/\\]OrderedHeaders\.tsx$/},()=>({contents:'export default function OrderedHeaders(){}'}));
  }}]});
  const module={exports:{}};new Function('module','exports','require',output.outputFiles[0].text)(module,module.exports,name=>name==='react'?react:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:name==='lucide-react'?new Proxy({},{get:()=>()=>null}):require(name));return module.exports.default;
}
function harness(Component,props){
  const h={slots:[],index:0,effects:[],cleanups:[],props,tree:null,changed:false};
  h.render=()=>{let limit=0;do{assert(limit++<30,'effects converge');h.changed=false;current=h;h.index=0;h.tree=Component(h.props);h.effects.splice(0).forEach(effect=>effect())}while(h.changed);return h.tree};
  h.find=predicate=>nodes(h.tree).find(predicate);h.all=predicate=>nodes(h.tree).filter(predicate);
  h.button=label=>h.find(n=>n.type==='button'&&text(n)===label);h.input=label=>h.find(n=>n.props?.['aria-label']===label);
  h.tick=async()=>{await new Promise(resolve=>setImmediate(resolve));h.render()};h.dispose=()=>h.cleanups.forEach(cleanup=>cleanup?.());
  h.render();return h;
}
const change=(h,label,value)=>{h.input(label).props.onChange({target:{value}});h.render()};
const check=(h,label,value)=>{const parent=h.find(n=>n.type==='label'&&text(n)===label);assert(parent,label);nodes(parent).find(n=>n.type==='input').props.onChange({target:{checked:value}});h.render()};
const click=(h,label)=>{const button=h.button(label);assert(button,label);button.props.onClick();h.render()};
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
const encoded=value=>Buffer.from(value).toString('base64');
const snapshot=()=>({config:{enabled:false,scope:'all',rules:[]},version:'version-1',hits:[]});
const flow={id:'captured',request:{method:'POST',url:'https://example.test/api?a=b+c#hash'},response:{status:201,headers:[{name:'X-Repeat',value:'a'},{name:'x-repeat',value:'b'},{name:'Content-Type',value:'application/json'},{name:'Content-Encoding',value:'gzip'},{name:'ETag',value:'old'}],bodyBase64:encoded([0,255,128])}};
(async()=>{
  const Component=await load(),calls=[];let state=snapshot(),saveWork=async input=>({...state,version:'version-2',config:structuredClone(input.config),hits:input.config.rules.map(()=>0)});
  global.__rewriteInvoke=async(command,input)=>{calls.push([command,input]);if(command==='response_rewrite')return structuredClone(state);if(command==='configure_response_rewrite')return saveWork(input);throw Error('Unexpected command '+command)};
  const h=harness(Component,{connected:true,active:false,seed:{key:'seed-1',flow,action:'replace'}});
  assert.equal(calls.length,0,'hidden page does not read config');assert(h.tree.props.hidden);
  h.props={...h.props,active:true};h.render();await h.tick();
  assert.equal(calls.length,1);assert.equal(h.input('自动响应请求方法').props.value,'POST');assert.equal(h.input('替换正文编码').props.value,'base64');
  assert.equal(h.input('替换响应状态码').props.value,201);assert.equal(h.find(n=>n.type?.name==='BodyCodeEditor').props.value,flow.response.bodyBase64);
  assert.deepEqual(h.find(n=>n.type?.name==='OrderedHeaders').props.headers,flow.response.headers,'captured headers/order/duplicates prefill exactly');
  assert.equal(h.find(n=>n.type?.name==='OrderedHeaders').props.context,'response');
  assert.equal(h.find(n=>n.type==='label'&&text(n)==='启用此规则').props.children[0].props.checked,false);
  assert(text(h.tree).includes('已创建禁用的捕获模板草稿'));assert.equal(calls.filter(([command])=>command==='configure_response_rewrite').length,0,'seed never enables, saves or sends');
  h.render();assert.equal(h.all(n=>n.props?.className?.includes('rewrite-rule-row')).length,1,'same seed key is consumed once');
  h.props={...h.props,active:false};h.render();h.props={...h.props,active:true};h.render();assert.equal(calls.length,1,'navigation preserves the dirty draft and does not reload over it');
  change(h,'自动响应规则名称','Captured rule');change(h,'替换正文编码','text');assert(text(h.tree).includes('当前正文不是有效 UTF-8'));
  change(h,'替换正文编码','base64');h.find(n=>n.type?.name==='BodyCodeEditor').props.onChange(encoded('replacement'));h.render();
  change(h,'自动响应匹配内容','^https://example\\.test/api$');check(h,'启用此规则',true);check(h,'启用自动响应重写',true);change(h,'自动响应作用范围','replay');
  const pending=deferred();saveWork=()=>pending.promise;
  const saveButton=h.button('保存全部规则');saveButton.props.onClick();h.render();saveButton.props.onClick();
  const writes=()=>calls.filter(([command])=>command==='configure_response_rewrite');assert.equal(writes().length,1,'duplicate save clicks are rejected');assert.equal(writes()[0][1].expectedVersion,'version-1');
  assert.equal(writes()[0][1].config.rules[0].replacement.bodyBase64,encoded('replacement'));assert.equal(writes()[0][1].config.rules[0].enabled,true);assert.equal(writes()[0][1].config.enabled,true);assert.equal(writes()[0][1].config.scope,'replay');
  assert(h.find(n=>n.type?.name==='BodyCodeEditor').props.disabled,'CodeMirror is locked too, not only native fieldset controls');
  pending.reject(Error('配置版本冲突'));await h.tick();assert(text(h.tree).includes('配置版本冲突'));assert.equal(h.input('自动响应规则名称').props.value,'Captured rule');assert.equal(h.find(n=>n.type?.name==='BodyCodeEditor').props.value,encoded('replacement'));
  saveWork=async input=>{state={config:structuredClone(input.config),version:'version-2',hits:[0]};return structuredClone(state)};
  click(h,'保存全部规则');await h.tick();assert.equal(writes().at(-1)[1].expectedVersion,'version-1','failed writes never update CAS or blindly retry');assert(text(h.tree).includes('自动响应规则已保存'));assert(h.button('保存全部规则').props.disabled);
  h.input('复制响应规则 Captured rule').props.onClick();h.render();assert.equal(writes().length,2);assert.equal(h.input('自动响应规则名称').props.value,'Captured rule · 副本');
  assert.equal(h.find(n=>n.type==='label'&&text(n)==='启用此规则').props.children[0].props.checked,false,'copies default disabled');
  h.input('上移响应规则 Captured rule · 副本').props.onClick();h.render();const names=h.all(n=>n.type==='button'&&n.props.className==='rewrite-row-main').map(n=>text(n.props.children[0]));assert.deepEqual(names,['Captured rule · 副本','Captured rule']);
  h.input('删除响应规则 Captured rule · 副本').props.onClick();h.render();assert.equal(h.all(n=>n.props?.className?.includes('rewrite-rule-row')).length,1);
  click(h,'取消修改');assert.equal(h.all(n=>n.props?.className?.includes('rewrite-rule-row')).length,1);assert(h.button('保存全部规则').props.disabled);
  h.find(n=>n.type==='button'&&n.props?.className?.includes('rewrite-action-card')&&text(n).includes('修改部分内容')).props.onClick();h.render();
  change(h,'自动响应规则名称','Modify rule');change(h,'自动响应匹配内容','/modify');change(h,'查找内容 1','old');change(h,'替换内容 1','$1');
  const regex=h.find(n=>n.type==='label'&&text(n)==='使用正则表达式');nodes(regex).find(n=>n.type==='input').props.onChange({target:{checked:true}});h.render();change(h,'应用次数 1','first');
  click(h,'添加修改项');change(h,'修改目标 2','header');change(h,'响应头名称 2','X-Debug');change(h,'替换内容 2','updated');
  assert.equal(h.input('修改操作 2').props.value,'set');
  click(h,'保存全部规则');await h.tick();const modificationPayload=writes().at(-1)[1];assert.equal(modificationPayload.expectedVersion,'version-2');
  assert.deepEqual(modificationPayload.config.rules[1].modifications.map(item=>[item.target,item.operation,item.regex,item.all]),[['body','replace',true,false],['header','set',false,true]]);
  change(h,'响应头名称 2','Content-Length');const beforeInvalid=writes().length;click(h,'保存全部规则');assert.equal(writes().length,beforeInvalid);assert(text(h.tree).includes('由内核管理'));assert.equal(h.input('响应头名称 2').props.value,'Content-Length');
  const preserved=h.input('自动响应匹配内容').props.value;h.props={...h.props,seed:{key:'seed-2',flow,action:'replace'}};h.render();assert.equal(h.all(n=>n.props?.className?.includes('rewrite-rule-row')).length,3,'new captured seed appends without overwriting earlier dirty rules');
  h.find(n=>n.type==='button'&&n.props.className==='rewrite-row-main'&&text(n.props.children[0])==='Modify rule').props.onClick();h.render();assert.equal(h.input('自动响应匹配内容').props.value,preserved);assert.equal(h.input('响应头名称 2').props.value,'Content-Length');
  h.find(n=>n.type==='button'&&n.props.className==='rewrite-row-main'&&text(n.props.children[0])==='替换响应 · POST').props.onClick();h.render();
  const fileRead=deferred(),fileInput=h.find(n=>n.type==='input'&&n.props.type==='file');let readCalls=0;
  fileInput.props.onChange({target:{files:[{size:3,type:'application/octet-stream',arrayBuffer:()=>{readCalls++;return fileRead.promise}}],value:'chosen'}});h.render();assert.equal(readCalls,1);assert(h.find(n=>n.type?.name==='BodyCodeEditor').props.disabled);
  fileRead.resolve(Uint8Array.from([0,255,128]).buffer);await h.tick();assert.equal(h.find(n=>n.type?.name==='BodyCodeEditor').props.value,encoded([0,255,128]));assert.equal(h.find(n=>n.type?.name==='OrderedHeaders').props.headers.at(-1).value,'application/octet-stream');assert(text(h.tree).includes('文件已读入草稿'));
  h.find(n=>n.type==='input'&&n.props.type==='file').props.onChange({target:{files:[{size:8*1024*1024+1,arrayBuffer:()=>assert.fail('oversized file must not be read')}],value:'chosen'}});await h.tick();assert(text(h.tree).includes('8 MiB'));
  const late=deferred();h.find(n=>n.type==='input'&&n.props.type==='file').props.onChange({target:{files:[{size:1,type:'text/plain',arrayBuffer:()=>late.promise}],value:'chosen'}});h.dispose();const viewState=()=>JSON.stringify(h.slots.filter(value=>!value||typeof value!=='object'||!('current'in value)));const beforeUnmount=viewState();late.resolve(Uint8Array.from([65]).buffer);await new Promise(resolve=>setImmediate(resolve));assert.equal(viewState(),beforeUnmount,'late file read cannot update an unmounted editor');
  assert(calls.every(([command])=>['response_rewrite','configure_response_rewrite'].includes(command)),'editing rules never sends or replays network traffic');
  state=snapshot();saveWork=async input=>{state={config:structuredClone(input.config),version:'disabled-save-version',hits:[0]};return structuredClone(state)};
  const disabled=harness(Component,{connected:true,seed:{key:'disabled-seed',flow,action:'replace'}});await disabled.tick();
  click(disabled,'保存全部规则');await disabled.tick();assert.equal(state.config.enabled,false);assert.equal(state.config.rules[0].enabled,false);assert(text(disabled.tree).includes('总开关关闭，不会自动处理响应'),'ordinary saving explicitly explains disabled status');
  const acceptSave=saveWork;saveWork=async()=>{throw Error('Explicit enable conflict')};click(disabled,'保存并启用此规则');await disabled.tick();assert(text(disabled.tree).includes('Explicit enable conflict'));assert.equal(disabled.find(n=>n.type==='label'&&text(n)==='启用此规则').props.children[0].props.checked,false,'failed explicit enable leaves the disabled draft intact');assert.equal(state.config.enabled,false);
  saveWork=acceptSave;click(disabled,'保存并启用此规则');await disabled.tick();assert.equal(writes().at(-1)[1].expectedVersion,'disabled-save-version');assert.equal(state.config.enabled,true);assert.equal(state.config.rules[0].enabled,true);assert(text(disabled.tree).includes('后续匹配的响应生效'),'one explicit save-and-enable action enables both switches');disabled.dispose();
  global.__rewriteInvoke=async()=>{throw Error('Unknown command: response_rewrite')};const old=harness(Component,{connected:true});await old.tick();assert(text(old.tree).includes('当前内核不支持自动响应重写'));assert(old.button('保存全部规则').props.disabled);old.dispose();
  console.log('Response rewrite UI: real component seeds/navigation, explicit enable/CAS/conflict preservation, list editing, ordered modifications and bounded file lifecycle passed.');
})().catch(error=>{console.error(error);process.exitCode=1});
