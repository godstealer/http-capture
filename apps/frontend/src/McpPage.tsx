import {useEffect, useRef, useState} from 'react';
import {Cable, Check, Copy, Play, RefreshCw, ShieldCheck, Square} from 'lucide-react';
import {invoke} from './api';
import {writeClipboard} from './clipboard';
import {t} from './i18n';
import McpClients from './McpClients';
import './mcp-page.css';

interface Config {enabled:boolean;port:number;allowSend:boolean;allowCaptureControl:boolean;includeSensitiveHeaders:boolean}
interface Status {config:Config;running:boolean;address:string|null;lastError:string|null;toolNames:string[]}
const defaults:Config={enabled:false,port:1422,allowSend:false,allowCaptureControl:false,includeSensitiveHeaders:false};

export default function McpPage() {
  const [tab,setTab]=useState('server'), [status,setStatus]=useState<Status>(), [config,setConfig]=useState(defaults);
  const [port,setPort]=useState('1422'), [busy,setBusy]=useState(false), [error,setError]=useState(''), [notice,setNotice]=useState('');
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
      }catch(e){if(mounted.current&&generation===epoch.current)setError(String(e))}
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
    }catch(e){if(mounted.current)setError(String(e))}
    finally{pending.current=false;if(mounted.current)setBusy(false)}
  }
  async function copyConnection(){
    if(pending.current)return;
    pending.current=true;epoch.current++;setBusy(true);setError('');setNotice('');
    try{
      const connection=await invoke('mcp_connection_info');
      await writeClipboard(JSON.stringify(connection,null,2));
      if(mounted.current)setNotice(t('连接配置已复制，包含访问 Token，请只交给可信的 AI 客户端。'));
    }catch(e){if(mounted.current)setError(String(e))}
    finally{pending.current=false;if(mounted.current)setBusy(false)}
  }
  const running=!!status?.running, locked=busy||running||!status;
  return <main className="mcp-page">
    <header className="mcp-page-heading"><div><h1><Cable size={23}/>MCP</h1><p>{t('让 AI 操作当前抓包工具，或连接外部 MCP 服务。')}</p></div></header>
    <nav className="mcp-page-tabs" aria-label={t('MCP 功能')}><button aria-current={tab==='server'?'page':undefined} className={tab==='server'?'active':''} onClick={()=>setTab('server')}>{t('AI 接入')}</button><button aria-current={tab==='client'?'page':undefined} className={tab==='client'?'active':''} onClick={()=>setTab('client')}>{t('外部服务')}</button></nav>
    {tab==='client'?<McpClients/>:<section className="mcp-server-panel">
      <div className="mcp-server-heading"><div><h2>{t('AI 操作抓包工具')}</h2><p>{t('使用同一个抓包内核，查询记录、读取正文，并按授权发送请求或启停抓包。')}</p></div><span className={'mcp-running'+(running?' is-running':'')}>{running?t('运行中'):t('未启动')}</span></div>
      {error&&<p className="mcp-page-error" role="alert">{error}</p>}
      {status?.lastError&&status.lastError!==error&&<p className="mcp-page-error" role="alert">{status.lastError}</p>}
      {notice&&<p className="mcp-page-notice" role="status"><Check size={15}/>{notice}</p>}
      <div className="mcp-server-settings"><label className="mcp-port-field">{t('MCP 监听端口')}<input aria-label={t('MCP 监听端口')} inputMode="numeric" pattern="[0-9]*" maxLength={5} disabled={locked} value={port} onChange={event=>setPort(event.target.value.replace(/\D/g,''))}/><small>{t('仅监听本机 127.0.0.1，端口占用时会提示。')}</small></label>
      <fieldset disabled={locked}><legend><ShieldCheck size={16}/>{t('AI 可用权限')}</legend>
        <label><input type="checkbox" checked disabled/><span><strong>{t('查询抓包记录与正文')}</strong><small>{t('默认隐藏认证头和敏感查询参数；正文仍可能包含私人信息，请只连接可信客户端。')}</small></span></label>
        <label><input type="checkbox" checked={config.allowSend} onChange={event=>setConfig({...config,allowSend:event.target.checked})}/><span><strong>{t('允许发送与取消请求')}</strong><small>{t('AI 可以通过当前转发设置访问目标地址，发送结果写入抓包记录。')}</small></span></label>
        <label><input type="checkbox" checked={config.allowCaptureControl} onChange={event=>setConfig({...config,allowCaptureControl:event.target.checked})}/><span><strong>{t('允许启动与停止抓包')}</strong><small>{t('操作当前应用共用的监听器，只能从本机地址启动。')}</small></span></label>
        <label><input type="checkbox" checked={config.includeSensitiveHeaders} onChange={event=>setConfig({...config,includeSensitiveHeaders:event.target.checked})}/><span><strong>{t('显示敏感请求头')}</strong><small>{t('开启后，Cookie、Authorization 等凭据可被 AI 读取。')}</small></span></label>
      </fieldset></div>
      {running&&<p className="mcp-page-hint">{t('修改端口或权限前，请先停止 MCP 服务。')}</p>}
      <div className="mcp-page-actions">{running?<button className="button" disabled={busy} onClick={()=>void configure(false)}><Square size={14}/>{t('停止 MCP')}</button>:<><button className="button primary" disabled={busy||!status} onClick={()=>void configure(true)}><Play size={14}/>{busy?t('处理中…'):t('启动 MCP')}</button><button className="button" disabled={busy||!status} onClick={()=>void configure(false)}><RefreshCw size={14}/>{t('保存配置')}</button></>}</div>
      <section className="mcp-connection"><h3>{t('客户端连接')}</h3><div><code>{status?.address??`http://127.0.0.1:${port||'1422'}/mcp`}</code><button className="button" disabled={busy||!running} onClick={()=>void copyConnection()}><Copy size={14}/>{t('复制连接配置')}</button></div><p>{t('在支持 Streamable HTTP 的 AI 客户端中填写 URL 和 Authorization 请求头。复制的 JSON 包含这两项，具体配置格式请按客户端要求填写。')}</p><p>{t('服务使用独立 Bearer Token，仅供本机连接。当前未提供远程 OAuth 接入。')}</p></section>
      <details className="mcp-tool-list"><summary>{t('可用工具')} ({status?.toolNames.length??0})</summary><div>{status?.toolNames.map(name=><code key={name}>{name}</code>)}</div></details>
    </section>}
  </main>;
}
