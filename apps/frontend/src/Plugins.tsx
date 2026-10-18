/// <reference types="vite/client" />
import {lazy, Suspense, useEffect, useId, useRef, useState} from 'react';
import {createPortal} from 'react-dom';
import {PackageOpen} from 'lucide-react';
import {invoke} from './api';
import {writeClipboard} from './clipboard';
import {t} from './i18n';
import guide from '../../../docs/plugins.md?raw';
import requestIdExample from '../../../examples/plugins/local-request-id.json';
import jsonResponseExample from '../../../examples/plugins/example-json-response.json';
import noCacheExample from '../../../examples/plugins/local-no-cache.json';
import './plugin-help.css';

const BodyCodeEditor = lazy(()=>import('./BodyCodeEditor'));
interface Plugin {id:string;name:string;version:string;apiVersion:number;enabled:boolean;scope:string;urlPattern:string;before:string;after:string}
type Editing = {plugin:Plugin;originalId?:string};
function PluginHelpDialog({onClose}:{onClose:()=>void}) {
  const id=useId(), dialogRef=useRef<HTMLDialogElement>(null);
  const [copying,setCopying]=useState(false),[message,setMessage]=useState(''),[error,setError]=useState('');
  useEffect(()=>{
    const dialog=dialogRef.current!,previousFocus=document.activeElement;
    if(!dialog.open)dialog.showModal();
    return()=>{if(dialog.open)dialog.close();if(previousFocus instanceof HTMLElement&&previousFocus.isConnected)previousFocus.focus();};
  },[]);
  async function copy(){
    setCopying(true);setMessage('');setError('');
    try{await writeClipboard(guide);setMessage(t('完整说明已复制。'));}
    catch{setError(t('复制失败，可在说明中选中文本后手动复制。'));}
    finally{setCopying(false);}
  }
  return createPortal(<dialog ref={dialogRef} className="plugin-help-dialog" aria-labelledby={id+'-title'} aria-describedby={id+'-description'} onCancel={event=>{event.preventDefault();onClose();}}>
    <header><h2 id={id+'-title'}>{t('插件完整说明')}</h2><p id={id+'-description'}>{t('随当前应用打包的 Markdown 说明，可选中文本或复制全文。')}</p></header>
    <pre className="plugin-help-document" tabIndex={0} aria-label={t('插件使用说明全文')}>{guide}</pre>
    <footer><span role="status">{message}</span>{error&&<span role="alert">{error}</span>}
      <button type="button" disabled={copying} onClick={()=>void copy()}>{t('复制完整说明')}</button>
      <button type="button" autoFocus onClick={onClose}>{t('关闭')}</button>
    </footer>
  </dialog>,document.body);
}
export default function Plugins(){
  const [items,setItems]=useState<Plugin[]>([]),[logs,setLogs]=useState<string[]>([]);
  const [candidate,setCandidate]=useState<Plugin|null>(null),[editing,setEditing]=useState<Editing|null>(null);
  const [error,setError]=useState(''),[message,setMessage]=useState(''),[busy,setBusy]=useState(false),[ready,setReady]=useState(false),[helpOpen,setHelpOpen]=useState(false);
  const [search,setSearch]=useState(''),[status,setStatus]=useState('all'),[scope,setScope]=useState('all'),[detailId,setDetailId]=useState<string|null>(null);
  const lock=useRef(false),editorRef=useRef<HTMLElement>(null),editorId=useId();
  const scopeName=(value:string)=>value==='capture'?t('抓包请求'):value==='replay'?t('手动请求'):t('抓包与手动请求');
  const query=search.trim().toLowerCase();
  const visible=items.filter(plugin=>(!query||[plugin.id,plugin.name,plugin.version,plugin.urlPattern].some(value=>value.toLowerCase().includes(query)))&&(status==='all'||plugin.enabled===(status==='enabled'))&&(scope==='all'||plugin.scope===(scope==='both'?'all':scope)));
  const detail=detailId?items.find(plugin=>plugin.id===detailId):undefined;
  function begin(){if(lock.current)return false;lock.current=true;setBusy(true);setError('');setMessage('');return true;}
  function end(){lock.current=false;setBusy(false);}
  async function refresh(){
    if(!begin())return;
    try{const [list,output]=await Promise.all([invoke<Plugin[]>('plugins_list'),invoke<string[]>('plugin_logs')]);setItems(list);setLogs(output);setReady(true);}
    catch(e){setError(String(e));}finally{end();}
  }
  useEffect(()=>{void refresh();},[]);
  useEffect(()=>{if(editing)editorRef.current?.scrollIntoView({block:'nearest'});},[editing?.originalId,!!editing]);
  async function persist(next:Plugin[]){
    await invoke('plugins_save',{plugins:next});setItems(next);setCandidate(null);setEditing(null);setDetailId(null);
  }
  async function save(next:Plugin[]){
    if(!begin())return;
    try{await persist(next);}catch(e){setError(String(e));}finally{end();}
  }
  function chooseExample(plugin:Plugin){if(lock.current)return;setCandidate({...plugin,enabled:false});setEditing(null);setDetailId(null);setError('');setMessage('');}
  function edit(plugin?:Plugin,originalId?:string){
    if(lock.current)return;
    setCandidate(null);setDetailId(null);setError('');setMessage('');
    setEditing({originalId,plugin:plugin?{...plugin,enabled:false}:{
      id:'my-plugin',name:t('我的插件'),version:'1.0.0',apiVersion:1,enabled:false,scope:'replay',
      urlPattern:'^http://(?:127\\.0\\.0\\.1|localhost)(?::[0-9]+)?/',before:"request.headers.push({name: 'X-Debug', value: 'hello'});",after:''
    }});
  }
  function change(patch:Partial<Plugin>){if(lock.current)return;setEditing(value=>value?{...value,plugin:{...value.plugin,...patch}}:value);setMessage('');setError('');}
  async function check(saveAfter:boolean){
    if(!editing||!begin())return;
    const snapshot={...editing.plugin,enabled:false};
    try{
      if(items.some(plugin=>plugin.id===snapshot.id&&plugin.id!==editing.originalId))throw Error(t('插件 ID 已存在，请先卸载旧版本。'));
      await invoke('plugins_validate',{plugin:snapshot});
      if(saveAfter){
        const next=editing.originalId?items.map(plugin=>plugin.id===editing.originalId?snapshot:plugin):[...items,snapshot];
        await persist(next);setMessage(t('插件已保存并保持禁用；确认后勾选启用。'));
      }else{setMessage(t('清单和 JavaScript 语法检查通过；代码尚未执行。'));}
    }catch(e){setError(String(e));}finally{end();}
  }
  return <section className="request-details plugins-workspace">
    <header className="plugins-heading"><h1>{t('插件管理')}</h1><p>{t('本地 JavaScript 插件；安装后默认禁用。启用后可读取并修改匹配的请求和响应，不提供文件、网络或系统命令权限。')}</p></header>
    <details className="plugin-help">
      <summary>{t('使用说明')}</summary>
      <div className="plugin-help-content">
        <ol className="plugin-help-steps">
          <li><strong>{t('选择或编写插件')}</strong><p>{t('导入本地 JSON、选择示例，或点击「编写插件」直接编辑。')}</p></li>
          <li><strong>{t('保存并启用')}</strong><p>{t('确认匹配地址和代码，安装或保存后再勾选「启用插件」；新代码默认禁用。')}</p></li>
          <li><strong>{t('发送请求并查看日志')}</strong><p>{t('向匹配的本机测试接口发送普通 HTTP 请求，再点击「刷新」查看插件日志。')}</p></li>
        </ol>
        <h2>{t('从示例开始')}</h2><p>{t('示例只进入安装预览，不会自动启用、发送请求或启动服务器。')}</p>
        <div className="plugin-help-examples">{[
          {plugin:requestIdExample,title:t('本机请求标识'),description:t('仅手动请求：给本机 HTTP 接口添加随机调试标识。')},
          {plugin:jsonResponseExample,title:t('JSON 响应修改'),description:t('仅手动请求：修改 example.invalid/api/ 的 JSON；使用前请改为自己的本机地址。')},
          {plugin:noCacheExample,title:t('本机缓存调试'),description:t('抓包和手动请求：调整本机 /demo/ 接口的请求及响应缓存头。')},
        ].map(example=><article key={example.plugin.id}><h3>{example.title}</h3><p>{example.description}</p><button type="button" className="button" disabled={busy||!ready} onClick={()=>chooseExample(example.plugin)}>{t('预览此示例')}</button></article>)}</div>
        <h2>{t('常见排错')}</h2><ul className="plugin-help-troubleshooting">
          <li>{t('没有日志：检查是否启用、来源范围与原始 URL 是否匹配，然后点击「刷新」。')}</li>
          <li>{t('修改没有生效：查看错误日志；失败会回滚本次钩子，后续插件、脚本和断点也可能覆盖修改。')}</li>
          <li>{t('JSON 解析失败：正文可能已压缩、为空或不是 UTF-8 JSON；插件不会自动解压。')}</li>
          <li>{t('SSE 接口应排除响应后钩子；跨阶段变量、网络请求和文件操作暂不支持。')}</li>
        </ul><button type="button" className="button" onClick={()=>setHelpOpen(true)}>{t('完整说明')}</button>
      </div>
    </details>
    <div className="plugins-toolbar">
      <button className="button" disabled={busy||!ready} onClick={()=>edit()}>{t('编写插件')}</button>
      <input aria-label={t('导入插件')} type="file" accept=".json" disabled={busy||!ready} onChange={async event=>{
        const file=event.target.files?.[0];event.target.value='';if(!file||!begin())return;
        try{if(file.size>150000)throw Error('Plugin package exceeds 150 KB');const plugin=JSON.parse(await file.text());if(!plugin||['id','name','version','scope','urlPattern','before','after'].some(key=>typeof plugin[key]!=='string')||!Number.isInteger(plugin.apiVersion))throw Error('Invalid plugin manifest');setCandidate({...plugin,enabled:false});setEditing(null);setDetailId(null);}
        catch(e){setError(String(e));}finally{end();}
      }}/>
      <button className="button" disabled={busy} onClick={()=>void refresh()}>{t('刷新')}</button>
    </div>
    {error&&<p role="alert">{error}</p>}{message&&<p className="plugin-help-status" role="status">{message}</p>}
    {editing&&<article className="plugin-editor" ref={editorRef}>
      <h2>{editing.originalId?t('编辑插件'):t('编写插件')}</h2>
      <p>{t('在应用内检查并保存，无需本地 JSON 文件。语法检查不会执行代码；保存后保持禁用，原插件只在保存成功后替换。')}</p>
      <fieldset disabled={busy} className="plugin-editor-fields">
        <label htmlFor={editorId+'-id'}>{t('插件 ID')}<input id={editorId+'-id'} value={editing.plugin.id} readOnly={!!editing.originalId} maxLength={80} onChange={event=>change({id:event.target.value})}/></label>
        <label htmlFor={editorId+'-name'}>{t('插件名称')}<input id={editorId+'-name'} value={editing.plugin.name} onChange={event=>change({name:event.target.value})}/></label>
        <label htmlFor={editorId+'-version'}>{t('版本')}<input id={editorId+'-version'} value={editing.plugin.version} onChange={event=>change({version:event.target.value})}/></label>
        <label htmlFor={editorId+'-scope'}>{t('生效范围')}<select id={editorId+'-scope'} value={editing.plugin.scope} onChange={event=>change({scope:event.target.value})}><option value="replay">{t('手动请求')}</option><option value="capture">{t('抓包请求')}</option><option value="all">{t('抓包与手动请求')}</option></select></label>
        <label className="plugin-editor-pattern" htmlFor={editorId+'-pattern'}>{t('URL 正则表达式')}<input id={editorId+'-pattern'} value={editing.plugin.urlPattern} onChange={event=>change({urlPattern:event.target.value})}/></label>
      </fieldset>
      <div className="plugin-editor-code">{(['before','after'] as const).map(stage=><section key={stage}><h3>{stage==='before'?t('请求前 JavaScript'):t('响应后 JavaScript')}</h3><Suspense fallback={<pre>{editing.plugin[stage]}</pre>}><BodyCodeEditor parser="babel" label={stage==='before'?t('插件请求前代码'):t('插件响应后代码')} value={editing.plugin[stage]} disabled={busy} formatting onChange={text=>change({[stage]:text})}/></Suspense></section>)}</div>
      <div className="plugin-editor-actions"><button className="button" disabled={busy} onClick={()=>void check(false)}>{t('检查语法')}</button><button className="button" disabled={busy} onClick={()=>void check(true)}>{t('检查并保存插件')}</button><button className="button" disabled={busy} onClick={()=>setEditing(null)}>{t('取消')}</button></div>
    </article>}
    {ready&&!items.length&&!candidate&&!editing&&<div className="secondary-empty plugins-empty"><PackageOpen size={28} aria-hidden="true"/><strong>{t('尚未安装插件')}</strong><p>{t('导入本地插件包后，可在这里管理启用状态与执行顺序。')}</p></div>}
    {candidate&&<article className="setting-card"><div><h3>{candidate.name} · {candidate.version}</h3><p>{candidate.id} · API {candidate.apiVersion} · {candidate.scope} · {candidate.urlPattern}</p><details><summary>{t('查看插件代码')}</summary><pre>{candidate.before+'\n\n'+candidate.after}</pre></details>
      <button className="button" disabled={busy||items.some(plugin=>plugin.id===candidate.id)} onClick={()=>void save([...items,candidate])}>{t('安装插件')}</button><button className="button" disabled={busy} onClick={()=>edit(candidate)}>{t('编辑后安装')}</button><button className="button" disabled={busy} onClick={()=>setCandidate(null)}>{t('取消')}</button>{items.some(plugin=>plugin.id===candidate.id)&&<p>{t('插件 ID 已存在，请先卸载旧版本。')}</p>}
    </div></article>}
    {!!items.length&&<section className="plugin-list-panel" aria-label={t('已安装插件')}>
      <div className="plugin-list-filters">
        <input type="search" aria-label={t('搜索插件')} placeholder={t('搜索名称、ID 或 URL')} value={search} onChange={event=>setSearch(event.target.value)}/>
        <select aria-label={t('按启用状态筛选')} value={status} onChange={event=>setStatus(event.target.value)}><option value="all">{t('全部状态')}</option><option value="enabled">{t('已启用')}</option><option value="disabled">{t('已禁用')}</option></select>
        <select aria-label={t('按生效范围筛选')} value={scope} onChange={event=>setScope(event.target.value)}><option value="all">{t('全部范围')}</option><option value="capture">{t('抓包请求')}</option><option value="replay">{t('手动请求')}</option><option value="both">{t('抓包与手动请求')}</option></select>
        <span className="plugin-list-count" role="status">{t('{v0} / {v1} 个插件',{v0:visible.length,v1:items.length})}</span>
      </div>
      {!visible.length&&<p className="plugin-list-empty">{t('没有匹配的插件，试试清除搜索或筛选条件。')}</p>}
      {visible.map(plugin=>{const index=items.findIndex(item=>item.id===plugin.id);return <div className="plugin-list-row" key={plugin.id} data-selected={detailId===plugin.id}>
        <label className="plugin-list-toggle"><input type="checkbox" aria-label={t('启用插件：{v0}',{v0:plugin.name})} disabled={busy} checked={plugin.enabled} onChange={event=>void save(items.map(value=>value.id===plugin.id?{...value,enabled:event.target.checked}:value))}/>{plugin.enabled?t('已启用'):t('已禁用')}</label>
        <div className="plugin-list-identity"><strong>{plugin.name}</strong><small>{plugin.id} · {plugin.version}</small></div><span className="plugin-list-scope">{scopeName(plugin.scope)}</span>
        <div className="plugin-list-actions">
          <button className="button" disabled={busy} aria-label={t('查看插件：{v0}',{v0:plugin.name})} onClick={()=>{setEditing(null);setCandidate(null);setDetailId(detailId===plugin.id?null:plugin.id);}}>{t('详情')}</button>
          <button className="button" disabled={busy} aria-label={t('编辑插件：{v0}',{v0:plugin.name})} onClick={()=>edit(plugin,plugin.id)}>{t('编辑')}</button>
          <button className="button" disabled={busy||index===0} aria-label={t('上移插件：{v0}',{v0:plugin.name})} onClick={()=>{const next=[...items];[next[index-1],next[index]]=[next[index],next[index-1]];void save(next);}}>{t('上移')}</button>
          <button className="button" disabled={busy||index===items.length-1} aria-label={t('下移插件：{v0}',{v0:plugin.name})} onClick={()=>{const next=[...items];[next[index+1],next[index]]=[next[index],next[index+1]];void save(next);}}>{t('下移')}</button>
          <button className="button" disabled={busy} aria-label={t('卸载插件：{v0}',{v0:plugin.name})} onClick={()=>void save(items.filter(value=>value.id!==plugin.id))}>{t('卸载')}</button>
        </div>
      </div>;})}
    </section>}
    {detail&&!editing&&!candidate&&<article className="plugin-detail"><h2>{detail.name}</h2><p>{detail.id} · API {detail.apiVersion} · {scopeName(detail.scope)}</p><p>{t('URL 正则表达式')}：<code>{detail.urlPattern||t('匹配任意 URL')}</code></p><h3>{t('请求前 JavaScript')}</h3><pre>{detail.before||t('无代码')}</pre><h3>{t('响应后 JavaScript')}</h3><pre>{detail.after||t('无代码')}</pre><button className="button" onClick={()=>setDetailId(null)}>{t('关闭详情')}</button></article>}
    <section className="plugins-logs"><h2>{t('插件日志')}</h2><pre style={{whiteSpace:'pre-wrap'}}>{logs.join('\n')||'—'}</pre></section>
    {helpOpen&&<PluginHelpDialog onClose={()=>setHelpOpen(false)}/>}
  </section>;
}
