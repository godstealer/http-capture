import {isTauri,invoke} from '@tauri-apps/api/core';
import {t} from './i18n';
interface Handle { name:string; createWritable():Promise<{write(data:Blob):Promise<void>;close():Promise<void>;abort():Promise<void>}> }
export async function chooseSave(name:string):Promise<Handle|null|undefined> {
  if(isTauri()) return undefined;
  const picker=(window as unknown as {showSaveFilePicker?:(options:{suggestedName:string})=>Promise<Handle>}).showSaveFilePicker;
  if(!picker)return undefined;
  try{return await picker.call(window,{suggestedName:name});}catch(e){if((e as DOMException).name==='AbortError')return null;throw e;}
}
export async function saveText(content:string,name:string,format:string,handle:Handle|null|undefined):Promise<string>{
  if(handle===null)return t('已取消保存');
  if(isTauri()) {const path=await invoke<string|null>('save_session_file',{content,format});return path?t('已保存：')+path:t('已取消保存');}
  const blob=new Blob([content],{type:format==='pem'?'application/x-pem-file':['json','har'].includes(format)?'application/json':'text/plain;charset=utf-8'});
  if(handle){const writer=await handle.createWritable();try{await writer.write(blob);await writer.close();}catch(e){await writer.abort().catch(()=>{});throw e;}return t('已保存：')+handle.name;}
  const url=URL.createObjectURL(blob);const link=document.createElement('a');link.href=url;link.download=name;link.click();setTimeout(()=>URL.revokeObjectURL(url),30000);
  return t('已提交浏览器下载，请在浏览器下载列表查看保存位置');
}

// Browsers without a save picker still require a Blob for a download URL.
// Keep serialized records as separate parts so the caller can release each
// complete Flow before loading the next one.
export function downloadTextParts(parts:string[],name:string,format:string):string {
  const blob=new Blob(parts,{type:['json','har'].includes(format)?'application/json':'text/plain;charset=utf-8'});
  const url=URL.createObjectURL(blob);
  try {const link=document.createElement('a');link.href=url;link.download=name;link.click();}
  finally {setTimeout(()=>URL.revokeObjectURL(url),30000);}
  return t('已提交浏览器下载，请在浏览器下载列表查看保存位置');
}

export async function saveCodeFile(content:string,name:string):Promise<string> {
  if(isTauri()) {
    const path=await invoke<string|null>('save_code_file',{content,filename:name});
    return path?t('已保存：')+path:t('已取消保存');
  }
  return saveText(content,name,'txt',await chooseSave(name));
}
