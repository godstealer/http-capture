import {useEffect, useRef, useState} from 'react';
import {BookOpen, Cable, Check, Copy, Play, RefreshCw, ShieldCheck, Square} from 'lucide-react';
import {invoke} from './api';
import {writeClipboard} from './clipboard';
import {t} from './i18n';
import McpClients from './McpClients';
import McpHelp from './McpHelp';
import McpSetup from './McpSetup';
import './mcp-page.css';

interface Config {enabled:boolean;port:number;allowSend:boolean;allowCaptureControl:boolean;includeSensitiveHeaders:boolean;allowWorkspaceWrite:boolean;allowRulesWrite:boolean;allowScriptsWrite:boolean}
interface AiClient {id:string;name:string;version:string;protocol:string;lastSeen:number;firstSeen:number;requests:number}
interface Status {config:Config;running:boolean;address:string|null;lastError:string|null;toolNames:string[];clients?:AiClient[]}
const defaults:Config={enabled:false,port:1422,allowSend:false,allowCaptureControl:false,includeSensitiveHeaders:false,allowWorkspaceWrite:false,allowRulesWrite:false,allowScriptsWrite:false};
function readableError(error:unknown){const message=String(error);return /\bHTTP\s+404\b|\b404\s+Not Found\b/i.test(message)?t('当前内核不支持此 MCP 接口，请重启内核加载新版本；开发模式重新运行 npm run capture。'):message}

export default function McpPage() {
  const [tab,setTab]=useState('server'), [status,setStatus]=useState<Status>(), [config,setConfig]=useState(defaults);
  const [port,setPort]=useState('1422'), [busy,setBusy]=useState(false), [error,setError]=useState(''), [notice,setNotice]=useState('');
  const [helpOpen,setHelpOpen]=useState(false);
  const mounted=useRef(false), pending=useRef(false), loaded=useRef(false), epoch=useRef(0);
  useEffect(()=>{
    mounted.current=true;
    async function refresh(){
      if(pending.current)return;
      const generation=++epoch.current;
      try{
        const next=await invoke<Status>('mcp_status');
        if(!mounted.current||pending.current||generation!==epoch.current)return;
        setStatus(next);
        if(!loaded.current||next.running){loaded.current=true;setConfig(next.config);setPort(String(next.config.port))}
      }catch(e){if(mounted.current&&generation===epoch.current)setError(readableError(e))}
    }
    void refresh();const timer=setInterval(()=>void refresh(),3000);
    return()=>{mounted.current=false;epoch.current++;clearInterval(timer)};
  },[]);
  async function configure(enabled:boolean){
    if(pending.current)return;
    const value=Number(port);
    if(!/^\d+$/.test(port)||!Number.isInteger(value)||value<1||value>65535){setError(t('端口必须为 1 到 65535 的整数'));return}
    pending.current=true;epoch.current++;setBusy(true);setError('');setNotice('');
    try{
      const next=await invoke<Status>('mcp_configure',{config:{...config,port:value,enabled}});
      if(mounted.current){setStatus(next);setConfig(next.config);setPort(String(next.config.port));setNotice(enabled?t('MCP 服务已启动，重启应用后会自动恢复。'):t('MCP 服务已停用，配置已保存。'))}
    }catch(e){if(mounted.current)setError(readableError(e))}
    finally{pending.current=false;if(mounted.current)setBusy(false)}
  }
  async function copyConnection(){
    if(pending.current)return;
    pending.current=true;epoch.current++;setBusy(true);setError('');setNotice('');
    try{
      const connection=await invoke('mcp_connection_info');
      await writeClipboard(JSON.stringify(connection,null,2));
      if(mounted.current)setNotice(t('连接配置已复制，包含访问 Token，请只交给可信的 AI 客户端。'));
    }catch(e){if(mounted.current)setError(readableError(e))}
    finally{pending.current=false;if(mounted.current)setBusy(false)}
  }
  const running=!!status?.running, locked=busy||running||!status;
  return <main className="mcp-page">
    <header className="mcp-page-heading"><div><h1><Cable size={23}/>MCP</h1><p>{t('让 AI 操作当前抓包工具，或连接外部 MCP 服务。')}</p></div><button type="button" className="button" onClick={()=>setHelpOpen(true)}><BookOpen size={16}/>{t('使用说明')}</button></header>
    <nav className="mcp-page-tabs" aria-label={t('MCP 功能')}><button aria-current={tab==='server'?'page':undefined} className={tab==='server'?'active':''} onClick={()=>setTab('server')}>{t('AI 接入')}</button><button aria-current={tab==='client'?'page':undefined} className={tab==='client'?'active':''} onClick={()=>setTab('client')}>{t('外部服务')}</button></nav>
    <aside className="mcp-quick-start" aria-label={t('MCP 快速入门')}>
      <div><BookOpen size={18}/><strong>{tab==='server'?t('让 AI 帮你分析抓包，只需三步'):t('手动使用外部工具，只需三步')}</strong></div>
      {tab==='server'?<ol><li>{t('先启动 MCP，再在下方接入向导选择 AI 客户端和 HTTP / stdio，生成配置。')}</li><li>{t('将生成的配置合并到指定文件，保留其他服务，再重新连接 AI 客户端。')}</li><li>{t('让 AI「查看抓包状态，列出最近 5 条请求」，确认连接成功。')}</li></ol>:<ol><li>{t('点击「添加服务」，填写 HTTP 地址或本地 stdio 程序并保存。')}</li><li>{t('点击「连接」→「工具目录」，选择要使用的工具。')}</li><li>{t('按参数 Schema 填写 JSON 对象，再点击「手动调用工具」。')}</li></ol>}
      <p>{tab==='server'?t('MCP 默认端口是 1422，抓包代理默认端口是 8080；不要填混。'):t('本页不会自动调用工具。HTTP Token 只保留本次会话，重启后需要补填。')}</p>
    </aside>
    {tab==='client'?<McpClients/>:<section className="mcp-server-panel">
      <div className="mcp-server-heading"><div><h2>{t('AI 操作抓包工具')}</h2><p>{t('使用同一个抓包内核，查询记录、读取正文，并按授权发送请求或启停抓包。')}</p></div><span className={'mcp-running'+(running?' is-running':'')}>{running?t('运行中'):t('未启动')}</span></div>
      {error&&<p className="mcp-page-error" role="alert">{error}</p>}
      {status?.lastError&&status.lastError!==error&&<p className="mcp-page-error" role="alert">{status.lastError}</p>}
      {notice&&<p className="mcp-page-notice" role="status"><Check size={15}/>{notice}</p>}
      <div className="mcp-server-settings"><label className="mcp-port-field">{t('MCP 监听端口')}<input aria-label={t('MCP 监听端口')} inputMode="numeric" pattern="[0-9]*" maxLength={5} disabled={locked} value={port} onChange={event=>setPort(event.target.value.replace(/\D/g,''))}/><small>{t('仅监听本机 127.0.0.1，端口占用时会提示。')}</small></label>
      <fieldset disabled={locked}><legend><ShieldCheck size={16}/>{t('AI 可用权限')}</legend>
        <label><input type="checkbox" checked disabled/><span><strong>{t('查询抓包记录与正文')}</strong><small>{t('默认隐藏认证头和敏感查询参数；正文仍可能包含私人信息，请只连接可信客户端。')}</small></span></label>
        <label><input type="checkbox" checked={config.allowSend} onChange={event=>setConfig({...config,allowSend:event.target.checked})}/><span><strong>{t('允许发送与取消请求')}</strong><small>{t('AI 可发送普通 HTTP 请求，默认直连；发送结果保存到抓包记录。')}</small></span></label>
        <label><input type="checkbox" checked={config.allowCaptureControl} onChange={event=>setConfig({...config,allowCaptureControl:event.target.checked})}/><span><strong>{t('允许启动与停止抓包')}</strong><small>{t('操作当前应用共用的监听器，只能从本机地址启动。')}</small></span></label>
        <label><input type="checkbox" checked={!!config.allowWorkspaceWrite} onChange={event=>setConfig({...config,allowWorkspaceWrite:event.target.checked})}/><span><strong>{t('允许管理请求与集合')}</strong><small>{t('AI 可以查看和编辑请求草稿、创建集合并保存请求，修改会同步到界面。')}</small></span></label>
        <label><input type="checkbox" checked={!!config.allowRulesWrite} onChange={event=>setConfig({...config,allowRulesWrite:event.target.checked})}/><span><strong>{t('允许管理断点与替换响应')}</strong><small>{t('AI 可以修改拦截规则，读取等待中的正文，并放行、终止或替换被拦截的响应。')}</small></span></label>
        <label><input type="checkbox" checked={!!config.allowScriptsWrite} onChange={event=>setConfig({...config,allowScriptsWrite:event.target.checked})}/><span><strong>{t('允许管理和启用全局脚本')}</strong><small>{t('AI 可读取脚本和变量、编写影响后续请求的 JavaScript；变量可能包含凭据。保存时禁用，确认版本后才可启用。')}</small></span></label>
        <label><input type="checkbox" checked={config.includeSensitiveHeaders} onChange={event=>setConfig({...config,includeSensitiveHeaders:event.target.checked})}/><span><strong>{t('显示敏感请求头')}</strong><small>{t('开启后，Cookie、Authorization 等凭据可被 AI 读取。')}</small></span></label>
      </fieldset></div>
      {running&&<p className="mcp-page-hint">{t('修改端口或权限前，请先停止 MCP 服务。')}</p>}
      <div className="mcp-page-actions">{running?<button className="button" disabled={busy} onClick={()=>void configure(false)}><Square size={14}/>{t('停止 MCP')}</button>:<><button className="button primary" disabled={busy||!status} onClick={()=>void configure(true)}><Play size={14}/>{busy?t('处理中…'):t('启动 MCP')}</button><button className="button" disabled={busy||!status} onClick={()=>void configure(false)}><RefreshCw size={14}/>{t('保存配置')}</button></>}</div>
      <section className="mcp-connection"><h3>{t('客户端连接')}</h3><div><code>{status?.address??`http://127.0.0.1:${port||'1422'}/mcp`}</code><button className="button" disabled={busy||!running} onClick={()=>void copyConnection()}><Copy size={14}/>{t('复制连接配置')}</button></div><p>{t('在支持 Streamable HTTP 的 AI 客户端中填写 URL 和 Authorization 请求头。复制的 JSON 包含这两项，具体配置格式请按客户端要求填写。')}</p><p>{t('服务使用独立 Bearer Token，仅供本机连接。当前未提供远程 OAuth 接入。')}</p></section>
      <details className="mcp-tool-list"><summary>{t('可用工具')} ({status?.toolNames.length??0})</summary><div>{status?.toolNames.map(name=><code key={name}>{name}</code>)}</div></details>
      <McpSetup key={`${running}:${status?.address??''}`} running={running}/>
      <section className="mcp-incoming-clients"><h3>{t('最近活动的 AI 客户端')}</h3><p>{t('显示最近 5 分钟访问过 MCP 的客户端。HTTP 连接可能随时关闭，此列表表示最近活动，不代表持续在线。')}</p>{status?.clients?.length?<ul>{status.clients.map(client=><li key={client.id}><strong>{client.name}</strong><span>{client.version} · {client.protocol}</span><time dateTime={new Date(client.lastSeen).toISOString()}>{new Date(client.lastSeen).toLocaleTimeString()}</time><small>{t('访问次数')} {client.requests}</small></li>)}</ul>:<p>{t('还没有 AI 客户端访问，完成上方配置后即可在这里看到活动。')}</p>}</section>
    </section>}
    {helpOpen&&<McpHelp onClose={()=>setHelpOpen(false)}/>}
  </main>;
}
