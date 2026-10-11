import { useEffect, useState } from 'react';
import { invoke } from './api';
import { t } from './i18n';
interface ProxyState { supported:boolean; active:boolean; pendingRecovery:boolean; message:string }
interface CaState { supported:boolean; installed:boolean|null; scope:string; certificate:{ subject:string; sha256:string; notAfter:string } }
export default function SystemSettings({running,useSystemProxy,onUseSystemProxy}:{running:boolean;useSystemProxy:boolean;onUseSystemProxy:(value:boolean)=>void}) {
  const [proxy,setProxy]=useState<ProxyState>(); const [ca,setCa]=useState<CaState>();
  const [error,setError]=useState(''); const [busy,setBusy]=useState(false);const [confirm,setConfirm]=useState(false);
  async function refresh() {
    const results=await Promise.allSettled([invoke<ProxyState>('system_proxy_status'),invoke<CaState>('ca_status')]);
    if(results[0].status==='fulfilled')setProxy(results[0].value);
    if(results[1].status==='fulfilled')setCa(results[1].value);
    setError(results.filter(r=>r.status==='rejected').map(r=>String((r as PromiseRejectedResult).reason)).join('\n'));
  }
  useEffect(()=>{void refresh();},[running]);
  async function action(command:string,args:Record<string,unknown>) {
    setBusy(true);setError('');try{await invoke(command,args);setConfirm(false);await refresh();}
    catch(e){await refresh();setError(String(e));}finally{setBusy(false);}
  }
  return <section className="system-settings">
    <h3>{t("系统代理")}</h3>
    <label className="setting-label" style={{display:'flex',alignItems:'center',gap:8}}>
      <input type="checkbox" style={{width:'auto'}} checked={useSystemProxy} disabled={busy || (proxy?.supported === false && !useSystemProxy)} onChange={e=>onUseSystemProxy(e.target.checked)} />
      {t("启动抓包时同时设置为系统代理")}
    </label>
    <p className="muted">{t("默认关闭，仅监听。此选项可随时修改，下次启动抓包时生效，不会立即改变当前系统代理。")}</p>
    {running && <p className="muted">{t("当前正在监听：如需立即切换系统代理，请使用下方按钮。")}</p>}
    {!proxy && <p role="status">{t("系统代理接口尚不可用，可先选择启动偏好；实际启用前会检查内核支持。")}</p>}
    <p className="muted">{t("手动启用后保存原配置；停止抓包或正常退出时恢复。异常退出后，下次启动尝试恢复。外部修改发生冲突时不会覆盖。")}</p>
    {proxy && <><p>{proxy.active?t("当前由 HTTP Capture 管理"):proxy.pendingRecovery?t("有待恢复的代理配置"):t("未接管系统代理")}</p>
      {!proxy.supported && <p>{t("此平台暂需手动配置系统代理。")}</p>}
      {proxy.message && <p role="alert">{proxy.message}</p>}
      <button className="button" disabled={busy||!proxy.supported||!running||proxy.pendingRecovery} onClick={()=>void action('set_system_proxy',{enabled:true})}>{t("设为系统代理")}</button>{' '}
      <button className="button" disabled={busy||!proxy.pendingRecovery} onClick={()=>void action('set_system_proxy',{enabled:false})}>{t("恢复原代理")}</button>
    </>}
    <h3>{t("CA 信任管理")}</h3>
    {ca && <><p>{ca.certificate.subject}</p><p className="muted">{ca.scope}</p>
      <p>{ca.installed===null?t("请导出 CA 并在客户端手动安装"):ca.installed?t("本证书已安装到当前用户根证书库"):t("当前用户根证书库中未安装本证书")}</p>
      <p className="muted">{t("其他设备、Firefox 或使用独立信任库的应用可能仍需单独安装。")}</p>
      <p>SHA-256: <code style={{overflowWrap:'anywhere'}}>{ca.certificate.sha256}</code></p><p>{t("有效期至")}: {ca.certificate.notAfter}</p>
      {ca.supported && <><button className="button" disabled={busy} onClick={()=>setConfirm(!confirm)}>{ca.installed?t("卸载本 CA"):t("安装并信任本 CA")}</button>
      {confirm && <div className="setting-card"><div><p>{ca.installed?t("仅移除指纹匹配的本 CA，不删除证书文件；HTTPS 解密将需要重新信任。"):t("将此 CA 加入当前用户根证书库，允许本工具签发 HTTPS 解密证书。")}</p><button className="button primary" disabled={busy} onClick={()=>void action('set_ca_trust',{install:!ca.installed,sha256:ca.certificate.sha256})}>{t("确认")}</button>{' '}<button className="button" disabled={busy} onClick={()=>setConfirm(false)}>{t("取消")}</button></div></div>}</>}
    </>}
    <p><button className="text-button" disabled={busy} onClick={()=>void refresh()}>{t("刷新状态")}</button></p>
    {busy&&<p role="status">{t("处理中…")}</p>}{error&&<p role="alert" style={{whiteSpace:'pre-wrap'}}>{error}</p>}
  </section>;
}
