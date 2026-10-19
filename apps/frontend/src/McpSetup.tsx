import {useState} from 'react';
import {Check,Copy,ExternalLink,Settings2} from 'lucide-react';
import {invoke} from './api';
import {writeClipboard} from './clipboard';
import {t} from './i18n';
import {buildMcpConfiguration,type McpConfigurationClient,type McpConfigurationTransport,type McpConnectionInfo} from './mcpConfiguration';

export default function McpSetup({running}:{running:boolean}){
  const [client,setClient]=useState<McpConfigurationClient>('codex'),[transport,setTransport]=useState<McpConfigurationTransport>('http');
  const [connection,setConnection]=useState<McpConnectionInfo>(),[busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState('');
  const configuration=connection?buildMcpConfiguration(client,transport,connection):null;
  async function generate(){
    if(busy||!running)return;
    setBusy(true);setError('');setNotice('');
    try{const next=await invoke<McpConnectionInfo>('mcp_connection_info');buildMcpConfiguration(client,transport,next);setConnection(next)}
    catch(e){setError(String(e))}
    finally{setBusy(false)}
  }
  async function copy(){
    if(!configuration||busy)return;
    setBusy(true);setError('');setNotice('');
    try{await writeClipboard(configuration.text);setNotice(t('客户端配置已复制，请合并到指定配置文件并重新连接。'))}
    catch(e){setError(String(e))}
    finally{setBusy(false)}
  }
  return <section className="mcp-setup">
    <h3><Settings2 size={17}/>{t('AI 客户端接入向导')}</h3>
    <p>{t('选择你使用的 AI 客户端，生成对应配置。先启动上方 MCP 服务，再生成配置。')}</p>
    <div className="mcp-setup-fields"><label>{t('AI 客户端')}<select aria-label={t('AI 客户端')} disabled={busy} value={client} onChange={event=>{setClient(event.target.value as McpConfigurationClient);setNotice('')}}><option value="codex">Codex</option><option value="cursor">Cursor</option><option value="vscode">VS Code</option><option value="generic">{t('通用配置')}</option></select></label>
    <label>{t('连接方式')}<select aria-label={t('MCP 接入方式')} disabled={busy} value={transport} onChange={event=>{setTransport(event.target.value as McpConfigurationTransport);setConnection(undefined);setNotice('');setError('')}}><option value="http">Streamable HTTP</option><option value="stdio">stdio</option></select></label><button className="button" disabled={!running||busy} onClick={()=>void generate()}>{busy?t('处理中…'):t('生成连接配置')}</button></div>
    <p>{transport==='stdio'?t('AI 会启动应用的桥接进程，连接到当前正在运行的抓包工具；无需另外安装 Node.js。'):t('AI 直接连接本机 MCP 地址，应用需要保持运行。')}</p>
    {configuration&&<><div className="mcp-setup-target">{configuration.target?<><span>{t('配置文件')}</span><code>{configuration.target}</code></>:<span>{t('在客户端中填写下面的连接字段。')}</span>}{configuration.documentationUrl&&<a href={configuration.documentationUrl} target="_blank" rel="noreferrer"><ExternalLink size={13}/>{t('官方配置说明')}</a>}</div>
      <p>{t('配置包含访问 Token，请只保存在可信客户端中。合并时保留其他服务；若已有 httpCapture 条目，请更新原条目。')}</p>
      <pre className="mcp-setup-preview" aria-label={t('AI 客户端配置预览')} tabIndex={0}>{configuration.text}</pre><button className="button primary" disabled={busy} onClick={()=>void copy()}><Copy size={14}/>{t('复制客户端配置')}</button>
      <ol><li>{t('将配置合并到指定文件，或在 AI 客户端设置里填写对应字段。')}</li><li>{t('重新连接 MCP，必要时重启 AI 客户端。')}</li><li>{t('对 AI 说：查看抓包状态，列出最近 5 条请求。')}</li></ol></>}
    {error&&<p className="mcp-page-error" role="alert">{error}</p>}{notice&&<p className="mcp-page-notice" role="status"><Check size={15}/>{notice}</p>}
  </section>;
}
