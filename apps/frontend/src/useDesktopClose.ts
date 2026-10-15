import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { t } from './i18n';
import { createCloseGuard } from './closeGuard';

// Browser beforeunload remains separate; native close must await persistence.
export function useDesktopClose(flush:()=>Promise<boolean>,libraryDirty:()=>boolean,onError:(message:string)=>void,updating?:()=>boolean) {
  const current=useRef({flush,libraryDirty,onError,updating});
  useLayoutEffect(()=>{current.current={flush,libraryDirty,onError,updating};});
  const [closing,setClosing]=useState(false);
  useEffect(()=>{
    if(!isTauri())return;
    let disposed=false;let unlisten:(()=>void)|undefined;
    const window=getCurrentWindow();
    const guard=createCloseGuard({
      blocked:()=>current.current.updating?.()??false,
      dirty:()=>current.current.libraryDirty(),
      flush:()=>current.current.flush(),
      destroy:()=>window.destroy(),
      busy:setClosing,
      error:error=>current.current.onError(error==='update-in-progress'
        ?t("更新正在进行，请等待完成后再关闭。") :error==='dirty-library'
        ?t("集合或环境有未保存的修改，请保存后再关闭。")
        :error==='save-failed'?t("请求保存失败，窗口保持打开，请重试保存后再关闭。") :String(error)),
    });
    void window.onCloseRequested(async event=>{
      event.preventDefault();
      await guard.close();
    }).then(stop=>{if(disposed)stop();else unlisten=stop;}).catch(error=>{if(!disposed)current.current.onError(String(error));});
    return()=>{disposed=true;guard.dispose();unlisten?.();};
  },[]);
  return closing;
}
