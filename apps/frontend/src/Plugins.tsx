import {useEffect,useState} from 'react';
import {invoke} from './api';
import {t} from './i18n';
import { PackageOpen } from 'lucide-react';
interface Plugin {id:string;name:string;version:string;apiVersion:number;enabled:boolean;scope:string;urlPattern:string;before:string;after:string}
export default function Plugins(){
 const [items,setItems]=useState<Plugin[]>([]),[logs,setLogs]=useState<string[]>([]),[candidate,setCandidate]=useState<Plugin|null>(null),[error,setError]=useState(''),[busy,setBusy]=useState(false),[ready,setReady]=useState(false);
 async function refresh(){try{const [list,output]=await Promise.all([invoke<Plugin[]>('plugins_list'),invoke<string[]>('plugin_logs')]);setItems(list);setLogs(output);setReady(true);setError('')}catch(e){setError(String(e))}}
 useEffect(()=>{void refresh()},[]);
 async function save(next:Plugin[]){setBusy(true);try{await invoke('plugins_save',{plugins:next});setItems(next);setCandidate(null);setError('')}catch(e){setError(String(e))}finally{setBusy(false)}}
 return <section className="request-details plugins-workspace"><header className="plugins-heading"><h1>{t('插件管理')}</h1><p>{t('本地 JavaScript 插件；安装后默认禁用。启用后可读取并修改匹配的请求和响应，不提供文件、网络或系统命令权限。')}</p></header>
 <div className="plugins-toolbar"><input aria-label={t('导入插件')} type="file" accept=".json" disabled={busy||!ready} onChange={async e=>{const f=e.target.files?.[0];e.target.value='';if(!f)return;try{if(f.size>150000)throw Error('Plugin package exceeds 150 KB');const p=JSON.parse(await f.text());if(!p||typeof p.id!=='string'||typeof p.name!=='string'||typeof p.before!=='string'||typeof p.after!=='string')throw Error('Invalid plugin manifest');setCandidate({...p,enabled:false});setError('')}catch(e){setError(String(e))}}}/><button className="button" disabled={busy} onClick={()=>void refresh()}>{t('刷新')}</button></div>
 {error&&<p role="alert">{error}</p>}
 {ready&&!items.length&&!candidate&&<div className="secondary-empty plugins-empty"><PackageOpen size={28} aria-hidden="true"/><strong>{t('尚未安装插件')}</strong><p>{t('导入本地插件包后，可在这里管理启用状态与执行顺序。')}</p></div>}
 {candidate&&<article className="setting-card"><div><h3>{candidate.name} · {candidate.version}</h3><p>{candidate.id} · API {candidate.apiVersion} · {candidate.scope} · {candidate.urlPattern}</p><details><summary>{t('查看插件代码')}</summary><pre>{candidate.before+'\n\n'+candidate.after}</pre></details><button className="button" disabled={busy||items.some(p=>p.id===candidate.id)} onClick={()=>void save([...items,candidate])}>{t('安装插件')}</button><button className="button" onClick={()=>setCandidate(null)}>{t('取消')}</button>{items.some(p=>p.id===candidate.id)&&<p>{t('插件 ID 已存在，请先卸载旧版本。')}</p>}</div></article>}
 {items.map((p,i)=><article className="setting-card" key={p.id}><div style={{width:'100%'}}><h3>{p.name} <small>{p.version}</small></h3><p>{p.id} · {p.scope} · {p.urlPattern}</p><label><input type="checkbox" disabled={busy} checked={p.enabled} onChange={e=>void save(items.map(v=>v.id===p.id?{...v,enabled:e.target.checked}:v))}/>{t('启用插件')}</label><button className="button" disabled={busy||i===0} onClick={()=>{const next=[...items];[next[i-1],next[i]]=[next[i],next[i-1]];void save(next)}}>{t('上移')}</button><button className="button" disabled={busy} onClick={()=>void save(items.filter(v=>v.id!==p.id))}>{t('卸载插件')}</button><details><summary>{t('查看插件代码')}</summary><pre>{p.before+'\n\n'+p.after}</pre></details></div></article>)}
 <section className="plugins-logs"><h2>{t('插件日志')}</h2><pre style={{whiteSpace:'pre-wrap'}}>{logs.join('\n')||'—'}</pre></section></section>
}
