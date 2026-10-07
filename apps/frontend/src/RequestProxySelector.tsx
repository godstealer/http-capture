import { t } from './i18n';
import { useEffect, useState } from 'react';
import { invoke } from './api';
import type { RequestDraft, UpstreamProfile } from './types';

export default function RequestProxySelector({ draft, onChange, onManage }: {
 draft: RequestDraft; onChange: (draft: RequestDraft) => void; onManage: () => void;
}) {
 const [profiles,setProfiles]=useState<UpstreamProfile[]>([]);
 const [error,setError]=useState('');
 const [loading,setLoading]=useState(true);
 const selected=draft.upstreamProfileId??'';
 const supported=['auto','native','h2','h3','httpcloak','wreq'].includes(draft.engine??'auto');
 const supportsProfile=(p:UpstreamProfile)=>supported&&(draft.engine!=='h3'||p.url.startsWith('socks5://'));
 async function refresh(){setLoading(true);try{setProfiles(await invoke<UpstreamProfile[]>('upstream_profiles'));setError('');}catch(e){setError(String(e));}finally{setLoading(false);}}
 useEffect(()=>{void refresh();const focus=()=>void refresh();window.addEventListener('focus',focus);return()=>window.removeEventListener('focus',focus);},[]);
 const profile=profiles.find(p=>p.id===selected);
 return <div className="engine-selector"><label>{t("本请求代理")}<select aria-label={t("本请求代理")} value={selected} onChange={e=>onChange({...draft,upstreamProfileId:e.target.value||null})}>
 <option value="">{t("直连")}</option>
 {selected&&!profile&&<option value={selected} disabled>{loading?t("正在读取代理…"):t("代理已删除或暂不可用")}</option>}
 {profiles.map(p=><option key={p.id} value={p.id} disabled={p.needsPassword||!supportsProfile(p)}>{p.name} · {p.url}{p.needsPassword?t(" · 需补填密码"):''}</option>)}
 </select></label><div><button onClick={()=>void refresh()} disabled={loading}>{t("刷新代理列表")}</button><button onClick={onManage}>{t("管理代理")}</button></div>
 <span>{t("仅作用于当前请求，不改变抓包的上游代理。发送开始时固定代理配置，失败不回退直连。")}</span>
 {!supported&&<p role="alert">{t("当前引擎不支持上游代理，请选择直连或切换到 Auto、native、h2、httpcloak、wreq。")}</p>}
 {draft.engine==='h3'&&<p role={profile&&!supportsProfile(profile)?'alert':undefined}>{t("H3 仅支持具有 UDP ASSOCIATE 能力的 SOCKS5 上游；不支持普通 HTTP 代理，失败不回退直连。")}</p>}
 {selected&&profile?.needsPassword&&<p role="alert">{t("该代理需要补填密码，请打开管理代理。")}</p>}
 {error&&<p role="alert">{error}</p>}</div>;
}
