import {lazy, Suspense, useEffect, useRef, useState} from 'react';
import {Cable, Check, CircleHelp, LoaderCircle, Plus, RefreshCw, Unplug} from 'lucide-react';
import {invoke} from './api';
import {t} from './i18n';
import BodyCodeEditor from './BodyCodeEditor';
import './mcp-clients.css';
const CodeView=lazy(()=>import('./CodeView'));

export interface McpClientStatus {
  id:string; name:string; transport:'http'|'stdio'; url:string; command:string; args:string[];
  hasToken:boolean; connected:boolean; serverInfo:unknown|null; lastError:string|null;
}
interface Tool {name:string; description?:string; inputSchema?:unknown}
interface Editor {id?:string; name:string; transport:'http'|'stdio'; url:string; command:string; args:string; token:string; clearToken:boolean; hasToken:boolean}
const emptyEditor = ():Editor => ({name:'',transport:'http',url:'',command:'',args:'[]',token:'',clearToken:false,hasToken:false});

export default function McpClients() {
  const [items,setItems]=useState<McpClientStatus[]>([]), [ready,setReady]=useState(false);
  const [editor,setEditor]=useState<Editor|null>(null), [error,setError]=useState(''), [notice,setNotice]=useState('');
  const [busy,setBusy]=useState(''), [disconnecting,setDisconnecting]=useState<string[]>([]);
  const [serviceId,setServiceId]=useState(''), [tools,setTools]=useState<Tool[]>([]), [toolName,setToolName]=useState('');
  const [argumentsText,setArgumentsText]=useState('{}'), [result,setResult]=useState<unknown>();
  const mounted=useRef(true), operation=useRef(false), catalogGeneration=useRef(0), refreshGeneration=useRef(0);
  useEffect(()=>{mounted.current=true;void refresh();return()=>{mounted.current=false;catalogGeneration.current++;refreshGeneration.current++}},[]);
  async function refresh() {
    const generation=++refreshGeneration.current;
    try {const next=await invoke<McpClientStatus[]>('mcp_clients');if(mounted.current&&generation===refreshGeneration.current){setItems(next);setReady(true)}}
    catch(e){if(mounted.current&&generation===refreshGeneration.current)setError(String(e))}
  }
  async function action(key:string,run:()=>Promise<void>) {
    if(operation.current)return;
    operation.current=true;refreshGeneration.current++;setBusy(key);setError('');setNotice('');
    try {await run()}
    catch(e){if(mounted.current)setError(String(e))}
    finally {await refresh();operation.current=false;if(mounted.current)setBusy('')}
  }
  function openEditor(item?:McpClientStatus) {
    setEditor(item?{id:item.id,name:item.name,transport:item.transport,url:item.url,command:item.command,args:JSON.stringify(item.args,null,2),token:'',clearToken:false,hasToken:item.hasToken}:emptyEditor());
    setError('');setNotice('');
  }
  async function save() {
    if(!editor)return;
    const submitted={...editor};
    await action('save',async()=>{
      let args:unknown=[];
      if(submitted.transport==='stdio') {
        try {args=JSON.parse(submitted.args)} catch {throw Error(t('命令参数必须为 JSON 字符串数组'))}
        if(!Array.isArray(args)||args.some(value=>typeof value!=='string'))throw Error(t('命令参数必须为 JSON 字符串数组'));
      }
      await invoke<McpClientStatus>('mcp_save_client',{input:{id:submitted.id??null,name:submitted.name,transport:submitted.transport,
        url:submitted.transport==='http'?submitted.url:'',command:submitted.transport==='stdio'?submitted.command:'',args,
        bearerToken:submitted.transport==='http'?(submitted.clearToken?'':submitted.token||null):''}});
      if(mounted.current){setEditor(null);setNotice(t('连接配置已保存，点击连接后才会访问服务。'))}
    });
  }
  async function connect(item:McpClientStatus) {
    await action('connect:'+item.id,async()=>{await invoke('mcp_connect_client',{id:item.id});if(mounted.current)setNotice(t('MCP 服务已连接，可读取工具目录。'))});
  }
  async function disconnect(item:McpClientStatus) {
    if(disconnecting.includes(item.id))return;
    refreshGeneration.current++;setDisconnecting(previous=>[...previous,item.id]);setError('');
    try {
      await invoke('mcp_disconnect_client',{id:item.id});
      if(serviceId===item.id){catalogGeneration.current++;setTools([]);setResult(undefined)}
      if(mounted.current)setNotice(t('已断开 MCP 连接。远端已开始的操作可能仍会继续。'));
    } catch(e){if(mounted.current)setError(String(e))}
    finally {await refresh();if(mounted.current)setDisconnecting(previous=>previous.filter(id=>id!==item.id))}
  }
  async function loadTools(item:McpClientStatus) {
    const generation=++catalogGeneration.current;setServiceId(item.id);setTools([]);setToolName('');setResult(undefined);
    await action('tools:'+item.id,async()=>{
      const catalog=await invoke<unknown[]>('mcp_client_tools',{id:item.id});
      if(mounted.current&&catalogGeneration.current===generation){
        const usable=catalog.filter((value):value is Tool=>!!value&&typeof value==='object'&&typeof (value as Tool).name==='string');
        setTools(usable);setToolName(usable[0]?.name??'');setArgumentsText('{}');
      }
    });
  }
  async function call() {
    const selectedService=serviceId, selectedName=toolName, text=argumentsText, generation=catalogGeneration.current;
    await action('call:'+selectedService,async()=>{
      let args:unknown;
      try{args=JSON.parse(text)}catch{throw Error(t('工具参数必须为有效的 JSON 对象'))}
      if(!args||typeof args!=='object'||Array.isArray(args))throw Error(t('工具参数必须为有效的 JSON 对象'));
      if(mounted.current)setResult(undefined);
      const output=await invoke('mcp_call_tool',{id:selectedService,name:selectedName,arguments:args});
      if(mounted.current&&catalogGeneration.current===generation)setResult(output);
    });
  }
  const selected=items.find(item=>item.id===serviceId), selectedTool=tools.find(tool=>tool.name===toolName);
  return <section className="mcp-clients">
    <header className="mcp-clients-heading"><div><h2>{t('外部 MCP 服务')}</h2><p>{t('连接外部服务，查看工具目录，并手动填写参数调用。不会自动上传抓包记录或自动执行工具。')}</p></div><div className="mcp-clients-actions"><button className="button" disabled={!!busy} onClick={()=>void action('refresh',refresh)}><RefreshCw size={14}/>{t('刷新')}</button><button className="button primary" disabled={!!busy||!ready} onClick={()=>openEditor()}><Plus size={15}/>{t('添加服务')}</button></div></header>
    <div className="mcp-clients-note"><CircleHelp size={16}/><p>{t('HTTP 支持 Streamable HTTP；stdio 运行你指定的本地程序。保存配置不会自动连接。Bearer Token 仅保留本次会话，重启后需重新填写。')}</p></div>
    {error&&<p className="mcp-client-error" role="alert">{error}</p>}
    {notice&&<p className="mcp-client-notice" role="status"><Check size={15}/>{notice}</p>}
    {editor&&<form className="mcp-client-editor" onSubmit={event=>{event.preventDefault();void save()}}>
      <h3>{editor.id?t('编辑 MCP 服务'):t('添加 MCP 服务')}</h3>
      <div className="mcp-client-fields"><label>{t('服务名称')}<input autoFocus aria-label={t('MCP 服务名称')} required maxLength={160} value={editor.name} disabled={!!busy} placeholder={t('例如：本地文档助手')} onChange={event=>setEditor({...editor,name:event.target.value})}/></label>
      <label>{t('传输方式')}<select aria-label={t('MCP 传输方式')} value={editor.transport} disabled={!!busy} onChange={event=>setEditor({...editor,transport:event.target.value as 'http'|'stdio',token:'',clearToken:false})}><option value="http">Streamable HTTP</option><option value="stdio">stdio</option></select></label></div>
      {editor.transport==='http'?<><label>{t('服务地址')}<input aria-label={t('MCP 服务地址')} type="url" required maxLength={4096} disabled={!!busy} value={editor.url} placeholder="http://127.0.0.1:3000/mcp" onChange={event=>setEditor({...editor,url:event.target.value})}/></label>
      <label>{t('Bearer Token（可选）')}<input aria-label={t('MCP Bearer Token')} type="password" autoComplete="new-password" spellCheck={false} maxLength={8192} value={editor.token} disabled={!!busy||editor.clearToken} placeholder={editor.hasToken?t('留空保留本次会话中的 Token'):t('服务需要认证时填写')} onChange={event=>setEditor({...editor,token:event.target.value})}/></label><label className="mcp-client-check"><input type="checkbox" checked={editor.clearToken} disabled={!!busy} onChange={event=>setEditor({...editor,clearToken:event.target.checked,token:''})}/>{t('清除 Token 及认证需求')}</label></>:<><label>{t('可执行程序')}<input aria-label={t('MCP 可执行程序')} required maxLength={4096} disabled={!!busy} value={editor.command} placeholder={t('例如：node 或程序的绝对路径')} onChange={event=>setEditor({...editor,command:event.target.value})}/></label><label>{t('命令参数（JSON 字符串数组）')}<textarea aria-label={t('MCP 命令参数')} value={editor.args} spellCheck={false} disabled={!!busy} rows={4} placeholder={'["C:/tools/mcp-server.mjs"]'} onChange={event=>setEditor({...editor,args:event.target.value})}/></label><p className="mcp-client-hint">{t('每个参数独立传入，不经过 shell。请使用已安装程序，不会自动安装依赖。')}</p></>}
      <div className="mcp-clients-actions"><button type="submit" className="button primary" disabled={!!busy}>{busy==='save'?t('正在保存…'):t('保存配置')}</button><button type="button" className="button" disabled={!!busy} onClick={()=>setEditor(null)}>{t('取消')}</button></div>
    </form>}
    {ready&&!items.length&&!editor&&<div className="mcp-clients-empty"><Cable size={28}/><strong>{t('还没有外部 MCP 服务')}</strong><p>{t('添加服务地址或本地程序，连接后即可查看可调用的工具。')}</p></div>}
    <div className="mcp-client-list">{items.map(item=>{
      const connecting=busy==='connect:'+item.id, closing=disconnecting.includes(item.id);
      return <article className={'mcp-client-card'+(selected?.id===item.id?' is-selected':'')} key={item.id}>
        <div className="mcp-client-card-heading"><div><h3>{item.name}<span className={'mcp-client-state'+(item.connected?' is-connected':'')}>{item.connected?t('已连接'):connecting?t('正在连接…'):t('未连接')}</span></h3><code>{item.transport==='http'?item.url:[item.command,...item.args].map(value=>JSON.stringify(value)).join(' ')}</code></div><span className="mcp-client-transport">{item.transport==='http'?'HTTP':'stdio'}</span></div>
        {item.lastError&&<p className="mcp-client-error">{t(item.lastError)}</p>}
        <div className="mcp-clients-actions">{item.connected||connecting?<button className="button" disabled={closing} onClick={()=>void disconnect(item)}><Unplug size={14}/>{closing?t('正在断开…'):connecting?t('取消连接'):t('断开')}</button>:<button className="button primary" disabled={!!busy||closing} onClick={()=>void connect(item)}><Cable size={14}/>{t('连接')}</button>}
        <button className="button" disabled={!item.connected||!!busy||closing} onClick={()=>void loadTools(item)}>{busy==='tools:'+item.id?<LoaderCircle size={14} className="mcp-client-spinner"/>:null}{t('工具目录')}</button><button className="button" disabled={!!busy||closing} onClick={()=>openEditor(item)}>{t('编辑')}</button><button className="button mcp-client-remove" disabled={!!busy||closing||item.connected} title={item.connected?t('先断开服务再删除'):undefined} onClick={()=>void action('remove:'+item.id,async()=>{await invoke('mcp_remove_client',{id:item.id});if(serviceId===item.id){catalogGeneration.current++;setServiceId('');setTools([]);setResult(undefined)}})}>{t('删除')}</button></div>
        {item.serverInfo!=null&&<details className="mcp-client-server-info"><summary>{t('服务信息')}</summary><pre>{JSON.stringify(item.serverInfo,null,2)}</pre></details>}
      </article>
    })}</div>
    {selected&&<section className="mcp-client-tools"><header><h3>{t('工具调用')} · {selected.name}</h3><p>{t('工具描述来自外部服务，请检查名称、输入格式及操作影响后手动调用。')}</p></header>
      {!tools.length?<p className="mcp-client-hint">{busy==='tools:'+selected.id?t('正在读取工具目录…'):t('当前没有工具，请连接服务并读取工具目录。')}</p>:<><label>{t('选择工具')}<select aria-label={t('MCP 工具')} value={toolName} disabled={!!busy} onChange={event=>{setToolName(event.target.value);setResult(undefined)}}>{tools.map(tool=><option value={tool.name} key={tool.name}>{tool.name}</option>)}</select></label>
      {selectedTool?.description&&<p className="mcp-tool-description">{selectedTool.description}</p>}
      <details><summary>{t('参数 Schema')}</summary><pre>{JSON.stringify(selectedTool?.inputSchema??{},null,2)}</pre></details>
      <div className="mcp-client-arguments"><h4>{t('参数（JSON 对象）')}</h4><BodyCodeEditor label={t('MCP 工具参数')} parser="json" formatting value={argumentsText} disabled={!!busy} onChange={setArgumentsText}/></div>
      <button className="button primary" disabled={!!busy||!selected.connected||!toolName||disconnecting.includes(selected.id)} onClick={()=>void call()}>{busy==='call:'+selected.id?<LoaderCircle size={15} className="mcp-client-spinner"/>:null}{busy==='call:'+selected.id?t('正在调用…'):t('手动调用工具')}</button></>}
      {result!==undefined&&<div className="mcp-client-result"><h4>{t('调用结果')}</h4><Suspense fallback={<p>{t('正在读取…')}</p>}><CodeView label={t('MCP 工具调用结果')} language="json" text={JSON.stringify(result,null,2)}/></Suspense></div>}
    </section>}
  </section>;
}
