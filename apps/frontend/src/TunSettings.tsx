import { t } from './i18n';
import { useState } from 'react';
import { invoke } from './api';
export interface TunStatus {available:boolean;running:boolean;helperPath:string;config?:{applications:string[];blockQuic:boolean;captureQuic?:boolean}|null;error?:string|null}
export default function TunSettings({status,onUpdate}:{status:TunStatus;onUpdate:(s:TunStatus)=>void}) {
 const [apps,setApps]=useState(status.config?.applications.join('\n')??'');
 const [block,setBlock]=useState(status.config?.blockQuic??false);
 const [capture,setCapture]=useState(status.config?.captureQuic??false);
 const [busy,setBusy]=useState(false);const [error,setError]=useState('');
 async function change(){
  setBusy(true);setError('');
  try {await invoke(status.running?'stop_tun':'start_tun',status.running?{}:{config:{applications:apps.split('\n').map(x=>x.trim()).filter(Boolean),blockQuic:block,captureQuic:capture}});}
  catch(e){setError(String(e));}
  finally {try{onUpdate(await invoke<TunStatus>('tun_status'));}catch{}setBusy(false);}
 }
 return <section className="tun-settings"><h3>{t("TUN · 指定应用")}</h3><p>{t("选中应用的 HTTP/HTTPS 进入抓包内核，其他流量直连。底层会创建虚拟网卡和系统路由，需要管理员/root 权限；HTTPS 仍需客户端信任 CA。")}</p>
 <label>{t("应用名称或绝对路径（每行一个）")}<textarea aria-label={t("TUN 应用列表")} disabled={busy||status.running} value={apps} onChange={e=>setApps(e.target.value)} placeholder={'chrome.exe\nC:\\Windows\\System32\\curl.exe'}/></label>
 <label><input type="checkbox" disabled={busy||status.running} checked={capture} onChange={e=>{setCapture(e.target.checked);if(e.target.checked)setBlock(false);}}/> {t("解密选中应用的 H3（QUIC / UDP 443）")}</label>
 <label><input type="checkbox" disabled={busy||status.running} checked={block} onChange={e=>{setBlock(e.target.checked);if(e.target.checked)setCapture(false);}}/> {t("阻止选中应用的 UDP 443，促使支持回退的客户端使用 HTTPS/TCP")}</label>
 <p>{t("H3 抓包要求客户端信任 CA，且握手包含可见 SNI；暂不支持 ECH、非 443 端口和跨域连接复用。两项均关闭时 UDP 直连。已建立连接需重新建立；停止显式代理不会停止 TUN。")}</p>
 {!status.available&&<p>{t("辅助进程未构建，请运行 scripts/build-tun.ps1。路径：")}<code>{status.helperPath}</code></p>}
 {(error||status.error)&&<p role="alert">{error||status.error}</p>}
 <button className="button" disabled={busy||(!status.running&&(!status.available||!apps.trim()))} onClick={()=>void change()}>{busy?t("处理中…"):status.running?t("停止 TUN"):t("启动 TUN")}</button>
 </section>;
}
