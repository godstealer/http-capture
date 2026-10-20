import { useEffect, useId, useRef, useState } from 'react';
import { Gauge, Plus, RefreshCw, Save, Trash2 } from 'lucide-react';
import { invoke } from './api';
import { t } from './i18n';
import './request-blocking.css';

interface FaultRule {id:string;enabled:boolean;matchType:'urlContains'|'urlRegex'|'host';pattern:string;method:string;delayMs:number;rateLimit:{requests:number;windowMs:number}|null}
interface Config {enabled:boolean;scope:'capture'|'all';rules:FaultRule[]}
interface Snapshot {config:Config;version:string}
const initial=():Config=>({enabled:false,scope:'capture',rules:[]});

export default function FaultRulesPage({connected}:{connected:boolean}) {
  const heading=useId();
  const [snapshot,setSnapshot]=useState<Snapshot|null>(null),[draft,setDraft]=useState<Config>(initial);
  const [busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState(''),[reload,setReload]=useState(0);
  const edited=useRef(false);
  const dirty=snapshot!==null&&JSON.stringify(snapshot.config)!==JSON.stringify(draft);
  const locked=!connected||busy||snapshot===null;
  useEffect(()=>{
    if(!connected)return;
    let live=true;setBusy(true);setError('');
    invoke<Snapshot>('fault_rules').then(value=>{if(live&&!edited.current){setSnapshot(value);setDraft(structuredClone(value.config));setNotice('')}}).catch(e=>{if(live)setError(String(e).includes('404')?t('当前内核尚不支持故障模拟，请更新并重启内核。'):String(e))}).finally(()=>{if(live)setBusy(false)});
    return()=>{live=false};
  },[connected,reload]);
  function update(value:Config){edited.current=snapshot===null||JSON.stringify(value)!==JSON.stringify(snapshot.config);setDraft(value);setNotice('');setError('')}
  function patch(id:string,value:Partial<FaultRule>){update({...draft,rules:draft.rules.map(rule=>rule.id===id?{...rule,...value}:rule)})}
  async function save(){
    if(locked||!snapshot||!dirty)return;
    if(draft.rules.some(rule=>!rule.pattern.trim()||(!rule.delayMs&&!rule.rateLimit))){setError(t('请填写匹配内容，并至少设置延迟或频率限制。'));return;}
    setBusy(true);setError('');setNotice('');
    try{const value=await invoke<Snapshot>('configure_fault_rules',{config:draft,expectedVersion:snapshot.version});edited.current=false;setSnapshot(value);setDraft(structuredClone(value.config));setNotice(t('模拟规则已保存；计数已重置，下一条请求生效。'))}catch(e){setError(String(e))}finally{setBusy(false)}
  }
  return <section className="request-blocking-page fault-rules-page" aria-labelledby={heading}>
    <header className="blocking-heading"><div className="blocking-heading-copy"><div className="blocking-title"><Gauge size={22}/><h1 id={heading}>{t('故障模拟')}</h1>{snapshot&&<span className={'blocking-status'+(snapshot.config.enabled?' is-enabled':'')}>{snapshot.config.enabled?t('已开启'):t('已关闭')}</span>}</div><p>{t('在发送前模拟请求延迟和频率限制，测试超时、重试和限流处理。')}</p></div><button type="button" disabled={!connected||busy||dirty} onClick={()=>setReload(value=>value+1)}><RefreshCw size={15}/>{t('重新加载')}</button></header>
    {!connected&&<p className="blocking-message" role="status">{t('连接抓包内核后可设置模拟规则。')}</p>}
    {busy&&<p className="blocking-message" role="status">{t('正在处理模拟规则…')}</p>}{error&&<p className="blocking-message blocking-error" role="alert">{error}</p>}{notice&&<p className="blocking-message blocking-notice" role="status">{notice}</p>}
    <form className="blocking-form" onSubmit={event=>{event.preventDefault();void save()}}><fieldset className="blocking-editor" disabled={locked}>
      <div className="blocking-settings"><label className="blocking-switch"><input type="checkbox" checked={draft.enabled} onChange={e=>update({...draft,enabled:e.target.checked})}/>{t('启用故障模拟')}</label><label className="blocking-scope">{t('作用范围')}<select value={draft.scope} onChange={e=>update({...draft,scope:e.target.value as Config['scope']})}><option value="capture">{t('仅抓包流量')}</option><option value="all">{t('抓包和手动请求')}</option></select></label></div>
      <p className="blocking-help">{t('只执行第一条匹配规则，在请求脚本和拦截编辑之后匹配最终 URL。仅处理 HTTP 请求，不处理 WebSocket、CONNECT 直通或正文带宽。')}</p>
      <div className="blocking-rules-heading"><h2>{t('模拟规则')}</h2><span>{draft.rules.length} / 100</span></div>
      <div className="blocking-rule-list">{!draft.rules.length&&<div className="blocking-empty"><Gauge size={26}/><strong>{t('尚未添加模拟规则')}</strong><p>{t('例如延迟 1000 ms，或每 1000 ms 最多允许 5 次请求。')}</p></div>}
        {draft.rules.map((rule,index)=><article key={rule.id} className={'blocking-rule'+(rule.enabled?'':' is-disabled')}><div className="blocking-rule-heading"><label><input type="checkbox" checked={rule.enabled} onChange={e=>patch(rule.id,{enabled:e.target.checked})}/>{t('启用规则 {v0}',{v0:index+1})}</label><button type="button" className="blocking-remove" aria-label={t('删除模拟规则 {v0}',{v0:index+1})} onClick={()=>update({...draft,rules:draft.rules.filter(value=>value.id!==rule.id)})}><Trash2 size={15}/></button></div>
          <div className="blocking-rule-fields"><label>{t('匹配方式')}<select value={rule.matchType} onChange={e=>patch(rule.id,{matchType:e.target.value as FaultRule['matchType']})}><option value="urlContains">{t('URL 包含')}</option><option value="urlRegex">{t('URL 正则表达式')}</option><option value="host">{t('域名精确匹配')}</option></select></label><label>{t('匹配内容')}<input required maxLength={2048} value={rule.pattern} spellCheck={false} placeholder={rule.matchType==='host'?'example.com':'/api/'} onChange={e=>patch(rule.id,{pattern:e.target.value})}/></label><label>{t('方法')}<input value={rule.method} maxLength={32} spellCheck={false} placeholder={t('任意方法')} onChange={e=>patch(rule.id,{method:e.target.value.toUpperCase()})}/></label></div>
          <div className="fault-actions"><label>{t('请求延迟（ms，0 关闭）')}<input type="number" required min={0} max={30000} step={1} value={rule.delayMs} onChange={e=>patch(rule.id,{delayMs:Number(e.target.value)})}/></label><label className="fault-limit-switch"><input type="checkbox" checked={rule.rateLimit!==null} onChange={e=>patch(rule.id,{rateLimit:e.target.checked?{requests:5,windowMs:1000}:null})}/>{t('限制请求频率')}</label>{rule.rateLimit&&<><label>{t('窗口内允许次数')}<input type="number" required min={1} max={10000} step={1} value={rule.rateLimit.requests} onChange={e=>patch(rule.id,{rateLimit:{...rule.rateLimit!,requests:Number(e.target.value)}})}/></label><label>{t('窗口时长（ms）')}<input type="number" required min={100} max={60000} step={1} value={rule.rateLimit.windowMs} onChange={e=>patch(rule.id,{rateLimit:{...rule.rateLimit!,windowMs:Number(e.target.value)}})}/></label></>}</div>
          {rule.rateLimit&&<p className="blocking-rule-hint">{t('每条规则共享一个固定窗口；超额请求本地返回 429 和 Retry-After。保存或重启会清空计数，取消的已允许请求也计次。')}</p>}
        </article>)}
      </div><button type="button" className="blocking-add" disabled={draft.rules.length>=100} onClick={()=>update({...draft,rules:[...draft.rules,{id:crypto.randomUUID(),enabled:true,matchType:'urlContains',pattern:'',method:'',delayMs:1000,rateLimit:null}]})}><Plus size={15}/>{t('添加规则')}</button>
      <div className="blocking-actions"><span>{dirty?t('有未应用的修改'):t('已与保存的配置同步')}</span><button type="button" disabled={!dirty} onClick={()=>{edited.current=false;if(snapshot)setDraft(structuredClone(snapshot.config));setError('');setNotice('')}}>{t('取消修改')}</button><button type="submit" className="blocking-save" disabled={!dirty}><Save size={15}/>{t('保存规则')}</button></div>
    </fieldset></form>
  </section>;
}
