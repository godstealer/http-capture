import {useEffect, useRef, useState} from 'react';
import {History, RefreshCw, ShieldCheck} from 'lucide-react';
import {invoke} from './api';
import {t} from './i18n';
import './audit.css';

interface Entry {
  id:string;startedAt:number;finishedAt:number|null;actor:string;entryPoint:string;
  clientFamily:string;clientRef:string|null;operation:string;targetKind:string;
  targetId:string|null;outcome:string;reason:string|null;
}
interface Page {entries:Entry[];nextCursor:string|null;retainedCount:number;retentionLimit:number}
const pageSize=50;
function operationLabel(operation:string) {
  switch(operation) {
    case 'send_request':return t('发送请求');case 'cancel_request':return t('取消请求');
    case 'start_capture':return t('启动抓包监听');case 'stop_capture':return t('停止抓包监听');
    case 'create_collection':return t('创建集合');case 'create_request':return t('创建请求草稿');
    case 'update_request':return t('修改请求草稿');case 'save_request_to_collection':return t('保存请求到集合');
    case 'configure_interception':return t('修改断点配置');case 'resolve_interception':return t('处理等待中的拦截');
    case 'set_global_scripts':return t('保存全局脚本');case 'set_global_scripts_enabled':return t('更改全局脚本启用状态');
    default:return t('未知操作');
  }
}
function targetLabel(kind:string) {
  switch(kind) {
    case 'request':return t('请求');case 'listener':return t('抓包监听器');case 'collection':return t('集合');
    case 'requestEditor':return t('请求草稿');case 'interceptionConfig':return t('断点配置');
    case 'pendingInterception':return t('等待中的拦截');case 'globalScripts':return t('全局脚本');
    default:return t('未知目标');
  }
}
function clientLabel(family:string) {
  switch(family) {case 'codex':return 'Codex';case 'cursor':return 'Cursor';case 'vsCode':return 'VS Code';case 'claude':return 'Claude';case 'other':return t('其他客户端');default:return t('未提供客户端信息')}
}
function outcomeLabel(outcome:string) {
  switch(outcome) {case 'succeeded':return t('操作成功');case 'failed':return t('操作失败');case 'rejected':return t('操作被拒绝');case 'cancelled':return t('操作已取消');default:return t('结果未确认')}
}
function reasonLabel(reason:string|null) {
  switch(reason) {
    case 'permissionDenied':return t('没有所需操作权限');case 'invalidArguments':return t('操作参数无效');
    case 'conflict':return t('配置版本冲突');case 'notFound':return t('操作目标不存在');case 'cancelled':return t('操作被取消');
    case 'operationFailed':return t('执行未完成');case 'unavailable':return t('服务暂不可用');
    case 'responseLimit':return t('返回结果超过限制');case 'internal':return t('内部处理失败');default:return '';
  }
}
function timeValue(value:number|null) {
  if(value===null||!Number.isFinite(value))return undefined;
  const date=new Date(value);return Number.isNaN(date.getTime())?undefined:date;
}

export default function AuditPage() {
  const [page,setPage]=useState<Page>(),[busy,setBusy]=useState(false),[error,setError]=useState('');
  const mounted=useRef(false),pending=useRef(false),generation=useRef(0);
  async function load(more=false) {
    if(pending.current)return;
    const cursor=more?page?.nextCursor:undefined;
    if(more&&!cursor)return;
    pending.current=true;const epoch=++generation.current;setBusy(true);setError('');
    try {
      const next=await invoke<Page>('mcp_audit',{...(cursor?{cursor}:{}),limit:pageSize});
      if(!mounted.current||epoch!==generation.current)return;
      setPage(previous=>more&&previous?{...next,entries:[...previous.entries,...next.entries.filter(entry=>!previous.entries.some(existing=>existing.id===entry.id))]}:next);
    } catch(cause) {
      if(mounted.current&&epoch===generation.current)setError(/Unknown command: mcp_audit|\bHTTP\s+404\b/i.test(String(cause))
        ? t('当前内核不支持操作审计，请重启内核加载新版本。')
        : t('无法读取操作审计，现有记录保持不变，请稍后重试。'));
    } finally {
      if(epoch===generation.current){pending.current=false;if(mounted.current)setBusy(false)}
    }
  }
  useEffect(()=>{
    mounted.current=true;pending.current=false;void load();
    return()=>{mounted.current=false;pending.current=false;generation.current++};
  },[]);
  return <section className="audit-page" aria-label={t('MCP 操作审计')}>
    <header className="audit-heading"><div><h2><History size={20}/>{t('MCP 操作审计')}</h2><p>{t('查看 AI 通过 MCP 发起的写操作和结果。')}</p></div><button type="button" className="button" disabled={busy} onClick={()=>void load()}><RefreshCw size={14}/>{busy?t('读取中…'):t('刷新审计')}</button></header>
    <aside className="audit-scope"><ShieldCheck size={18}/><div>
      <p>{t('仅记录认证后的 MCP 发送、监听、请求草稿、集合、断点及全局脚本写操作；界面手动操作、权限设置变更、读取工具和认证前拒绝不在本页范围内。')}</p>
      <p>{t('不保存请求参数、正文、凭据、Token、变量或脚本源码。客户端分类与摘要来自自报信息，不代表已认证的个人身份；所有调用者共用本机 MCP Token 权限。')}</p>
      <p>{t('只保留最近 10000 条操作，重启后仍可查看。结果未确认表示操作可能仍在进行，或结束结果未能落盘；请先核对实际状态，不要直接重复操作。')}</p>
    </div></aside>
    {error&&<p className="audit-error" role="alert">{error}</p>}
    <div className="audit-summary" role="status">{page?t('本机保留 {v0} 条，当前显示 {v1} 条。',{v0:page.retainedCount,v1:page.entries.length}):busy?t('正在读取本机审计记录…'):t('尚未读取审计记录。')}</div>
    {page?.entries.length?<div className="audit-table-scroll"><table><thead><tr><th scope="col">{t('操作时间')}</th><th scope="col">{t('客户端与入口')}</th><th scope="col">{t('操作与目标')}</th><th scope="col">{t('操作结果')}</th></tr></thead><tbody>{page.entries.map(entry=>{
      const started=timeValue(entry.startedAt),finished=timeValue(entry.finishedAt);
      return <tr key={entry.id}><td><time dateTime={started?.toISOString()}>{started?.toLocaleString()??'—'}</time><small>{t('审计编号')} {entry.id}</small>{finished&&<small>{t('结束时间')} {finished.toLocaleString()}</small>}</td>
        <td><strong>{clientLabel(entry.clientFamily)}</strong><small>MCP HTTP · {t('共享 Token 持有者')}</small>{entry.clientRef&&<code title={entry.clientRef}>{t('客户端摘要')} {entry.clientRef.slice(0,12)}</code>}</td>
        <td><strong>{operationLabel(entry.operation)}</strong><small>{targetLabel(entry.targetKind)}</small>{entry.targetId&&<code className="audit-target-id">{entry.targetId}</code>}</td>
        <td><span className={'audit-outcome audit-outcome-'+(['succeeded','failed','rejected','cancelled'].includes(entry.outcome)?entry.outcome:'pending')}>{outcomeLabel(entry.outcome)}</span>{reasonLabel(entry.reason)&&<small>{reasonLabel(entry.reason)}</small>}{entry.outcome==='pending'&&<small>{t('先确认实际状态，再决定是否重试。')}</small>}</td>
      </tr>;
    })}</tbody></table></div>:page?<div className="audit-empty"><History size={28}/><strong>{t('暂无 MCP 写操作记录')}</strong><p>{t('AI 首次发起此页范围内的操作后，刷新即可查看。')}</p></div>:null}
    {page?.nextCursor&&<div className="audit-pagination"><button type="button" className="button" disabled={busy} onClick={()=>void load(true)}>{busy?t('读取中…'):t('加载更早记录')}</button><span>{t('每次最多读取 50 条，按新到旧显示。')}</span></div>}
  </section>;
}
