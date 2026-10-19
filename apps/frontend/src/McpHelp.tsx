/// <reference types="vite/client" />
import {useEffect, useId, useRef, useState} from 'react';
import {createPortal} from 'react-dom';
import {BookOpen, Copy, X} from 'lucide-react';
import {writeClipboard} from './clipboard';
import {t} from './i18n';
import guide from '../../../docs/mcp.md?raw';

export default function McpHelp({onClose}:{onClose:()=>void}) {
  const id=useId(),dialog=useRef<HTMLDialogElement>(null);
  const [copying,setCopying]=useState(false),[message,setMessage]=useState(''),[error,setError]=useState('');
  useEffect(()=>{
    const element=dialog.current!,previousFocus=document.activeElement;
    if(!element.open)element.showModal();
    return()=>{if(element.open)element.close();if(previousFocus instanceof HTMLElement&&previousFocus.isConnected)previousFocus.focus()};
  },[]);
  async function copy(){
    if(copying)return;
    setCopying(true);setMessage('');setError('');
    try{await writeClipboard(guide);setMessage(t('完整说明已复制。'))}
    catch{setError(t('复制失败，可在说明中选中文本后手动复制。'))}
    finally{setCopying(false)}
  }
  return createPortal(<dialog ref={dialog} className="mcp-help-dialog" aria-labelledby={id+'-title'} aria-describedby={id+'-description'} onCancel={event=>{event.preventDefault();onClose()}}>
    <header><div><h2 id={id+'-title'}><BookOpen size={20}/>{t('MCP 使用说明')}</h2><p id={id+'-description'}>{t('从第一次连接到常见问题，完整文档随应用提供，断网也能查看。')}</p></div><button type="button" className="button" aria-label={t('关闭 MCP 使用说明')} onClick={onClose}><X size={18}/></button></header>
    <pre className="mcp-help-document" aria-label={t('MCP 使用说明全文')} tabIndex={0}>{guide}</pre>
    <footer><span role={error?'alert':'status'}>{error||message}</span><button type="button" className="button" disabled={copying} onClick={()=>void copy()}><Copy size={14}/>{t('复制完整说明')}</button><button type="button" className="button primary" autoFocus onClick={onClose}>{t('关闭')}</button></footer>
  </dialog>,document.body);
}
