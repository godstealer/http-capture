import { useEffect, useRef, useState } from 'react';
import { Globe2, Check, X, Clock3, CircleAlert } from 'lucide-react';
import { invoke } from './api';
import { t } from './i18n';
import './system-proxy-toggle.css';
interface Status { supported:boolean;active:boolean;pendingRecovery:boolean;message:string }
export default function SystemProxyToggle({running,connected,locked,preferred,onPreference,onError}:{running:boolean;connected:boolean;locked:boolean;preferred:boolean;onPreference:(v:boolean)=>void;onError:(s:string)=>void}) {
  const [status,setStatus]=useState<Status>();const [busy,setBusy]=useState(false);const changing=useRef(false);
  useEffect(()=>{
    let disposed=false;let timer:ReturnType<typeof setTimeout>;
    async function refresh(){
      if(!changing.current){try{const result=await invoke<Status>('system_proxy_status');if(!disposed&&!changing.current)setStatus(result);}catch{if(!disposed)setStatus(undefined);}}
      if(!disposed)timer=setTimeout(()=>void refresh(),2000);
    }
    if(connected)void refresh();else setStatus(undefined);
    return()=>{disposed=true;clearTimeout(timer);};
  },[connected,running]);
  const active=!!status?.active;
  const recovery=!!status?.pendingRecovery&&!active;
  const pending=!running&&preferred&&!active&&!recovery;
  const label=busy?t("正在切换系统代理…"):!connected?t("系统代理：内核未连接"):!status?t("系统代理：状态不可用，请重启或更新内核"):!status.supported?t("此平台暂需手动配置系统代理。"):recovery?t("系统代理：有待恢复的配置，点击尝试恢复"):active?t("系统代理已开启，点击恢复原代理"):running?t("仅监听，点击开启系统代理"):pending?t("下次启动将设置系统代理，点击取消"):t("仅监听，点击设为下次启动时开启系统代理");
  async function toggle(){
    if(changing.current)return;
    if(!running&&!active&&!recovery){onPreference(!preferred);return;}
    changing.current=true;setBusy(true);
    try{
      // Fetch fresh ownership information before modifying OS settings.
      const current=await invoke<Status>('system_proxy_status');
      const enabled=!(current.active||current.pendingRecovery);
      await invoke('set_system_proxy',{enabled});
      onPreference(enabled);
    }catch(e){onError(String(e));}
    finally{
      try{setStatus(await invoke<Status>('system_proxy_status'));}catch{setStatus(undefined);}
      changing.current=false;setBusy(false);
    }
  }
  return <button className={`icon-button system-proxy-toggle ${active?'is-active':pending?'is-pending':recovery?'is-warning':''}`} title={label} aria-label={label} aria-pressed={active} aria-busy={busy} disabled={busy||locked||!connected||!status?.supported} onClick={()=>void toggle()}>
    <Globe2 size={21}/><span className="system-proxy-badge">{active?<Check size={10}/>:pending?<Clock3 size={10}/>:recovery||!status?<CircleAlert size={10}/>:<X size={10}/>}</span>
  </button>;
}
