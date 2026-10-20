const assert=require('node:assert/strict');
const {build}=require('esbuild');
let current;
const react={
  useState(initial){const h=current,i=h.index++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return[h.slots[i],value=>{if(h.disposed){h.lateUpdates++;return}const next=typeof value==='function'?value(h.slots[i]):value;if(!Object.is(next,h.slots[i])){h.slots[i]=next;h.changed=true}}]},
  useRef(initial){const h=current,i=h.index++;return h.slots[i]??={current:initial}},
  useMemo(calculate,deps){const h=current,i=h.index++,old=h.slots[i];if(!old||deps.some((v,j)=>v!==old.deps[j]))h.slots[i]={value:calculate(),deps};return h.slots[i].value},
  useEffect(effect,deps){const h=current,i=h.index++,old=h.slots[i];if(!old||deps.some((v,j)=>v!==old[j])){h.effects.push(()=>{h.cleanups[i]?.();h.cleanups[i]=effect()});h.slots[i]=deps}},
};
const jsx=(type,props={})=>({type,props});
const text=node=>Array.isArray(node)?node.map(text).join(''):node===null||node===undefined||node===false?'':typeof node==='object'?text(node.props?.children):String(node);
const nodes=node=>Array.isArray(node)?node.flatMap(nodes):node&&typeof node==='object'?[node,...nodes(node.props?.children)]:[];
function harness(Component,props){
  const h={slots:[],index:0,effects:[],cleanups:[],props,tree:null,changed:false,disposed:false,lateUpdates:0};
  h.render=()=>{let limit=0;do{assert(limit++<30,'effects converge');h.changed=false;current=h;h.index=0;h.tree=Component(h.props);h.effects.splice(0).forEach(effect=>effect())}while(h.changed);return h.tree};
  h.all=predicate=>nodes(h.tree).filter(predicate);h.button=label=>h.all(n=>n.type==='button'&&text(n)===label)[0];
  h.click=label=>{const button=h.button(label);assert(button,label);assert(!button.props.disabled,label+' is enabled');button.props.onClick();h.render()};
  h.tick=async()=>{await new Promise(resolve=>setImmediate(resolve));h.render()};
  h.dispose=()=>{h.cleanups.forEach(cleanup=>cleanup?.());h.disposed=true};h.render();return h;
}
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
const SIZE=65536;
const flow=(id='a',total=3,revision='instance:1')=>({id,bodyRevision:revision,bodyReferences:{request:{byteLength:0},response:{byteLength:total},originalResponse:{byteLength:total}}});
const page=(bytes,total=bytes.length,offset=0)=>({offset,totalBytes:total,bodyBase64:Buffer.from(bytes).toString('base64'),nextOffset:offset+bytes.length<total?offset+bytes.length:null});
(async()=>{
  const output=await build({entryPoints:['apps/frontend/src/RecordedBody.tsx'],bundle:true,platform:'node',format:'cjs',write:false,packages:'external',jsx:'automatic',loader:{'.css':'empty'},plugins:[{name:'recorded-body-ui',setup(builder){
    builder.onLoad({filter:/[/\\]api\.ts$/},()=>({contents:'export const invoke=(...args)=>globalThis.__bodyInvoke(...args);export const downloadFlowBody=(...args)=>globalThis.__bodyDownload(...args);'}));
    builder.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export const t=(key,args={})=>key.replace(/\\{(v[0-9]+)\\}/g,(_,name)=>String(args[name]??""));'}));
    builder.onLoad({filter:/[/\\]ResponseBody\.tsx$/},()=>({contents:'export default function ResponseBody(){throw Error("the complete-body worker is never executed by this harness")};'}));
  }}]});
  const module={exports:{}};new Function('module','exports','require',output.outputFiles[0].text)(module,module.exports,name=>name==='react'?react:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:require(name));const Component=module.exports.default;
  const calls=[],downloads=[],copied=[];let respond=async()=>page(Buffer.from('abc')),downloadResult=async()=>({kind:'saved',name:'body.bin',buffered:false});
  global.__bodyInvoke=(command,args)=>{calls.push([command,args]);assert.equal(command,'read_flow_body');return respond(args)};
  global.__bodyDownload=(...args)=>{downloads.push(args);return downloadResult(...args)};
  const props={flow:flow(),variant:'response',headers:[],onCopy:value=>copied.push(value)};
  const inactive=harness(Component,{...props,active:false});assert.equal(inactive.tree,null);assert.equal(calls.length,0);inactive.dispose();
  const offline=harness(Component,{...props,connected:false});assert(text(offline.tree).includes('内核已断开'));assert.equal(calls.length,0);offline.dispose();
  const small=harness(Component,props);assert.deepEqual(calls.at(-1),['read_flow_body',{id:'a',variant:'response',expectedVersion:'instance:1',offset:0,length:SIZE}]);await small.tick();
  const complete=small.all(n=>typeof n.type==='function'&&n.type.name==='ResponseBody');assert.equal(complete.length,1);assert.equal(complete[0].props.body,Buffer.from('abc').toString('base64'));
  small.props={...props,active:false};small.render();assert.equal(small.tree,null,'hidden bodies unmount the full decoder');assert(!small.slots.some(slot=>slot?.page),'hidden bodies release the cached page');small.dispose();
  const raw=harness(Component,{...props,mode:'raw'});await raw.tick();assert.equal(raw.all(n=>typeof n.type==='function').length,0);raw.click('复制当前片段');assert.equal(copied.at(-1),'abc');raw.dispose();
  const base64=harness(Component,{...props,mode:'base64'});await base64.tick();assert.equal(base64.all(n=>typeof n.type==='function').length,0);base64.click('复制当前片段');assert.equal(copied.at(-1),'YWJj');base64.dispose();

  const bigBytes=Buffer.alloc(SIZE+4,65);bigBytes.write('TAIL',SIZE);respond=async args=>page(bigBytes.subarray(args.offset,args.offset+SIZE),bigBytes.length,args.offset);
  const big=harness(Component,{...props,flow:flow('big',bigBytes.length),headers:[{name:'Content-Type',value:'image/svg+xml'}]});await big.tick();
  assert.equal(big.all(n=>typeof n.type==='function').length,0,'a syntactically valid prefix is never sent to preview/format workers');
  assert(text(big.tree).includes('不会解压、格式化或预览截断内容'));assert(big.button('上一片段').props.disabled);big.click('复制当前片段');assert.equal(copied.at(-1).length,SIZE);
  big.click('下一片段');assert.equal(calls.at(-1)[1].offset,SIZE);await big.tick();assert(big.button('下一片段').props.disabled);assert.equal(big.all(n=>n.type==='pre')[0].props.children,'TAIL','the second range replaces the first rather than accumulating');
  big.click('复制当前片段');assert.equal(copied.at(-1),'TAIL');assert(text(big.tree).includes('第 2 / 2 页 · 字节偏移 65536'));
  big.click('下载完整原始实体');await big.tick();assert.deepEqual(downloads.at(-1)[0],{id:'big',variant:'response',expectedVersion:'instance:1'});assert.equal(downloads.at(-1)[1].expectedBytes,SIZE+4);assert(downloads.at(-1)[1].signal instanceof AbortSignal);assert(text(big.tree).includes('已保存：body.bin'));
  big.props={...props,flow:flow('big',3,'instance:2')};respond=async()=>page(Buffer.from('new'));big.render();await big.tick();assert.equal(calls.at(-1)[1].offset,0,'a revision change resets the page');assert.equal(calls.at(-1)[1].expectedVersion,'instance:2');assert(!text(big.tree).includes('TAIL'));big.dispose();

  respond=async()=>page(Uint8Array.of(0,255,65));const binary=harness(Component,{...props,mode:'raw'});await binary.tick();assert(binary.button('原始文本片段').props.disabled);assert.equal(binary.all(n=>n.type==='pre')[0].props.children,'00000000  00 ff 41');binary.click('Base64');binary.click('复制当前片段');assert.equal(copied.at(-1),'AP9B');binary.dispose();
  respond=async()=>page(Buffer.from('abc'));const encoded=harness(Component,{...props,mode:'raw',headers:[{name:'Content-Encoding',value:'gzip'}]});await encoded.tick();assert(encoded.button('原始文本片段').props.disabled);assert(text(encoded.tree).includes('片段保留内容编码'));encoded.dispose();

  const old=deferred(),fresh=deferred();respond=args=>args.id==='old'?old.promise:fresh.promise;
  const stale=harness(Component,{...props,flow:flow('old')});stale.props={...props,flow:flow('fresh')};stale.render();fresh.resolve(page(Buffer.from('NEW')));await stale.tick();old.resolve(page(Buffer.from('OLD')));await stale.tick();assert.equal(stale.all(n=>typeof n.type==='function')[0].props.body,'TkVX','late selection responses cannot replace current bytes');
  const original=deferred();respond=()=>original.promise;stale.props={...props,flow:flow('fresh'),variant:'originalResponse'};stale.render();assert.equal(calls.at(-1)[1].variant,'originalResponse');stale.props={...stale.props,connected:false};stale.render();original.resolve(page(Buffer.from('OLD')));await stale.tick();assert(text(stale.tree).includes('内核已断开'));assert.equal(stale.all(n=>typeof n.type==='function').length,0);assert(!stale.slots.some(slot=>slot?.page),'disconnection releases cached bytes');
  respond=async()=>page(Buffer.from('NOW'));stale.props={...stale.props,connected:true};stale.render();await stale.tick();assert.equal(stale.all(n=>typeof n.type==='function')[0].props.body,'Tk9X');stale.dispose();
  const late=deferred();respond=()=>late.promise;const unmounted=harness(Component,props);unmounted.dispose();late.resolve(page(Buffer.from('L8R')));await new Promise(resolve=>setImmediate(resolve));assert.equal(unmounted.lateUpdates,0,'unmounted requests never update state');

  let metadataReloads=0;respond=async()=>{throw Error('version conflict')};const failed=harness(Component,{...props,onReloadMetadata:()=>metadataReloads++});await failed.tick();assert(failed.all(n=>n.props?.role==='alert').length);assert(text(failed.tree).includes('重新读取记录信息后重试'));failed.click('重新读取记录信息');assert.equal(metadataReloads,1);respond=async()=>page(Buffer.from('abc'));failed.click('重读当前片段');await failed.tick();assert.equal(failed.all(n=>n.props?.role==='alert').length,0);failed.dispose();

  const pendingDownload=deferred();downloadResult=()=>pendingDownload.promise;const saving=harness(Component,props);await saving.tick();saving.click('下载完整原始实体');const downloadSignal=downloads.at(-1)[1].signal;assert(saving.button('下载完整原始实体').props.disabled);saving.props={...props,connected:false};saving.render();assert(downloadSignal.aborted,'disconnect aborts the fetch/writer download');pendingDownload.resolve({kind:'browser',buffered:true});await saving.tick();assert(!text(saving.tree).includes('Blob'));saving.props=props;saving.render();await saving.tick();assert(!saving.button('下载完整原始实体').props.disabled,'aborting a download cannot leave a reconnect permanently busy');downloadResult=async()=>({kind:'browser',buffered:true});saving.click('下载完整原始实体');await saving.tick();assert(text(saving.tree).includes('此浏览器使用完整 Blob 缓冲，不保证低内存'),'fallback memory behavior is visible to the user');saving.dispose();
  const beforeMissing=calls.length;const missing=harness(Component,{...props,variant:'originalRequest'});await missing.tick();assert.equal(calls.length,beforeMissing,'an absent slot is never represented as an empty body');assert(missing.button('下载完整原始实体').props.disabled);missing.dispose();
  respond=async()=>page(Buffer.alloc(0));const empty=harness(Component,{...props,variant:'request'});await empty.tick();assert.equal(calls.at(-1)[1].variant,'request');assert.equal(empty.all(n=>typeof n.type==='function').length,0,'empty bodies validate their revision without starting a worker');assert(text(empty.tree).includes('（空正文）'));empty.dispose();

  const jumpTotal=SIZE*1234+5;respond=async args=>page(Buffer.alloc(Math.min(SIZE,jumpTotal-args.offset),65),jumpTotal,args.offset);
  const jumping=harness(Component,{...props,flow:flow('many-pages',jumpTotal)});await jumping.tick();
  const jumpInput=()=>jumping.all(n=>n.type==='input'&&n.props['aria-label']==='跳转到正文页码')[0];
  const typePage=value=>{jumpInput().props.onChange({target:{value}});jumping.render()};
  const submitPage=()=>{jumping.all(n=>n.type==='form'&&n.props.className==='recorded-body-jump')[0].props.onSubmit({preventDefault(){}});jumping.render()};
  const beforeTyping=calls.length;for(const value of ['1','12','123','1235']){typePage(value);assert(!jumpInput().props.disabled);assert.equal(calls.length,beforeTyping,'typing a multi-digit page never reads')}
  submitPage();assert.equal(calls.length,beforeTyping+1);assert.equal(calls.at(-1)[1].offset,SIZE*1234);await jumping.tick();assert(text(jumping.tree).includes('第 1235 / 1235 页'));assert.equal(jumping.all(n=>n.type==='pre')[0].props.children.length,5,'jumping retains only the last page');
  for(const value of ['0','-1','1.5','1e2','1236','','Infinity']){const count=calls.length;typePage(value);submitPage();assert.equal(calls.length,count,'invalid page '+value+' never reads');assert(jumpInput().props['aria-invalid']);assert(text(jumping.tree).includes('请输入 1 到 1235 的整数页码。'))}
  typePage('99');jumping.props={...props,flow:flow('many-pages',jumpTotal,'instance:2')};jumping.render();await jumping.tick();assert.equal(jumpInput().props.value,'1','new revisions cannot reuse a prior jump draft');
  typePage('77');jumping.props={...jumping.props,variant:'originalResponse'};jumping.render();await jumping.tick();assert.equal(jumpInput().props.value,'1','new body variants cannot reuse a prior jump draft');assert(!jumping.button('加载完整正文用于格式化或预览'));assert(text(jumping.tree).includes('处理上限为 8 MiB'));jumping.dispose();

  const fullBytes=Buffer.alloc(SIZE*2+5,66);fullBytes.write('final',SIZE*2);const fullProps={...props,flow:flow('explicit-full',fullBytes.length)};
  const readFull=args=>page(fullBytes.subarray(args.offset,args.offset+SIZE),fullBytes.length,args.offset);respond=async args=>readFull(args);
  const beforeComplete=calls.length,processing=harness(Component,fullProps);await processing.tick();assert.equal(calls.length,beforeComplete+1,'the default view reads one range only');assert.equal(processing.all(n=>typeof n.type==='function').length,0);
  processing.click('加载完整正文用于格式化或预览');await processing.tick();assert.equal(calls.length,beforeComplete+4,'explicit processing reads all three exact selected-entity ranges');assert.equal(processing.all(n=>typeof n.type==='function').length,1);assert.equal(processing.all(n=>typeof n.type==='function')[0].props.body,fullBytes.toString('base64'));assert(text(processing.tree).includes('已完整读取所选实体'));
  processing.click('下一片段');await processing.tick();assert.equal(processing.all(n=>typeof n.type==='function').length,0,'page changes release the complete processing cache');assert(!processing.slots.some(slot=>slot?.state==='ready'&&slot.body));processing.dispose();
  const failedProcessing=harness(Component,fullProps);await failedProcessing.tick();let fullRead=0;respond=async args=>{if(++fullRead===2)throw Error('version conflict midway');return readFull(args)};failedProcessing.click('加载完整正文用于格式化或预览');await failedProcessing.tick();assert.equal(fullRead,2);assert.equal(failedProcessing.all(n=>typeof n.type==='function').length,0,'failed complete reads never format their successful prefix');assert(text(failedProcessing.tree).includes('完整正文读取失败，未使用部分内容。'));failedProcessing.click('复制当前片段');assert.equal(copied.at(-1).length,SIZE);failedProcessing.dispose();
  respond=async args=>readFull(args);const processingRace=harness(Component,fullProps);await processingRace.tick();const fullPending=deferred();respond=()=>fullPending.promise;processingRace.click('加载完整正文用于格式化或预览');respond=async args=>readFull(args);processingRace.props={...fullProps,flow:flow('explicit-full',fullBytes.length,'instance:9')};processingRace.render();await processingRace.tick();fullPending.resolve(readFull({offset:0}));await processingRace.tick();assert.equal(processingRace.all(n=>typeof n.type==='function').length,0,'late complete reads cannot mount a worker for a newer revision');assert(!processingRace.slots.some(slot=>slot?.state==='ready'&&slot.body));processingRace.dispose();
  assert(calls.every(([command,args])=>command==='read_flow_body'&&args.length===SIZE),'body views issue only bounded range commands, never complete get_flow');
  console.log('Recorded body UI: activation gates, single-page replacement, numeric page jumps/draft resets, explicit complete-only processing, empty-body worker gate, binary/raw views, stale/unmount guards and pinned downloads passed');
})().catch(error=>{console.error(error);process.exit(1)});
