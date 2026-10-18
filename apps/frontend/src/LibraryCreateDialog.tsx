import {useEffect, useId, useRef, useState, type FormEvent} from 'react';
import {createPortal} from 'react-dom';
import {t} from './i18n';
import './save-request.css';

export default function LibraryCreateDialog({title, label, initialName='', busy, onCreate, onClose}: {
  title:string; label:string; initialName?:string; busy:boolean; onCreate:(name:string)=>void; onClose:()=>void;
}) {
  const id=useId(),dialogRef=useRef<HTMLDialogElement>(null),inputRef=useRef<HTMLInputElement>(null);
  const [name,setName]=useState(initialName),[error,setError]=useState('');
  useEffect(()=>{
    const dialog=dialogRef.current!,previousFocus=document.activeElement;
    if(!dialog.open)dialog.showModal();inputRef.current?.focus();inputRef.current?.select();
    return()=>{if(dialog.open)dialog.close();if(previousFocus instanceof HTMLElement&&previousFocus.isConnected)previousFocus.focus();};
  },[]);
  function submit(event:FormEvent){
    event.preventDefault();if(busy)return;
    if(!name.trim()){setError(t('请输入名称'));inputRef.current?.focus();return;}
    onCreate(name.trim());
  }
  return createPortal(<dialog ref={dialogRef} className="save-request-dialog" aria-labelledby={id+'-title'} aria-describedby={id+'-description'} onCancel={event=>{event.preventDefault();if(!busy)onClose();}}>
    <form onSubmit={submit} noValidate aria-busy={busy}>
      <header className="save-request-heading"><h2 id={id+'-title'}>{title}</h2><p id={id+'-description'}>{t('新增到当前编辑内容，点击「保存集合与环境」后写入磁盘。')}</p></header>
      <div className="save-request-fields"><label htmlFor={id+'-name'}>{label}</label><input ref={inputRef} id={id+'-name'} value={name} maxLength={128} required autoFocus autoComplete="off" disabled={busy} aria-invalid={!!error||undefined} aria-describedby={error?id+'-error':undefined} onChange={event=>{setName(event.target.value);setError('');}}/></div>
      {error&&<p className="save-request-error" id={id+'-error'} role="alert">{error}</p>}
      <footer className="save-request-actions"><button type="button" disabled={busy} onClick={onClose}>{t('取消')}</button><button type="submit" disabled={busy} className="save-request-confirm">{t('新增')}</button></footer>
    </form>
  </dialog>,document.body);
}
