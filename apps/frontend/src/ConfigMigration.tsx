import {useRef,useState} from 'react';
import {isTauri} from '@tauri-apps/api/core';
import {Download,FileJson,RotateCcw,Upload} from 'lucide-react';
import {invoke} from './api';
import {chooseSave,saveText} from './saveFile';
import {readConfigurationPreferences} from './configPreferences';
import {t} from './i18n';
import './config-migration.css';

export interface ConfigImportResult {epoch?:string;importedSections:string[];preferences:Record<string,string>;requiresRestart:boolean;recoveryAvailable:boolean;warnings?:string[]}
interface Section {id:string;count:number}
interface Snapshot {document:string;summary:{sections:Section[];warnings:string[]}}
interface Preview {expectedRevision:string;sections:Section[];warnings:string[];conflicts?:{id:string;section:string;label:string}[];requiresRestart:boolean;sourceVersion:string;exportedAt:number}
const maxBytes=24*1024*1024;
const plainMaxBytes=16*1024*1024;
function encrypted(value:string){try{return JSON.parse(value)?.format==='http-capture-encrypted-config'}catch{return false}}
function sectionLabel(id:string){return ({preferences:t('界面与监听偏好'),library:t('集合与环境'),workspace:t('请求草稿'),interception:t('断点规则'),blocking:t('阻止规则'),faults:t('模拟规则'),responseRewrite:t('响应重写规则'),scripts:t('全局抓包脚本'),plugins:t('插件'),decryption:t('解密绕过列表'),upstream:t('上游代理'),mcp:t('MCP 配置')})[id]??id}

export default function ConfigMigration({connected,beforeTransfer,onImported}:{connected:boolean;beforeTransfer:()=>Promise<void>;onImported:(result:ConfigImportResult)=>void}){
  const [busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState('');
  const [options,setOptions]=useState({includeRequestData:false,includeCode:false,includeWorkspace:false,includeProxyUsernames:false});
  const [protect,setProtect]=useState(false),[exportPassphrase,setExportPassphrase]=useState(''),[confirmPassphrase,setConfirmPassphrase]=useState(''),[importPassphrase,setImportPassphrase]=useState('');
  const [document,setDocument]=useState(''),[preview,setPreview]=useState<Preview|null>(null),[selected,setSelected]=useState<string[]>([]);
  const [importMode,setImportMode]=useState<'replace'|'merge'>('replace'),[needsPreview,setNeedsPreview]=useState(false);
  const [choices,setChoices]=useState<Record<string,'local'|'imported'>>({});
  const [warnings,setWarnings]=useState<string[]>([]);
  const fileInput=useRef<HTMLInputElement>(null);
  const previewPreferences=useRef('');
  const previewChoices=useRef('');
  async function exportFile(recovery=false){
    if(busy||!connected)return;
    if(protect&&([...(exportPassphrase)].length<12||new TextEncoder().encode(exportPassphrase).length>1024||exportPassphrase!==confirmPassphrase)){setError(t('加密密码至少需要 12 个字符，且两次输入必须一致。'));return}
    setBusy(true);setError('');setNotice('');setWarnings([]);
    try{
      const name=`http-capture-${recovery?'recovery':'config'}-${new Date().toISOString().slice(0,10)}.${protect?'hccfg':'json'}`;
      const handle=await chooseSave(name);if(handle===null)return;
      await beforeTransfer();
      const passphrase=protect?exportPassphrase:undefined;
      const snapshot=await invoke<Snapshot>(recovery?'config_recovery':'config_export',recovery?{...(passphrase?{passphrase}:{})}:{preferences:readConfigurationPreferences(),options,...(passphrase?{passphrase}:{})});
      setWarnings(snapshot.summary.warnings);
      if(isTauri()){
        const saved=await invoke<string|null>('config_save_file',{content:snapshot.document,filename:name});
        setNotice(saved?t('已保存：')+saved:t('已取消保存'));
      }else setNotice(await saveText(snapshot.document,name,'json',handle));
    }catch(e){setError(String(e))}finally{setBusy(false);setExportPassphrase('');setConfirmPassphrase('')}
  }
  function changeDocument(value:string){setDocument(value);setImportPassphrase('');setPreview(null);setSelected([]);setChoices({});setNeedsPreview(false);setError('');setNotice('')}
  async function readFile(file:File|undefined){
    if(!file||busy)return;
    setBusy(true);setError('');setPreview(null);setSelected([]);
    try{if(file.size>maxBytes)throw Error(t('配置文件不能超过 24 MiB。'));const value=await file.text();if(!encrypted(value)&&new TextEncoder().encode(value).length>plainMaxBytes)throw Error(t('配置文件不能超过 16 MiB。'));changeDocument(value)}
    catch(e){setError(String(e))}finally{setBusy(false);if(fileInput.current)fileInput.current.value=''}
  }
  async function inspect(){
    if(busy||!document.trim()||!connected)return;
    const requested=importMode==='merge'&&preview?selected:undefined;
    const requestedChoices=importMode==='merge'?Object.fromEntries(Object.entries(choices).filter(([id])=>{
      const conflict=preview?.conflicts?.find(item=>item.id===id);
      return conflict&&(!requested||requested.includes(conflict.section));
    })):{};
    if(requested&&requested.length===0){setError(t('请至少选择一个合并类别。'));return}
    setBusy(true);setError('');setNotice('');setPreview(null);
    try{
      if(new TextEncoder().encode(document).length>(encrypted(document)?maxBytes:plainMaxBytes))throw Error(t(encrypted(document)?'配置文件不能超过 24 MiB。':'配置文件不能超过 16 MiB。'));
      const currentPreferences=readConfigurationPreferences();
      const next=await invoke<Preview>('config_preview',{document,...(encrypted(document)?{passphrase:importPassphrase}:{}),...(importMode==='merge'?{mode:'merge',preferences:currentPreferences,...(requested?{sections:requested}:{}),...(Object.keys(requestedChoices).length?{choices:requestedChoices}:{})}:{})});setPreview(next);setSelected(requested??next.sections.map(section=>section.id));setChoices(requestedChoices);setNeedsPreview(false);previewPreferences.current=JSON.stringify(currentPreferences);previewChoices.current=JSON.stringify(requestedChoices);
    }catch(e){setError(String(e));setChoices({})}finally{setBusy(false)}
  }
  async function importFile(){
    if(busy||!preview||needsPreview||!selected.length||!connected)return;
    const currentPreferences=readConfigurationPreferences();
    if(JSON.stringify(currentPreferences)!==previewPreferences.current){setError(t('本机偏好在预览后已更改，请重新预览。'));setNeedsPreview(true);return}
    if(importMode==='merge'&&JSON.stringify(choices)!==previewChoices.current){setError(t('冲突处理方式已更改，请重新预览。'));setNeedsPreview(true);return}
    setBusy(true);setError('');setNotice('');
    try{
      await beforeTransfer();
      const result=await invoke<ConfigImportResult>('config_import',{document,sections:selected,expectedRevision:preview.expectedRevision,preferences:currentPreferences,...(encrypted(document)?{passphrase:importPassphrase}:{}),...(importMode==='merge'?{mode:'merge',...(Object.keys(choices).length?{choices}:{})}:{})});
      setImportPassphrase('');
      onImported(result);
    }catch(e){setError(String(e));setPreview(null);setSelected([])}finally{setBusy(false)}
  }
  return <div className="config-migration">
    <header><h3><FileJson size={19}/>{t('全局配置迁移')}</h3><p>{t('换设备时导出配置文件，在新设备预览后导入。抓包记录、证书私钥、本机 Token 和代理密码不包含在内。')}</p></header>
    <section className="settings-section"><h4>{t('导出配置')}</h4><p>{t('默认保留配置结构，清空请求内容和变量值；勾选后可能包含 Cookie、认证信息和业务数据。可以用密码加密导出文件。')}</p>
      <fieldset disabled={busy||!connected} className="config-options">
        <label><input type="checkbox" checked={options.includeRequestData} onChange={e=>setOptions({...options,includeRequestData:e.target.checked})}/>{t('包含请求内容和变量值')}</label>
        <label><input type="checkbox" checked={options.includeCode} onChange={e=>setOptions({...options,includeCode:e.target.checked})}/>{t('包含脚本与插件代码')}</label>
        <label><input type="checkbox" checked={options.includeWorkspace} onChange={e=>setOptions({...options,includeWorkspace:e.target.checked})}/>{t('包含请求工作区草稿')}</label>
        <label><input type="checkbox" checked={options.includeProxyUsernames} onChange={e=>setOptions({...options,includeProxyUsernames:e.target.checked})}/>{t('包含代理用户名')}</label>
      </fieldset><label className="config-protect"><input type="checkbox" checked={protect} disabled={busy} onChange={e=>{setProtect(e.target.checked);setExportPassphrase('');setConfirmPassphrase('')}}/>{t('用密码加密导出文件')}</label>
      {protect&&<div className="config-passphrases"><label>{t('加密密码')}<input type="password" autoComplete="new-password" value={exportPassphrase} disabled={busy} onChange={e=>setExportPassphrase(e.target.value)}/></label><label>{t('确认密码')}<input type="password" autoComplete="new-password" value={confirmPassphrase} disabled={busy} onChange={e=>setConfirmPassphrase(e.target.value)}/></label><small>{t('密码不会保存；遗失后无法解密文件。')}</small></div>}
      <button className="button primary" disabled={busy||!connected} onClick={()=>void exportFile()}><Download size={15}/>{t('导出全局配置')}</button>
    </section>
    <section className="settings-section"><h4>{t('导入配置')}</h4><p>{t('先预览，再选择要替换的类别。导入会停止网络功能并保留本机恢复点；完成后需要重启，不会自动启用代理、脚本或 MCP。')}</p>
      <div className="config-import-mode"><label><input type="radio" name="config-import-mode" checked={importMode==='replace'} disabled={busy} onChange={()=>{setImportMode('replace');setPreview(null);setSelected([]);setChoices({});setNeedsPreview(false)}}/>{t('替换所选类别')}</label><label><input type="radio" name="config-import-mode" checked={importMode==='merge'} disabled={busy} onChange={()=>{setImportMode('merge');setPreview(null);setSelected([]);setChoices({});setNeedsPreview(false)}}/>{t('合并到当前配置')}</label></div>
      {importMode==='merge'&&<p>{t('所有配置类别均可合并；导入的规则和插件默认禁用。全局脚本按空位和变量合并，新增内容后关闭；集合与环境、脚本、界面偏好、代理、请求标签、MCP 服务、插件和规则冲突可逐项选择。')}</p>}
      <input ref={fileInput} type="file" accept=".json,.hccfg,application/json" hidden disabled={busy} onChange={e=>void readFile(e.target.files?.[0])}/>
      <div className="config-actions"><button className="button" disabled={busy} onClick={()=>fileInput.current?.click()}><Upload size={15}/>{t('选择配置文件')}</button><button className="button" disabled={busy||!connected} onClick={()=>void exportFile(true)}><RotateCcw size={15}/>{t('导出上次导入前恢复点')}</button></div>
      <p>{t('恢复点包含原有请求内容、变量及代码，可能含业务凭据；不会按普通导出选项清空，请妥善保管。')}</p>
      <textarea aria-label={t('全局配置 JSON')} placeholder={t('选择配置文件，或在这里粘贴配置内容')} value={document} disabled={busy} onChange={e=>changeDocument(e.target.value)} spellCheck={false}/>
      {encrypted(document)&&<label className="config-import-passphrase">{t('配置文件密码')}<input type="password" autoComplete="off" value={importPassphrase} disabled={busy} onChange={e=>{setImportPassphrase(e.target.value);setPreview(null);setSelected([]);setChoices({})}}/></label>}
      <button className="button" disabled={busy||!connected||!document.trim()} onClick={()=>void inspect()}>{t('预览导入内容')}</button>
      {preview&&<div className="config-preview"><p>{t('来源版本')} <code>{preview.sourceVersion}</code> · {t('导出时间')} {new Date(preview.exportedAt).toLocaleString()}</p><fieldset disabled={busy}><legend>{importMode==='merge'?t('合并所选类别；MCP 服务和权限仍会重置'):t('替换所选类别，未选类别仅调整关联；MCP 服务和权限始终重置')}</legend>{preview.sections.map(section=><label key={section.id}><input type="checkbox" checked={selected.includes(section.id)} onChange={e=>{setSelected(values=>e.target.checked?[...values,section.id]:values.filter(value=>value!==section.id));if(importMode==='merge')setNeedsPreview(true)}}/><span>{sectionLabel(section.id)}</span><small>{section.count}</small></label>)}</fieldset>{importMode==='merge'&&(preview.conflicts??[]).some(conflict=>selected.includes(conflict.section))&&<fieldset className="config-conflicts" disabled={busy}><legend>{t('逐项处理冲突')}</legend>{preview.conflicts!.filter(conflict=>selected.includes(conflict.section)).map(conflict=><label key={conflict.id}><span>{conflict.label}</span><select value={choices[conflict.id]??'local'} onChange={e=>{setChoices(value=>({...value,[conflict.id]:e.target.value as 'local'|'imported'}));setNeedsPreview(true)}}><option value="local">{t('保留本机')}</option><option value="imported">{t('使用导入')}</option></select></label>)}</fieldset>}{preview.warnings.length>0&&<ul>{preview.warnings.map((warning,i)=><li key={i}>{warning}</li>)}</ul>}{needsPreview&&<p role="status">{t('合并类别或冲突选择已更改，请重新预览后再导入。')}</p>}<button className="button primary" disabled={busy||needsPreview||!selected.length} onClick={()=>void importFile()}>{t('导入所选配置并停止网络')}</button></div>}
    </section>
    {warnings.length>0&&<ul className="config-warnings">{warnings.map((warning,i)=><li key={i}>{warning}</li>)}</ul>}
    {busy&&<p role="status">{t('正在处理配置…')}</p>}{error&&<p className="error-box" role="alert">{error}</p>}{notice&&<p role="status">{notice}</p>}
  </div>;
}
