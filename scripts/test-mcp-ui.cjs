const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const {build}=require('esbuild');
let current;
class Element {
  isConnected=true;open=false;
  focus(){} showModal(){this.open=true} close(){this.open=false}
}
global.HTMLElement=Element;global.document={body:new Element(),activeElement:new Element()};
const react={
  useState(initial){const h=current,i=h.index++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return [h.slots[i],value=>{h.slots[i]=typeof value==='function'?value(h.slots[i]):value}]},
  useRef(initial){const h=current,i=h.index++;return h.slots[i]??={current:initial}},
  useId(){const h=current,i=h.index++;return h.slots[i]??=('mcp-test-'+i)},
  useEffect(effect,deps){const h=current,i=h.index++,previous=h.slots[i];if(!previous||deps.some((value,index)=>value!==previous[index])){h.effects.push(()=>{h.cleanups[i]?.();h.cleanups[i]=effect()});h.slots[i]=deps}},
  lazy:()=>function Lazy(){},Suspense:Symbol('Suspense'),
};
const jsx=(type,props)=>{if(typeof type==='string'&&props?.ref&&!props.ref.current)props.ref.current=new Element();return {type,props:props??{}}};
async function load(file){
  const intervals=new Map();let nextInterval=0;
  const output=await build({entryPoints:[file],bundle:true,platform:'node',format:'cjs',jsx:'automatic',write:false,packages:'external',loader:{'.css':'empty'},plugins:[{name:'mcp-ui-contracts',setup(build){
    build.onResolve({filter:/\?raw$/},args=>({path:path.resolve(args.resolveDir,args.path.slice(0,-4)),namespace:'raw'}));
    build.onLoad({filter:/.*/,namespace:'raw'},args=>({contents:'export default '+JSON.stringify(fs.readFileSync(args.path,'utf8'))}));
    build.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export const t=(key)=>key;'}));
    build.onLoad({filter:/[/\\]api\.ts$/},()=>({contents:'export const invoke=(...args)=>globalThis.__mcpInvoke(...args);'}));
    build.onLoad({filter:/[/\\]clipboard\.ts$/},()=>({contents:'export const writeClipboard=(...args)=>globalThis.__mcpClipboard(...args);'}));
    build.onLoad({filter:/[/\\](?:BodyCodeEditor|CodeView)\.tsx$/},()=>({contents:'export default function Editor(){}'}));
  }}]});
  const module={exports:{}};
  new Function('module','exports','require','setInterval','clearInterval',output.outputFiles[0].text)(module,module.exports,name=>name==='react'?react:name==='react/jsx-runtime'?{jsx,jsxs:jsx}:name==='react-dom'?{createPortal:node=>node}:name==='lucide-react'?new Proxy({},{get:()=>()=>null}):require(name),fn=>{const id=++nextInterval;intervals.set(id,fn);return id},id=>intervals.delete(id));
  return {Component:module.exports.default,exports:module.exports,intervals};
}
function text(node){if(node==null||node===false)return '';if(typeof node==='string'||typeof node==='number')return String(node);if(Array.isArray(node))return node.map(text).join('');return text(node.props?.children)}
function find(tree,predicate){const result=[];function walk(node){if(!node||typeof node!=='object')return;if(Array.isArray(node)){node.forEach(walk);return}if(predicate(node))result.push(node);walk(node.props?.children)}walk(tree);return result}
function harness(Component,props={}){
  const h={slots:[],effects:[],cleanups:[],index:0,tree:null};
  h.render=()=>{current=h;h.index=0;h.tree=Component(props);for(const effect of h.effects.splice(0))effect();return h.tree};
  h.find=predicate=>find(h.tree,predicate);h.button=name=>h.find(node=>node.type==='button'&&text(node)===name)[0];
  h.input=label=>h.find(node=>node.props?.['aria-label']===label)[0];
  h.tick=async()=>{await new Promise(resolve=>setImmediate(resolve));h.render()};
  h.dispose=()=>{for(const cleanup of h.cleanups)cleanup?.()};h.render();return h;
}
const change=(node,value)=>node.props.onChange({target:{value}});
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return {promise,resolve,reject}};
const config={enabled:false,port:1422,allowSend:false,allowCaptureControl:false,includeSensitiveHeaders:false,allowWorkspaceWrite:false,allowRulesWrite:false,allowScriptsWrite:false};
const status=(running=false,overrides={})=>({config:{...config,enabled:running},running,address:running?'http://127.0.0.1:1422/mcp':null,lastError:null,toolNames:['capture_status','list_requests'],...overrides});

(async()=>{
  const calls=[],copies=[];const initial=deferred();let read=()=>initial.promise,configure=async input=>status(input.enabled,{config:input});
  global.__mcpClipboard=async value=>copies.push(value);
  global.__mcpInvoke=async(command,args)=>{calls.push([command,args]);if(command==='mcp_status')return read();if(command==='mcp_configure')return configure(args.config);if(command==='mcp_connection_info')return {url:'http://127.0.0.1:1422/mcp',headers:{Authorization:'Bearer private-fixture-token'}};throw Error('unexpected '+command)};
  const pageModule=await load('apps/frontend/src/McpPage.tsx'),page=harness(pageModule.Component);
  assert(page.button('启动 MCP').props.disabled,'start is disabled until initial status loads');
  assert(page.input('MCP 监听端口').props.disabled,'initial status must not overwrite user input: controls stay disabled until loaded');
  assert.equal(calls.filter(([command])=>command==='mcp_configure').length,0,'opening the page must not start the server');
  initial.resolve(status());await page.tick();
  const beforeHelp=calls.length;
  assert(!page.find(node=>typeof node.type==='function'&&node.type.name==='McpHelp').length);
  page.button('使用说明').props.onClick();page.render();
  const helpEntry=page.find(node=>typeof node.type==='function'&&node.type.name==='McpHelp')[0];assert(helpEntry,'help is available directly from the MCP page');
  helpEntry.props.onClose();page.render();assert(!page.find(node=>typeof node.type==='function'&&node.type.name==='McpHelp').length);
  assert.equal(calls.length,beforeHelp,'opening and closing help must not configure MCP, reveal a token, connect or call a tool');

  const helpModule=await load('apps/frontend/src/McpHelp.tsx'),guide=fs.readFileSync('docs/mcp.md','utf8');let helpClosed=0;
  const help=harness(helpModule.Component,{onClose:()=>helpClosed++});
  assert.equal(help.find(node=>node.type==='pre'&&node.props['aria-label']==='MCP 使用说明全文')[0].props.children,guide,'the full local guide is bundled for offline reading');
  assert(help.tree.props.ref.current.open,'help opens as a native modal');
  help.button('复制完整说明').props.onClick();await help.tick();assert.equal(copies.pop(),guide,'copy uses the bundled guide, not live connection information');
  assert(text(help.tree).includes('完整说明已复制。'));
  global.__mcpClipboard=async()=>{throw Error('clipboard unavailable')};
  help.button('复制完整说明').props.onClick();await help.tick();assert(help.find(node=>node.props?.role==='alert').length,'a clipboard failure offers manual text selection');
  global.__mcpClipboard=async value=>copies.push(value);
  help.tree.props.onCancel({preventDefault(){}});assert.equal(helpClosed,1);help.button('关闭').props.onClick();assert.equal(helpClosed,2);
  const dialogElement=help.tree.props.ref.current;help.dispose();assert(!dialogElement.open,'unmount closes the modal');
  assert.equal(calls.length,beforeHelp,'offline reading and copying must not invoke MCP management or external tools');

  // Render the actual setup component independently; the lightweight page
  // harness intentionally does not recursively render child components.
  const setupModule=await load('apps/frontend/src/McpSetup.tsx');
  const configurationModule=await load('apps/frontend/src/mcpConfiguration.ts');
  const buildConfiguration=configurationModule.exports.buildMcpConfiguration;
  const token='a'.repeat(64),literal='quote"\n\t中文\\end';
  const connection={url:'http://127.0.0.1:1422/mcp',headers:{Authorization:'Bearer '+token},stdio:{command:'C:\\Program Files\\抓包工具\\HTTP Capture.exe',args:['--mcp-stdio','--url','http://127.0.0.1:1422/mcp','literal ; & $(no_shell)'],env:{HTTP_CAPTURE_MCP_TOKEN:token,TEST_LITERAL:literal}}};
  const setupCalls=[],setupCopies=[];let copyFails=false,connectionFails=false;
  const pageInvoke=global.__mcpInvoke,pageClipboard=global.__mcpClipboard;
  global.__mcpInvoke=async(command,args)=>{setupCalls.push([command,args]);assert.equal(command,'mcp_connection_info');if(connectionFails)throw Error('fixture connection unavailable');return structuredClone(connection)};
  global.__mcpClipboard=async value=>{if(copyFails)throw Error('clipboard denied');setupCopies.push(value)};
  const setupProps={running:false},setup=harness(setupModule.Component,setupProps);
  assert.equal(setupCalls.length,0,'opening the setup guide must not read a token or call a management API');
  assert(setup.button('生成连接配置').props.disabled);
  setup.button('生成连接配置').props.onClick();await setup.tick();assert.equal(setupCalls.length,0,'a stopped server cannot reveal connection credentials');
  change(setup.input('AI 客户端'),'cursor');setup.render();change(setup.input('MCP 接入方式'),'stdio');setup.render();assert.equal(setupCalls.length,0,'choosing a client or transport must not fetch credentials');
  setupProps.running=true;setup.render();assert.equal(setupCalls.length,0,'starting the parent server does not automatically reveal credentials');
  setup.button('生成连接配置').props.onClick();await setup.tick();assert.equal(setupCalls.length,1);
  const preview=()=>setup.find(node=>node.props?.['aria-label']==='AI 客户端配置预览')[0];
  assert.deepEqual(JSON.parse(preview().props.children).mcpServers.httpCapture,connection.stdio);
  assert(text(setup.tree).includes('~/.cursor/mcp.json'));
  for(const client of ['codex','cursor','vscode','generic']){
    change(setup.input('AI 客户端'),client);setup.render();
    assert.equal(setupCalls.length,1,'client changes reuse only the explicitly fetched connection information');
    const value=buildConfiguration(client,'stdio',connection);
    assert.equal(preview().props.children,value.text);
    if(client==='codex'){
      assert(value.text.startsWith('[mcp_servers.httpCapture]\n'));
      assert.equal(JSON.parse(value.text.match(/^command = (.+)$/m)[1]),connection.stdio.command,'TOML string escaping preserves Windows paths and spaces');
      assert.deepEqual(JSON.parse(value.text.match(/^args = (.+)$/m)[1]),connection.stdio.args,'arguments stay separate, including shell-like literal text');
      assert.equal(JSON.parse(value.text.match(/^"TEST_LITERAL" = (.+)$/m)[1]),literal,'TOML basic-string escaping preserves quotes, newlines, tabs and Unicode');
      assert.equal(value.target,'~/.codex/config.toml');
    }else{
      const parsed=JSON.parse(value.text),server=client==='cursor'?parsed.mcpServers.httpCapture:client==='vscode'?parsed.servers.httpCapture:parsed;
      assert.deepEqual(server,client==='vscode'?{type:'stdio',...connection.stdio}:connection.stdio,'JSON configuration preserves the exact executable, argv and environment');
    }
  }
  change(setup.input('MCP 接入方式'),'http');setup.render();assert(!preview(),'changing transport discards the old generated configuration');assert.equal(setupCalls.length,1);
  setup.button('生成连接配置').props.onClick();await setup.tick();assert.equal(setupCalls.length,2);
  for(const client of ['codex','cursor','vscode','generic']){
    change(setup.input('AI 客户端'),client);setup.render();const value=buildConfiguration(client,'http',connection);assert.equal(preview().props.children,value.text);
    if(client==='codex'){
      assert.equal(JSON.parse(value.text.match(/^url = (.+)$/m)[1]),connection.url);
      assert.equal(JSON.parse(value.text.match(/^http_headers = \{ Authorization = (.+) \}$/m)[1]),connection.headers.Authorization);
      assert(!value.text.includes('HTTP_CAPTURE_MCP_TOKEN'),'HTTP configuration has no unrelated stdio launch fields');
    }else{
      const parsed=JSON.parse(value.text),server=client==='cursor'?parsed.mcpServers.httpCapture:client==='vscode'?parsed.servers.httpCapture:parsed;
      assert.deepEqual(server,client==='vscode'?{type:'http',url:connection.url,headers:connection.headers}:{url:connection.url,headers:connection.headers});
    }
  }
  assert.equal(setupCalls.length,2);
  const preserved=preview().props.children;copyFails=true;
  setup.button('复制客户端配置').props.onClick();await setup.tick();assert.equal(preview().props.children,preserved,'clipboard errors keep the generated text selectable');assert(setup.find(node=>node.props?.role==='alert').length);assert.equal(setupCalls.length,2);
  copyFails=false;setup.button('复制客户端配置').props.onClick();await setup.tick();assert.equal(setupCopies.pop(),preserved);assert(text(setup.tree).includes('客户端配置已复制'));assert.equal(setupCalls.length,2,'copy uses the already generated text without another token request');
  connectionFails=true;setup.button('生成连接配置').props.onClick();await setup.tick();assert.equal(preview().props.children,preserved,'failed regeneration preserves the prior valid configuration');assert(setup.find(node=>node.props?.role==='alert').length);
  for(const url of ['https://example.invalid/mcp','http://127.0.0.1:1422/','http://user:secret@127.0.0.1:1422/mcp','http://127.0.0.1:1422/mcp?token=secret'])assert.throws(()=>buildConfiguration('generic','http',{...connection,url}));
  assert.throws(()=>buildConfiguration('generic','http',{...connection,headers:{Authorization:'Bearer placeholder'}}));
  assert.throws(()=>buildConfiguration('generic','stdio',{...connection,stdio:undefined}));
  setup.dispose();global.__mcpInvoke=pageInvoke;global.__mcpClipboard=pageClipboard;

  assert(!page.input('MCP 监听端口').props.disabled);change(page.input('MCP 监听端口'),'12a34');page.render();assert.equal(page.input('MCP 监听端口').props.value,'1234');
  change(page.input('MCP 监听端口'),'');page.render();page.button('启动 MCP').props.onClick();await page.tick();assert.equal(calls.filter(([command])=>command==='mcp_configure').length,0);assert(page.find(node=>node.props?.role==='alert').length);
  change(page.input('MCP 监听端口'),'1422');page.render();
  const polling=deferred();read=()=>polling.promise;[...pageModule.intervals.values()][0]();
  page.button('启动 MCP').props.onClick();await page.tick();assert(page.button('停止 MCP'));assert(page.input('MCP 监听端口').props.disabled);
  polling.resolve(status(false));await page.tick();assert(page.button('停止 MCP'),'an older poll must not roll back the successful start status');
  page.button('复制连接配置').props.onClick();await page.tick();assert.equal(copies.length,1);assert(JSON.parse(copies[0]).headers.Authorization.includes('private-fixture-token'));
  assert(!text(page.tree).includes('private-fixture-token'),'the token belongs only in the explicitly copied payload');
  page.button('停止 MCP').props.onClick();await page.tick();assert(page.button('启动 MCP'));assert(!page.input('MCP 监听端口').props.disabled);
  configure=async()=>{throw Error('bind fixture failure')};change(page.input('MCP 监听端口'),'4555');page.render();page.button('启动 MCP').props.onClick();await page.tick();assert.equal(page.input('MCP 监听端口').props.value,'4555');assert(page.find(node=>node.props?.role==='alert').length);
  assert(page.button('外部服务'));page.button('外部服务').props.onClick();page.render();assert(page.find(node=>typeof node.type==='function').some(node=>node.type.name.includes('McpClients')));page.dispose();
  global.__mcpInvoke=async command=>{assert.equal(command,'mcp_status');throw Error('HTTP 404: Not Found')};
  const oldCore=harness(pageModule.Component);await oldCore.tick();assert(text(oldCore.tree).includes('当前内核不支持此 MCP 接口，请重启内核加载新版本；开发模式重新运行 npm run capture。'),'an older running core gets an actionable restart hint');assert(oldCore.button('启动 MCP').props.disabled);assert(text(oldCore.tree).includes('HTTP / stdio'),'quick start directs users to the new client setup guide');oldCore.dispose();

  let items=[],saveError=false,callResult={content:[{type:'text',text:'first result'}]},callError=false,listing=()=>Promise.resolve(structuredClone(items));const operations=[];
  global.__mcpInvoke=async(command,args)=>{
    operations.push([command,args]);
    if(command==='mcp_clients')return listing();
    if(command==='mcp_save_client'){
      if(saveError)throw Error('fixture save failed');const input=args.input;
      const saved={id:input.id??'new',name:input.name,transport:input.transport,url:input.url,command:input.command,args:input.args,hasToken:!!input.bearerToken,connected:false,serverInfo:null,lastError:null};items=[saved];return saved;
    }
    if(command==='mcp_connect_client'){items=items.map(item=>({...item,connected:true}));return items[0]}
    if(command==='mcp_disconnect_client'){items=items.map(item=>({...item,connected:false}));return items[0]}
    if(command==='mcp_client_tools')return [{name:'echo',description:'Untrusted service text',inputSchema:{type:'object'}}];
    if(command==='mcp_call_tool'){if(callError)throw Error('fixture call failed');return callResult}
    if(command==='mcp_remove_client'){items=[];return}
    throw Error('unexpected '+command);
  };
  const clientModule=await load('apps/frontend/src/McpClients.tsx'),clients=harness(clientModule.Component);await clients.tick();
  assert(clients.button('添加服务'));assert(!operations.some(([command])=>command==='mcp_connect_client'||command==='mcp_call_tool'));
  clients.button('添加服务').props.onClick();clients.render();change(clients.input('MCP 服务名称'),'Echo service');clients.render();change(clients.input('MCP 服务地址'),'http://127.0.0.1:5555/mcp');clients.render();change(clients.input('MCP Bearer Token'),'client-session-secret');clients.render();
  saveError=true;clients.find(node=>node.type==='form')[0].props.onSubmit({preventDefault(){}});await clients.tick();assert.equal(clients.input('MCP 服务名称').props.value,'Echo service');assert.equal(clients.input('MCP Bearer Token').props.value,'client-session-secret');assert(clients.find(node=>node.props?.role==='alert').length);
  saveError=false;clients.find(node=>node.type==='form')[0].props.onSubmit({preventDefault(){}});await clients.tick();assert(!clients.input('MCP Bearer Token'));assert.equal(items[0].connected,false);assert(!operations.some(([command])=>command==='mcp_connect_client'));
  clients.button('连接').props.onClick();await clients.tick();assert(clients.button('断开'));assert(!operations.some(([command])=>command==='mcp_call_tool'));
  clients.button('工具目录').props.onClick();await clients.tick();assert.equal(clients.input('MCP 工具').props.value,'echo');assert(!operations.some(([command])=>command==='mcp_call_tool'));
  const parameter=()=>clients.find(node=>typeof node.type==='function'&&node.props?.label==='MCP 工具参数')[0];
  parameter().props.onChange('[]');clients.render();clients.button('手动调用工具').props.onClick();await clients.tick();assert(!operations.some(([command])=>command==='mcp_call_tool'));assert(clients.find(node=>node.props?.role==='alert').length);
  parameter().props.onChange('{"message":"中文 🛰️"}');clients.render();clients.button('手动调用工具').props.onClick();await clients.tick();assert.deepEqual(operations.filter(([command])=>command==='mcp_call_tool').at(-1)[1].arguments,{message:'中文 🛰️'});
  assert(clients.find(node=>typeof node.type==='function'&&node.props?.label==='MCP 工具调用结果').length);
  callError=true;clients.button('手动调用工具').props.onClick();await clients.tick();assert(!clients.find(node=>typeof node.type==='function'&&node.props?.label==='MCP 工具调用结果').length,'a failed new call must not present the previous result as its output');
  // A pending manual refresh may finish after a disconnect refresh. The old
  // response must not make a closed connection appear usable again.
  const stale=deferred();let reads=0;listing=()=>++reads===1?stale.promise:Promise.resolve(structuredClone(items));
  clients.button('刷新').props.onClick();clients.render();assert(!clients.button('断开').props.disabled,'disconnect stays available while another request is pending');
  clients.button('断开').props.onClick();await clients.tick();assert(!clients.button('断开'));stale.resolve([{...items[0],connected:true}]);await clients.tick();assert(!clients.button('断开'),'stale list response cannot resurrect a disconnected service');
  assert(clients.button('连接'));clients.button('编辑').props.onClick();clients.render();assert.equal(clients.input('MCP Bearer Token').props.value,'');
  change(clients.input('MCP 传输方式'),'stdio');clients.render();change(clients.input('MCP 可执行程序'),'node');clients.render();change(clients.input('MCP 命令参数'),'["server.mjs", "literal ; & $(no_shell)"]');clients.render();
  clients.find(node=>node.type==='form')[0].props.onSubmit({preventDefault(){}});await clients.tick();const stdio=operations.filter(([command])=>command==='mcp_save_client').at(-1)[1].input;assert.equal(stdio.transport,'stdio');assert.equal(stdio.url,'');assert.equal(stdio.bearerToken,'');assert.deepEqual(stdio.args,['server.mjs','literal ; & $(no_shell)']);
  clients.dispose();
  console.log('MCP UI: offline guide, actual client setup without automatic APIs, explicit credential generation, four client HTTP/stdio configurations, JSON/TOML escaping, clipboard and regeneration preservation, explicit start/connect/call, loading/poll guards, token boundaries and disconnect/list races passed.');
})().catch(error=>{console.error(error);process.exitCode=1});
