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
interface Preview {expectedRevision:string;sections:Section[];warnings:string[];requiresRestart:boolean;sourceVersion:string;exportedAt:number}
const maxBytes=16*1024*1024;
function sectionLabel(id:string){return ({preferences:t('界面与监听偏好'),library:t('集合与环境'),workspace:t('请求草稿'),interception:t('断点规则'),blocking:t('阻止规则'),scripts:t('全局抓包脚本'),plugins:t('插件'),decryption:t('解密绕过列表'),upstream:t('上游代理'),mcp:t('MCP 配置')})[id]??id}

export default function ConfigMigration({connected,beforeTransfer,onImported}:{connected:boolean;beforeTransfer:()=>Promise<void>;onImported:(result:ConfigImportResult)=>void}){
  const [busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState('');
  const [options,setOptions]=useState({includeRequestData:false,includeCode:false,includeWorkspace:false,includeProxyUsernames:false});
  const [document,setDocument]=useState(''),[preview,setPreview]=useState<Preview|null>(null),[selected,setSelected]=useState<string[]>([]);
  const [warnings,setWarnings]=useState<string[]>([]);
  const fileInput=useRef<HTMLInputElement>(null);
  async function exportFile(recovery=false){
    if(busy||!connected)return;
    setBusy(true);setError('');setNotice('');setWarnings([]);
    try{
      const name=`http-capture-${recovery?'recovery':'config'}-${new Date().toISOString().slice(0,10)}.json`;
      const handle=await chooseSave(name);if(handle===null)return;
      await beforeTransfer();
      const snapshot=await invoke<Snapshot>(recovery?'config_recovery':'config_export',recovery?{}:{preferences:readConfigurationPreferences(),options});
      setWarnings(snapshot.summary.warnings);
      if(isTauri()){
        const saved=await invoke<string|null>('config_save_file',{content:snapshot.document,filename:name});
        setNotice(saved?t('已保存：')+saved:t('已取消保存'));
      }else setNotice(await saveText(snapshot.document,name,'json',handle));
    }catch(e){setError(String(e))}finally{setBusy(false)}
  }
  function changeDocument(value:string){setDocument(value);setPreview(null);setSelected([]);setError('');setNotice('')}
  async function readFile(file:File|undefined){
    if(!file||busy)return;
    setBusy(true);setError('');setPreview(null);setSelected([]);
    try{if(file.size>maxBytes)throw Error(t('配置文件不能超过 16 MiB。'));changeDocument(await file.text())}
    catch(e){setError(String(e))}finally{setBusy(false);if(fileInput.current)fileInput.current.value=''}
  }
  async function inspect(){
    if(busy||!document.trim()||!connected)return;
    setBusy(true);setError('');setNotice('');setPreview(null);setSelected([]);
    try{
      if(new TextEncoder().encode(document).length>maxBytes)throw Error(t('配置文件不能超过 16 MiB。'));
      const next=await invoke<Preview>('config_preview',{document});setPreview(next);setSelected(next.sections.map(section=>section.id));
    }catch(e){setError(String(e))}finally{setBusy(false)}
  }
  async function importFile(){
    if(busy||!preview||!selected.length||!connected)return;
    setBusy(true);setError('');setNotice('');
    try{
      await beforeTransfer();
      const result=await invoke<ConfigImportResult>('config_import',{document,sections:selected,expectedRevision:preview.expectedRevision,preferences:readConfigurationPreferences()});
      onImported(result);
    }catch(e){setError(String(e));setPreview(null);setSelected([])}finally{setBusy(false)}
  }
  return <div className="config-migration">
    <header><h3><FileJson size={19}/>{t('全局配置迁移')}</h3><p>{t('换设备时导出配置文件，在新设备预览后导入。抓包记录、证书私钥、本机 Token 和代理密码不包含在内。')}</p></header>
    <section className="settings-section"><h4>{t('导出配置')}</h4><p>{t('标准 JSON 不加密。默认保留配置结构，清空请求内容和变量值；勾选后可能包含 Cookie、认证信息和业务数据，请妥善保存文件。')}</p>
      <fieldset disabled={busy||!connected} className="config-options">
        <label><input type="checkbox" checked={options.includeRequestData} onChange={e=>setOptions({...options,includeRequestData:e.target.checked})}/>{t('包含请求内容和变量值')}</label>
        <label><input type="checkbox" checked={options.includeCode} onChange={e=>setOptions({...options,includeCode:e.target.checked})}/>{t('包含脚本与插件代码')}</label>
        <label><input type="checkbox" checked={options.includeWorkspace} onChange={e=>setOptions({...options,includeWorkspace:e.target.checked})}/>{t('包含请求工作区草稿')}</label>
        <label><input type="checkbox" checked={options.includeProxyUsernames} onChange={e=>setOptions({...options,includeProxyUsernames:e.target.checked})}/>{t('包含代理用户名')}</label>
      </fieldset><button className="button primary" disabled={busy||!connected} onClick={()=>void exportFile()}><Download size={15}/>{t('导出全局配置')}</button>
    </section>
    <section className="settings-section"><h4>{t('导入配置')}</h4><p>{t('先预览，再选择要替换的类别。导入会停止网络功能并保留本机恢复点；完成后需要重启，不会自动启用代理、脚本或 MCP。')}</p>
      <input ref={fileInput} type="file" accept=".json,application/json" hidden disabled={busy} onChange={e=>void readFile(e.target.files?.[0])}/>
      <div className="config-actions"><button className="button" disabled={busy} onClick={()=>fileInput.current?.click()}><Upload size={15}/>{t('选择配置文件')}</button><button className="button" disabled={busy||!connected} onClick={()=>void exportFile(true)}><RotateCcw size={15}/>{t('导出上次导入前恢复点')}</button></div>
      <p>{t('恢复点包含原有请求内容、变量及代码，可能含业务凭据；不会按普通导出选项清空，请妥善保管。')}</p>
      <textarea aria-label={t('全局配置 JSON')} placeholder={t('选择 JSON 文件，或在这里粘贴配置内容')} value={document} disabled={busy} onChange={e=>changeDocument(e.target.value)} spellCheck={false}/>
      <button className="button" disabled={busy||!connected||!document.trim()} onClick={()=>void inspect()}>{t('预览导入内容')}</button>
      {preview&&<div className="config-preview"><p>{t('来源版本')} <code>{preview.sourceVersion}</code> · {t('导出时间')} {new Date(preview.exportedAt).toLocaleString()}</p><fieldset disabled={busy}><legend>{t('替换所选类别，未选类别仅调整关联；MCP 服务和权限始终重置')}</legend>{preview.sections.map(section=><label key={section.id}><input type="checkbox" checked={selected.includes(section.id)} onChange={e=>setSelected(values=>e.target.checked?[...values,section.id]:values.filter(value=>value!==section.id))}/><span>{sectionLabel(section.id)}</span><small>{section.count}</small></label>)}</fieldset>{preview.warnings.length>0&&<ul>{preview.warnings.map((warning,i)=><li key={i}>{warning}</li>)}</ul>}<button className="button primary" disabled={busy||!selected.length} onClick={()=>void importFile()}>{t('导入所选配置并停止网络')}</button></div>}
    </section>
    {warnings.length>0&&<ul className="config-warnings">{warnings.map((warning,i)=><li key={i}>{warning}</li>)}</ul>}
    {busy&&<p role="status">{t('正在处理配置…')}</p>}{error&&<p className="error-box" role="alert">{error}</p>}{notice&&<p role="status">{notice}</p>}
  </div>;
}
