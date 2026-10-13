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
  const blob=new Blob([content],{type:format==='pem'?'application/x-pem-file':'application/json'});
  if(handle){const writer=await handle.createWritable();try{await writer.write(blob);await writer.close();}catch(e){await writer.abort().catch(()=>{});throw e;}return t('已保存：')+handle.name;}
  const url=URL.createObjectURL(blob);const link=document.createElement('a');link.href=url;link.download=name;link.click();setTimeout(()=>URL.revokeObjectURL(url),30000);
  return t('已提交浏览器下载，请在浏览器下载列表查看保存位置');
}
