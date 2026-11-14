const assert = require('node:assert/strict');
const { build } = require('esbuild');
let active;
const react = {
  useState(initial) {
    const h=active,index=h.index++;
    if (!(index in h.hooks)) h.hooks[index]=typeof initial==='function'?initial():initial;
    return [h.hooks[index],value=>{
      if (h.disposed) { h.lateUpdates++; return; }
      const next=typeof value==='function'?value(h.hooks[index]):value;
      if (!Object.is(next,h.hooks[index])) { h.hooks[index]=next; h.changed=true; }
    }];
  },
  useRef(initial) { const h=active,index=h.index++; return h.hooks[index]??={current:initial}; },
  useMemo(calculate,deps) {
    const h=active,index=h.index++,old=h.hooks[index];
    if (!old || deps.some((value,i)=>!Object.is(value,old.deps[i]))) h.hooks[index]={value:calculate(),deps};
    return h.hooks[index].value;
  },
  useEffect(effect,deps) {
    const h=active,index=h.index++,old=h.hooks[index];
    if (!old || deps.some((value,i)=>!Object.is(value,old[i]))) {
      h.hooks[index]=deps;
      h.effects.push(()=>{h.cleanups[index]?.();h.cleanups[index]=effect();});
    }
  },
};
const jsx=(type,props={},key)=>({type,props,key});
const nodes=value=>Array.isArray(value)?value.flatMap(nodes):value&&typeof value==='object'?[value,...nodes(value.props?.children)]:[];
const text=value=>Array.isArray(value)?value.map(text).join(''):value===null||value===undefined||typeof value==='boolean'?'':typeof value==='object'?text(value.props?.children):String(value);
const encoded=value=>Buffer.from(value).toString('base64');
const decoded=value=>Buffer.from(value,'base64').toString();
const draft=(body='',mime='',extra={})=>({engine:'auto',method:'POST',url:'https://example.test/path',bodyBase64:encoded(body),tls:{preset:'native'},headers:[{name:'X-Keep',value:'first'},...(mime?[{name:'Content-Type',value:mime}]:[])],...extra});
function mount(Component,value) {
  const h={hooks:[],index:0,effects:[],cleanups:[],changes:[],changed:false,disposed:false,lateUpdates:0,dom:new Map(),focusCalls:[],selectCalls:[],scrollCalls:[],focused:null};
  h.props={draft:structuredClone(value),onChange:next=>{h.changes.push(next);h.props={...h.props,draft:next};}};
  // Commit callback refs after building the tree, with keyed row identity as
  // React would provide. Reattaching refs on each render intentionally detects
  // a focus implementation that steals focus again after ordinary typing.
  h.commitRefs=()=>{
    const present=new Set();
    function visit(value,parent='root',index=0,row=null) {
      if(Array.isArray(value)){value.forEach((child,i)=>visit(child,parent,i,row));return;}
      if(!value||typeof value!=='object')return;
      const path=parent+'/'+(value.key===undefined?`${index}:${typeof value.type==='string'?value.type:'component'}`:`key:${value.key}`);
      if(typeof value.type==='string') {
        present.add(path);
        let element=h.dom.get(path);
        if(!element) {
          element={
            type:value.type,
            focus(options){h.focused=element;h.focusCalls.push({element,options});},
            select(){h.selectCalls.push(element);},
            closest(selector){assert.equal(selector,'.request-body-row');return element.row;},
            scrollIntoView(options){h.scrollCalls.push({element,options});},
          };
          h.dom.set(path,element);
        }
        element.props=value.props;value.element=element;
        if(value.props.className?.split(' ').includes('request-body-row'))row=element;
        element.row=row;
        if(element.ref&&element.ref!==value.props.ref)element.ref(null);
        element.ref=value.props.ref;
        if(typeof value.props.ref==='function')value.props.ref(element);
      }
      visit(value.props?.children,path,0,row);
    }
    visit(h.tree);
    for(const [path,element] of h.dom)if(!present.has(path)){element.ref?.(null);h.dom.delete(path);}
  };
  h.render=()=>{
    let iterations=0;
    do {assert(iterations++<40,'component effects must converge');h.changed=false;active=h;h.index=0;h.tree=Component(h.props);h.commitRefs();h.effects.splice(0).forEach(effect=>effect());} while(h.changed);
    return h.tree;
  };
  h.all=predicate=>nodes(h.tree).filter(predicate);
  h.button=label=>h.all(n=>n.type==='button'&&text(n)===label)[0];
  h.input=(label,index=0)=>h.all(n=>(n.type==='input'||n.type==='select')&&n.props['aria-label']===label)[index];
  h.change=(label,value,index=0)=>{const node=h.input(label,index);assert(node,`${label} ${index} exists`);node.props.onChange({target:{value},currentTarget:{value}});h.render();};
  h.choose=value=>h.change('正文类型',value);
  h.click=label=>{const node=h.button(label);assert(node,`${label} button exists`);assert(!node.props.disabled,`${label} is enabled`);node.props.onClick();h.render();};
  h.raw=()=>h.all(n=>typeof n.type==='function'&&n.type.name==='BodyCodeEditor')[0];
  h.editRaw=value=>{assert(h.raw());h.raw().props.onChange(value);h.render();};
  h.tick=async()=>{await new Promise(resolve=>setImmediate(resolve));h.render();};
  h.settle=async()=>{for(let i=0;i<3;i++)await h.tick();};
  h.replace=value=>{h.props={...h.props,draft:structuredClone(value)};h.render();};
  h.dispose=()=>{h.cleanups.forEach(cleanup=>cleanup?.());h.disposed=true;};
  h.render();return h;
}
(async()=>{
  const output=await build({entryPoints:['apps/frontend/src/RequestBodyEditor.tsx'],bundle:true,platform:'node',format:'cjs',write:false,jsx:'automatic',packages:'external',loader:{'.css':'empty'},plugins:[{name:'request-body-boundaries',setup(builder){
    builder.onLoad({filter:/[/\\]BodyCodeEditor\.tsx$/},()=>({contents:'export default function BodyCodeEditor(){}'}));
    builder.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export const t=(key,args={})=>key.replace(/\\{(\\w+)\\}/g,(_,name)=>String(args[name]??""));'}));
  }}]});
  const loaded={exports:{}};
  new Function('module','exports','require',output.outputFiles[0].text)(loaded,loaded.exports,name=>name==='react'?react:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:name==='lucide-react'?new Proxy({}, {get:()=>function Icon(){}}):require(name));
  const Component=loaded.exports.default;

  // Empty captured requests are empty even when a browser left a form MIME.
  for(const mime of ['', 'application/x-www-form-urlencoded', 'multipart/form-data; boundary=old', 'application/json']) {
    for(const bodyBase64 of ['', ' \r\n\t']) {
      const original=draft('',mime,{method:'GET',url:'https://example.test/path?search=value',bodyBase64});
      const h=mount(Component,original);await h.settle();
      assert.equal(h.input('正文类型').props.value,'none');
      assert(text(h.tree).includes('此请求没有正文'));
      assert(text(h.tree).includes('查询参数，不属于请求体'));
      assert(!h.raw());assert(!h.input('字段名'));assert.equal(h.changes.length,0);
      assert.deepEqual(h.props.draft,original,'mounting must not normalize or erase captured data');
      h.dispose();
    }
  }
  const raw=mount(Component,draft('', 'application/x-www-form-urlencoded',{method:'GET',url:'https://example.test/?q=one'}));
  raw.click('添加文本 / JSON');assert.equal(raw.input('正文类型').props.value,'raw');assert(raw.raw());assert.equal(raw.changes.length,0);
  raw.editRaw('hello 中文');assert.equal(decoded(raw.props.draft.bodyBase64),'hello 中文');assert.equal(raw.props.draft.method,'GET');assert.equal(raw.props.draft.url,'https://example.test/?q=one');raw.dispose();

  const blank=mount(Component,draft('', 'application/x-www-form-urlencoded',{method:'GET'}));
  blank.click('添加表单字段');await blank.settle();
  assert.equal(blank.input('正文类型').props.value,'form');assert(blank.input('字段名'));assert(blank.input('字段值'));
  assert.equal(blank.button('应用到请求体').props.disabled,true);assert.equal(blank.changes.length,0);
  blank.click('添加字段');assert.equal(blank.all(n=>n.type==='input'&&n.props['aria-label']==='字段名').length,2);
  assert.equal(blank.button('应用到请求体').props.disabled,true,'new untouched placeholders are not payload changes');
  blank.button('应用到请求体').props.onClick();await blank.settle();assert.equal(blank.changes.length,0,'a pristine form cannot create an equals-sign body');
  blank.change('字段名','q');blank.change('字段值','a b');assert(text(blank.tree).includes('有修改尚未应用'));
  assert.equal(blank.props.draft.bodyBase64,'','staged fields do not silently alter the sent body');
  blank.click('应用到请求体');await blank.settle();
  assert.equal(decoded(blank.props.draft.bodyBase64),'q=a+b');assert.equal(blank.props.draft.method,'GET');assert.equal(blank.button('应用到请求体').props.disabled,true);
  assert(text(blank.tree).includes('与当前请求体一致'));blank.dispose();

  const addFocus=mount(Component,draft('first=one&last=two','application/x-www-form-urlencoded'));await addFocus.settle();
  const originalFirst=addFocus.input('字段名').element,originalLast=addFocus.input('字段名',1).element;
  const beforeAdd=addFocus.props.draft.bodyBase64;
  assert.equal(addFocus.focusCalls.length,0);assert.equal(addFocus.scrollCalls.length,0);
  addFocus.click('添加字段');
  const fieldNames=()=>addFocus.all(n=>n.type==='input'&&n.props['aria-label']==='字段名');
  assert.equal(fieldNames().length,3,'Add appends one visible editable row');
  const added=addFocus.input('字段名',2).element;
  assert(addFocus.focused===added,'the appended name input receives focus');
  assert.equal(addFocus.focusCalls.length,1);assert.deepEqual(addFocus.focusCalls[0].options,{preventScroll:true});
  assert.equal(addFocus.scrollCalls.length,1);assert(addFocus.scrollCalls[0].element===added.row,'the new row, rather than the whole panel, scrolls into view');
  assert.deepEqual(addFocus.scrollCalls[0].options,{block:'nearest',inline:'nearest'});
  assert(addFocus.input('字段名').element===originalFirst);assert(addFocus.input('字段名',1).element===originalLast);
  assert.equal(addFocus.input('字段名').props.value,'first');assert.equal(addFocus.input('字段值').props.value,'one');assert.equal(addFocus.input('字段名',1).props.value,'last');assert.equal(addFocus.input('字段值',1).props.value,'two');
  assert.equal(addFocus.props.draft.bodyBase64,beforeAdd);assert.equal(addFocus.changes.length,0,'adding and focusing never mutates the sent body');
  addFocus.change('字段名','third',2);addFocus.render();await addFocus.settle();
  assert.equal(addFocus.focusCalls.length,1,'typing and ref reattachment must not repeatedly steal focus');assert.equal(addFocus.scrollCalls.length,1);
  const rapidAdd=addFocus.button('添加字段');rapidAdd.props.onClick();rapidAdd.props.onClick();addFocus.render();
  assert.equal(fieldNames().length,5,'batched Add clicks preserve both appended rows');
  const newest=addFocus.input('字段名',4).element;
  assert(addFocus.focused===newest,'rapid Add clicks focus the last appended row');assert.equal(addFocus.focusCalls.length,2);assert.equal(addFocus.scrollCalls.length,2);assert(addFocus.scrollCalls[1].element===newest.row);
  assert.equal(addFocus.changes.length,0);assert.equal(addFocus.props.draft.bodyBase64,beforeAdd);
  addFocus.choose('raw');addFocus.choose('form');await addFocus.settle();assert.equal(addFocus.focusCalls.length,2,'returning to a staged form does not replay a stale Add focus request');
  addFocus.click('应用到请求体');await addFocus.settle();assert.equal(decoded(addFocus.props.draft.bodyBase64),'first=one&last=two&third=');addFocus.dispose();

  // Real captured empty pairs are distinct from optional editor placeholders.
  const captured=mount(Component,draft('=&a=one%20two','application/x-www-form-urlencoded'));
  await captured.settle();assert.equal(captured.input('字段名').props.value,'');assert.equal(captured.input('字段值').props.value,'');
  assert.equal(captured.changes.length,0);assert.equal(captured.button('应用到请求体').props.disabled,true);
  assert.equal(decoded(captured.props.draft.bodyBase64),'=&a=one%20two','viewing a form preserves original percent encoding');
  captured.change('字段值','changed',1);captured.click('添加字段');captured.click('应用到请求体');await captured.settle();
  assert.equal(decoded(captured.props.draft.bodyBase64),'=&a=changed','real empty pair survives while pristine placeholder is omitted');
  const remove=captured.all(n=>n.type==='button'&&n.props['aria-label']==='移除字段')[0];remove.props.onClick();captured.render();
  captured.click('应用到请求体');await captured.settle();assert.equal(decoded(captured.props.draft.bodyBase64),'a=changed');captured.dispose();

  const staged=mount(Component,draft('a=original','application/x-www-form-urlencoded'));
  await staged.settle();staged.change('字段值','staged');staged.choose('raw');assert.equal(staged.raw().props.value,'a=original');
  staged.choose('form');await staged.settle();assert.equal(staged.input('字段值').props.value,'staged','switching views preserves unapplied form fields');
  assert.equal(staged.changes.length,0);assert(text(staged.tree).includes('有修改尚未应用'));
  assert(staged.button('放弃修改并重新载入'),'reload makes draft loss explicit');
  staged.click('放弃修改并重新载入');await staged.settle();assert.equal(staged.input('字段值').props.value,'original');assert.equal(staged.button('应用到请求体').props.disabled,true);
  staged.change('字段值','again');staged.choose('raw');staged.editRaw('a=new-source');
  staged.choose('form');await staged.settle();assert(!staged.input('字段值'),'a raw body commit invalidates cached form edits instead of relabeling new text as old form data');assert(staged.button('新建表单'));staged.dispose();

  // None is a view of the actual empty wire body, not a deletion of staged fields.
  const emptyStaged=mount(Component,draft('', 'application/x-www-form-urlencoded'));
  emptyStaged.click('添加表单字段');await emptyStaged.settle();emptyStaged.change('字段名','saved');emptyStaged.change('字段值','not yet sent');
  emptyStaged.choose('none');await emptyStaged.settle();assert.equal(emptyStaged.input('正文类型').props.value,'none');assert.equal(emptyStaged.props.draft.bodyBase64,'');assert.equal(emptyStaged.changes.length,0);
  emptyStaged.choose('form');await emptyStaged.settle();assert.equal(emptyStaged.input('字段名').props.value,'saved');assert.equal(emptyStaged.input('字段值').props.value,'not yet sent');assert(text(emptyStaged.tree).includes('有修改尚未应用'));assert.equal(emptyStaged.changes.length,0);
  emptyStaged.click('应用到请求体');await emptyStaged.settle();assert.equal(decoded(emptyStaged.props.draft.bodyBase64),'saved=not+yet+sent');emptyStaged.dispose();

  const isolated=mount(Component,draft('url=original', 'application/x-www-form-urlencoded'));await isolated.settle();isolated.change('字段值','URL staged');
  isolated.choose('multipart');await isolated.settle();assert(!isolated.input('字段名'));isolated.click('新建表单');isolated.change('字段名','part');isolated.change('字段值','Multipart staged');
  isolated.choose('form');await isolated.settle();assert.equal(isolated.input('字段名').props.value,'url');assert.equal(isolated.input('字段值').props.value,'URL staged');
  isolated.choose('multipart');await isolated.settle();assert.equal(isolated.input('字段名').props.value,'part');assert.equal(isolated.input('字段值').props.value,'Multipart staged');assert.equal(isolated.changes.length,0);
  isolated.click('应用到请求体');await isolated.settle();assert.equal(isolated.changes.length,1);
  const multipartMime=isolated.props.draft.headers.find(h=>!h.disabled&&h.name.toLowerCase()==='content-type').value;
  assert.match(multipartMime,/^multipart\/form-data; boundary=/);
  const multipartData=await new Response(Buffer.from(isolated.props.draft.bodyBase64,'base64'),{headers:{'Content-Type':multipartMime}}).formData();
  assert.deepEqual([...multipartData],[['part','Multipart staged']]);
  isolated.choose('form');await isolated.settle();assert(!isolated.input('字段名'),'applying one encoding invalidates old drafts in the other encoding');assert(isolated.button('新建表单'));isolated.dispose();

  const initial=draft('payload','text/plain',{method:'PUT',url:'https://example.test/new',headers:[
    {name:'X-Keep',value:'first'},{name:'Content-Type',value:'text/plain'},
    {name:'Content-Length',value:'999'},{name:'Transfer-Encoding',value:'chunked'},{name:'Content-Encoding',value:'gzip'},
    {name:'Content-Length',value:'disabled length',disabled:true},{name:'Content-Type',value:'application/custom',disabled:true},
    {name:'X-Keep',value:'last'},
  ],customMetadata:{keep:true}});
  const snapshot=structuredClone(initial),none=mount(Component,initial);none.choose('none');await none.settle();
  assert.equal(none.props.draft.bodyBase64,'');assert.equal(none.input('正文类型').props.value,'none');assert.equal(none.changes.length,1);
  assert.deepEqual(none.props.draft.headers,initial.headers.filter(h=>h.disabled||!['content-length','transfer-encoding','content-encoding'].includes(h.name.toLowerCase())));
  assert.equal(none.props.draft.url,initial.url);assert.equal(none.props.draft.method,'PUT');assert.deepEqual(none.props.draft.customMetadata,{keep:true});assert.deepEqual(initial,snapshot);none.dispose();

  // Starting an async apply, then changing mode/body, must not commit late data.
  const modeRace=mount(Component,draft('a=old','application/x-www-form-urlencoded'));await modeRace.settle();modeRace.change('字段值','late');
  modeRace.button('应用到请求体').props.onClick();modeRace.choose('raw');await modeRace.settle();assert.equal(modeRace.changes.length,0);assert.equal(decoded(modeRace.props.draft.bodyBase64),'a=old');modeRace.dispose();
  const bodyRace=mount(Component,draft('a=old','application/x-www-form-urlencoded'));await bodyRace.settle();bodyRace.change('字段值','late');
  bodyRace.button('应用到请求体').props.onClick();bodyRace.replace(draft('a=external','application/x-www-form-urlencoded'));await bodyRace.settle();
  assert.equal(bodyRace.changes.length,0);assert.equal(bodyRace.input('字段值').props.value,'external');bodyRace.dispose();

  // File reading uses the actual stream implementation; controlled chunks make races deterministic.
  function slowFile() { let controller;const file=new Blob([],{type:'application/octet-stream'});file.name='slow.bin';file.stream=()=>new ReadableStream({start(value){controller=value;}});return{file,finish(){assert(controller);controller.enqueue(Uint8Array.of(0,255,1));controller.close();}}; }
  function startFile(h,file) {h.choose('file');const node=h.input('选择二进制文件');assert(node);node.props.onChange({target:{files:[file]},currentTarget:{value:'selected'}});h.render();}
  const slow=slowFile(),fileRace=mount(Component,draft());startFile(fileRace,slow.file);fileRace.choose('raw');slow.finish();await fileRace.settle();
  assert.equal(fileRace.changes.length,0);assert.equal(fileRace.input('正文类型').props.value,'raw');fileRace.dispose();
  const merge=slowFile(),latest=mount(Component,draft());startFile(latest,merge.file);
  latest.replace({...latest.props.draft,method:'PUT',url:'https://example.test/updated',headers:[{name:'X-Latest',value:'keep'}]});
  merge.finish();await latest.settle();assert.equal(latest.changes.length,1);assert.equal(latest.props.draft.bodyBase64,'AP8B');assert.equal(latest.props.draft.method,'PUT');assert.equal(latest.props.draft.url,'https://example.test/updated');assert(latest.props.draft.headers.some(h=>h.name==='X-Latest'));latest.dispose();
  const pending=slowFile(),unmounted=mount(Component,draft());startFile(unmounted,pending.file);unmounted.dispose();pending.finish();await new Promise(resolve=>setImmediate(resolve));assert.equal(unmounted.changes.length,0);assert.equal(unmounted.lateUpdates,0);

  console.log('Request body UI: Add row/focus/nearest-scroll/batched-clicks, empty/MIME state, GET query hint, untouched placeholders, captured empty fields, explicit apply/dirty state, form draft preservation/reload, clear-body metadata, stale apply and file races passed.');
})().catch(error=>{console.error(error);process.exitCode=1;});
