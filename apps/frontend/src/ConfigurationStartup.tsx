import {useEffect,useState,type ReactNode} from 'react';
import {invoke} from './api';
import {recoverConfigurationStartup,type ConfigurationImportStatus} from './configImportRecovery';
import {t} from './i18n';

/** Mount the workbench only after committed portable preferences are restored. */
export default function ConfigurationStartup({children,onReady}:{children:(status:ConfigurationImportStatus|undefined)=>ReactNode;onReady?:()=>void}){
  const [attempt,setAttempt]=useState(0),[state,setState]=useState<{ready:boolean;status?:ConfigurationImportStatus;error?:string}>({ready:false});
  useEffect(()=>{
    let live=true;
    setState({ready:false});
    void recoverConfigurationStartup(()=>invoke<ConfigurationImportStatus>('config_status'),undefined,()=>live).then(status=>{
      if(!live)return;
      onReady?.();setState({ready:true,status});
    }).catch(error=>{if(live)setState({ready:false,error:String(error)})});
    return()=>{live=false};
  },[attempt,onReady]);
  if(state.ready)return children(state.status);
  return <div className="config-restart" role={state.error?'alert':'status'}><div><h2>{state.error?t('恢复导入的界面偏好失败'):t('正在恢复配置…')}</h2>{state.error&&<><p>{t('已提交的配置仍保留在本机。保存界面偏好成功后才会打开工作区，避免丢失导入结果。')}</p><p className="error-box">{state.error}</p><button className="button primary" onClick={()=>setAttempt(value=>value+1)}>{t('重试恢复配置')}</button></>}</div></div>;
}
