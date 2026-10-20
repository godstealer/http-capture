import {useEffect, useId, useMemo, useRef, useState} from 'react';
import {ArrowDown, ArrowUp, Copy, Plus, RefreshCw, Replace, Save, Search, Trash2, Upload, Wand2, X} from 'lucide-react';
import {invoke} from './api';
import {t} from './i18n';
import BodyCodeEditor from './BodyCodeEditor';
import OrderedHeaders from './OrderedHeaders';
import {utf8Body} from './requestBody';
import {copyRewriteRule, emptyRewriteConfig, MAX_REWRITE_BODY_BYTES, MAX_REWRITE_RULES, moveRewriteRule, newModification,
  newRewriteRule, replacementEncoding, replacementFromFile, replacementParser, RewriteValidationError, seedRewriteRule,
  updateReplacementBody, validatedRewriteConfig, type Modification, type RewriteAction, type RewriteConfig,
  type RewriteRule, type RewriteSeed, type RewriteSnapshot} from './responseRewriteModel';
import './response-rewrite.css';

function errorText(error:unknown) {
  const message=error instanceof RewriteValidationError?t(error.message):String(error);
  return /Unknown command.*response_rewrite|\bHTTP\s+404\b/i.test(message)
    ? t('当前内核不支持自动响应重写，请更新并重启内核。'):message;
}
export default function ResponseRewrite({connected,seed,active=true}:{connected:boolean;seed?:RewriteSeed;active?:boolean}) {
  const heading=useId();
  const [snapshot,setSnapshot]=useState<RewriteSnapshot|null>(null),[config,setConfig]=useState<RewriteConfig>(emptyRewriteConfig);
  const [editing,setEditing]=useState(''),[query,setQuery]=useState('');
  const [busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState(''),[reload,setReload]=useState(0);
  const [encoding,setEncoding]=useState<'text'|'base64'>('text'),[parser,setParser]=useState('auto'),[editorEpoch,setEditorEpoch]=useState(0);
  const current=useRef(config),saved=useRef<RewriteSnapshot|null>(null),changed=useRef(false),operation=useRef(false),alive=useRef(false);
  const seedKey=useRef(''),fileEpoch=useRef(0),fileInput=useRef<HTMLInputElement>(null);
  const loadedReload=useRef<number>();
  const dirty=snapshot!==null&&JSON.stringify(config)!==JSON.stringify(snapshot.config);
  const locked=!connected||busy||snapshot===null;
  const rule=config.rules.find(value=>value.id===editing);
  const body=rule?.replacement?.bodyBase64??'';
  const text=useMemo(()=>utf8Body(body),[body]);
  const matching=config.rules.filter(value=>`${value.name} ${value.pattern} ${value.method}`.toLowerCase().includes(query.toLowerCase()));
  function update(value:RewriteConfig) {
    current.current=value;changed.current=saved.current===null||JSON.stringify(value)!==JSON.stringify(saved.current.config);
    setConfig(value);setError('');setNotice('');
  }
  function patch(id:string,value:Partial<RewriteRule>) {
    if(operation.current)return;
    update({...current.current,rules:current.current.rules.map(item=>item.id===id?{...item,...value}:item)});
  }
  function select(value:RewriteRule) {
    fileEpoch.current++;setEditing(value.id);setEncoding(replacementEncoding(value));setParser('auto');setEditorEpoch(value=>value+1);setError('');
  }
  function append(value:RewriteRule) {
    if(current.current.rules.length>=MAX_REWRITE_RULES){setError(t('最多允许 32 条自动响应规则。'));return false;}
    update({...current.current,rules:[...current.current.rules,value]});setQuery('');select(value);
    return true;
  }
  useEffect(()=>{alive.current=true;return()=>{alive.current=false;fileEpoch.current++}},[]);
  useEffect(()=>{
    if(!connected||!active){if(!operation.current)setBusy(false);return;}
    if(saved.current&&loadedReload.current===reload){if(!operation.current)setBusy(false);return;}
    let live=true;setBusy(true);setError('');
    invoke<RewriteSnapshot>('response_rewrite').then(value=>{
      if(!live)return;
      loadedReload.current=reload;
      if(changed.current){setNotice(t('已保留未保存草稿；保存时会检查内核配置版本。'));return;}
      saved.current=value;current.current=structuredClone(value.config);setSnapshot(value);setConfig(current.current);setNotice('');
    }).catch(error=>{if(live)setError(errorText(error))}).finally(()=>{if(live)setBusy(false)});
    return()=>{live=false};
  },[connected,reload,active]);
  useEffect(()=>{
    if(!seed||seed.key===seedKey.current||!snapshot||busy||!active)return;
    seedKey.current=seed.key;
    try{if(append(seedRewriteRule(seed.flow,seed.action,undefined,seed.responseBodyBase64)))setNotice(t('已创建禁用的捕获模板草稿；请填写内容、保存规则并确认开关后使用。'))}
    catch(error){setError(errorText(error))}
  },[seed?.key,snapshot,busy,active]);
  async function save(enableRuleId?:string) {
    if(operation.current||locked||!saved.current||(!changed.current&&!enableRuleId))return;
    let submitted:RewriteConfig;
    const candidate=enableRuleId?{...current.current,enabled:true,rules:current.current.rules.map(item=>item.id===enableRuleId?{...item,enabled:true}:item)}:current.current;
    try{submitted=validatedRewriteConfig(candidate)}catch(error){setError(errorText(error));return;}
    operation.current=true;setBusy(true);setError('');setNotice('');
    try{
      const next=await invoke<RewriteSnapshot>('configure_response_rewrite',{config:submitted,expectedVersion:saved.current.version});
      if(!alive.current)return;
      saved.current=next;current.current=structuredClone(next.config);changed.current=false;
      setSnapshot(next);setConfig(current.current);
      const selected=next.config.rules.find(item=>item.id===editing);
      setNotice(!next.config.enabled?t('自动响应规则已保存，但总开关关闭，不会自动处理响应。')
        :selected&&!selected.enabled?t('自动响应规则已保存，但当前规则关闭；请启用此规则后再次保存。')
        :!next.config.rules.some(item=>item.enabled)?t('自动响应规则已保存，但没有启用的规则。')
        :t('自动响应规则已保存，后续匹配的响应生效；命中次数已重置。'));
    }catch(error){if(alive.current)setError(errorText(error))}
    finally{operation.current=false;if(alive.current)setBusy(false)}
  }
  function cancel() {
    if(!saved.current||operation.current)return;
    fileEpoch.current++;current.current=structuredClone(saved.current.config);changed.current=false;
    setConfig(current.current);setEditing('');setError('');setNotice('');
  }
  function bodyChange(value:string) {
    if(!rule||operation.current)return;
    try{patch(rule.id,updateReplacementBody(rule,value,encoding))}catch(error){setError(errorText(error))}
  }
  async function readFile(file:File|undefined) {
    if(!file||!rule||rule.action!=='replace'||operation.current)return;
    const id=rule.id,epoch=++fileEpoch.current;operation.current=true;setBusy(true);setError('');
    try{
      if(file.size>MAX_REWRITE_BODY_BYTES)throw new RewriteValidationError('替换正文不能超过 8 MiB。');
      const data=new Uint8Array(await file.arrayBuffer());
      if(!alive.current||epoch!==fileEpoch.current)return;
      const selected=current.current.rules.find(value=>value.id===id);if(!selected)return;
      const next=replacementFromFile(selected,data,file.type);
      update({...current.current,rules:current.current.rules.map(value=>value.id===id?next:value)});
      setEncoding(replacementEncoding(next));setParser('auto');setEditorEpoch(value=>value+1);setNotice(t('文件已读入草稿，保存规则后才会用于响应。'));
    }catch(error){if(alive.current&&epoch===fileEpoch.current)setError(errorText(error))}
    finally{operation.current=false;if(alive.current)setBusy(false);if(fileInput.current)fileInput.current.value=''}
  }
  function changeModification(index:number,value:Partial<Modification>) {
    if(!rule)return;patch(rule.id,{modifications:rule.modifications.map((item,i)=>i===index?{...item,...value}:item)});
  }
  return <section className="response-rewrite-page" aria-labelledby={heading} hidden={!active}>
    <header className="rewrite-heading"><div><h1 id={heading}><Replace size={23}/>{t('响应重写')}</h1><p>{t('预设规则自动替换或修改普通 HTTP 响应，无需逐次暂停和放行。')}</p></div><span className={'rewrite-state'+(snapshot?.config.enabled?' is-enabled':'')}>{snapshot?.config.enabled?t('已开启'):t('已关闭')}</span></header>
    <p className="rewrite-scope-note">{t('先正常访问上游，再按列表顺序执行匹配规则；不是跳过上游的 Mock。规则出错时保留该规则执行前的响应并记录原因。')}</p>
    <p className="rewrite-help">{t('命中次数来自最近一次读取；重新加载会刷新，保存配置会清零。')}</p>
    {!connected&&<p role="status" className="rewrite-message">{t('连接内核后可管理自动响应规则。')}</p>}
    {busy&&<p role="status" className="rewrite-message">{t('正在处理自动响应规则…')}</p>}
    {error&&<p role="alert" className="rewrite-message rewrite-error">{error}</p>}{notice&&<p role="status" className="rewrite-message rewrite-notice">{notice}</p>}
    <div className="rewrite-config-bar"><label><input type="checkbox" disabled={locked} checked={config.enabled} onChange={event=>update({...current.current,enabled:event.target.checked})}/>{t('启用自动响应重写')}</label><label>{t('作用范围')}<select aria-label={t('自动响应作用范围')} disabled={locked} value={config.scope} onChange={event=>update({...current.current,scope:event.target.value as RewriteConfig['scope']})}><option value="all">{t('全部来源')}</option><option value="capture">{t('仅抓包流量')}</option><option value="replay">{t('仅手动请求')}</option></select></label><div className="rewrite-config-actions"><span>{dirty?t('有未保存的规则修改'):t('已与保存的配置同步')}</span><button type="button" disabled={locked||!dirty} onClick={cancel}>{t('取消修改')}</button><button type="button" disabled={!connected||busy||dirty} title={dirty?t('请先保存或取消当前修改'):undefined} onClick={()=>setReload(value=>value+1)}><RefreshCw size={14}/>{t('重新加载')}</button><button type="button" className="rewrite-primary" disabled={locked||!dirty} onClick={()=>void save()}><Save size={14}/>{t('保存全部规则')}</button></div></div>
    <div className="rewrite-action-cards">{(['replace','modify'] as const).map(action=><button type="button" key={action} className={'rewrite-action-card'+(rule?.action===action?' is-selected':'')} disabled={locked||config.rules.length>=MAX_REWRITE_RULES} onClick={()=>append(newRewriteRule(action))}>{action==='replace'?<Replace size={22}/>:<Wand2 size={22}/>}<span><strong>{action==='replace'?t('替换整份响应'):t('修改部分内容')}</strong><small>{action==='replace'?t('统一状态码、响应头和正文，支持文本或本地文件。'):t('按文本或正则改正文，也可设置、替换或删除响应头。')}</small></span><Plus size={17}/></button>)}</div>
    <div className="rewrite-workspace"><aside className="rewrite-list-panel"><div className="rewrite-list-heading"><strong>{t('自动响应规则')}</strong><span>{config.rules.length} / 32</span></div><label className="rewrite-search"><Search size={15}/><input aria-label={t('搜索响应规则')} placeholder={t('按名称、URL 或方法搜索')} value={query} onChange={event=>setQuery(event.target.value)}/></label>
      <div className="rewrite-rule-list">{matching.map(item=>{
        const index=config.rules.findIndex(value=>value.id===item.id),savedIndex=snapshot?.config.rules.findIndex(value=>value.id===item.id)??-1,hits=savedIndex<0?0:snapshot?.hits[savedIndex]??0;
        return <article key={item.id} className={'rewrite-rule-row'+(editing===item.id?' is-selected':'')+(item.enabled?'':' is-disabled')}><label className="rewrite-row-toggle"><input type="checkbox" disabled={locked} aria-label={t('启用响应规则 {v0}',{v0:item.name})} checked={item.enabled} onChange={event=>patch(item.id,{enabled:event.target.checked})}/></label><button type="button" className="rewrite-row-main" disabled={locked} onClick={()=>select(item)}><strong>{item.name||t('未命名规则')}</strong><small>{item.action==='replace'?t('替换整份响应'):t('文本 / 正则')} · {item.method||t('任意方法')}</small><code>{item.pattern||t('尚未填写匹配内容')}</code><span>{t('本次配置命中 {v0} 次',{v0:hits})}</span></button><div className="rewrite-row-actions"><button type="button" disabled={locked||index===0} aria-label={t('上移响应规则 {v0}',{v0:item.name})} onClick={()=>update({...current.current,rules:moveRewriteRule(current.current.rules,index,-1)})}><ArrowUp size={14}/></button><button type="button" disabled={locked||index===config.rules.length-1} aria-label={t('下移响应规则 {v0}',{v0:item.name})} onClick={()=>update({...current.current,rules:moveRewriteRule(current.current.rules,index,1)})}><ArrowDown size={14}/></button><button type="button" disabled={locked||config.rules.length>=MAX_REWRITE_RULES} aria-label={t('复制响应规则 {v0}',{v0:item.name})} onClick={()=>append(copyRewriteRule(item))}><Copy size={14}/></button><button type="button" disabled={locked} aria-label={t('删除响应规则 {v0}',{v0:item.name})} onClick={()=>{update({...current.current,rules:current.current.rules.filter(value=>value.id!==item.id)});if(editing===item.id)setEditing('')}}><Trash2 size={14}/></button></div></article>;
      })}{!matching.length&&<div className="rewrite-empty"><Replace size={26}/><strong>{config.rules.length?t('没有匹配的规则'):t('尚未添加自动响应规则')}</strong><p>{t('使用上方两种操作创建规则，或从抓包详情使用捕获响应模板。')}</p></div>}</div>
    </aside><div className="rewrite-editor-panel">{!rule?<div className="rewrite-empty"><Wand2 size={30}/><strong>{t('选择或新建响应规则')}</strong><p>{t('新规则默认关闭。列表编辑、复制、排序和删除均需保存后才生效。')}</p></div>:<form onSubmit={event=>{event.preventDefault();void save()}}><div className="rewrite-editor-heading"><h2>{rule.action==='replace'?t('替换整份响应'):t('修改部分内容')}</h2><button type="button" className="rewrite-icon" aria-label={t('收起规则编辑器')} disabled={busy} onClick={()=>setEditing('')}><X size={17}/></button></div><fieldset disabled={locked}>
      <div className="rewrite-field-grid"><label>{t('规则名称')}<input aria-label={t('自动响应规则名称')} maxLength={128} value={rule.name} onChange={event=>patch(rule.id,{name:event.target.value})}/></label><label className="rewrite-enable-rule"><input type="checkbox" checked={rule.enabled} onChange={event=>patch(rule.id,{enabled:event.target.checked})}/>{t('启用此规则')}</label><label>{t('匹配方式')}<select aria-label={t('自动响应匹配方式')} value={rule.matchType} onChange={event=>patch(rule.id,{matchType:event.target.value as RewriteRule['matchType']})}><option value="urlContains">{t('URL 包含')}</option><option value="urlRegex">{t('URL 正则表达式')}</option><option value="host">{t('域名精确匹配')}</option></select></label><label>{t('请求方法（留空匹配全部）')}<input aria-label={t('自动响应请求方法')} maxLength={32} placeholder="GET / POST" value={rule.method} onChange={event=>patch(rule.id,{method:event.target.value.toUpperCase()})}/></label><label className="rewrite-wide">{t('匹配内容')}<input aria-label={t('自动响应匹配内容')} maxLength={2048} required spellCheck={false} value={rule.pattern} placeholder={rule.matchType==='host'?'example.com':rule.matchType==='urlRegex'?'^https://example\\.com/api$':'https://example.com/api'} onChange={event=>patch(rule.id,{pattern:event.target.value})}/></label></div>
      <p className="rewrite-help">{t('URL 正则使用 Rust 语法，不支持环视或反向引用；域名只做精确匹配。每条规则的条件须同时满足。')}</p>
      {rule.action==='replace'&&rule.replacement?<><div className="rewrite-section-heading"><h3>{t('替换响应')}</h3><label>{t('响应状态码')}<input aria-label={t('替换响应状态码')} type="number" min={200} max={599} step={1} required value={rule.replacement.status} onChange={event=>patch(rule.id,{replacement:{...rule.replacement!,status:Number(event.target.value)}})}/></label></div><h3>{t('响应头')}</h3><OrderedHeaders key={rule.id} context="response" headers={rule.replacement.headers} onChange={headers=>patch(rule.id,{replacement:{...rule.replacement!,headers}})}/><div className="rewrite-body-toolbar"><h3>{t('替换正文')}</h3><label>{t('正文编码')}<select aria-label={t('替换正文编码')} value={encoding} onChange={event=>{setEncoding(event.target.value as 'text'|'base64');setEditorEpoch(value=>value+1)}}><option value="text">UTF-8</option><option value="base64">Base64</option></select></label>{encoding==='text'&&<select aria-label={t('替换正文语言')} value={parser} onChange={event=>setParser(event.target.value)}><option value="auto">{t('自动识别')}</option><option value="text">Text</option><option value="json">JSON</option><option value="html">HTML / SVG</option><option value="babel">JavaScript</option><option value="css">CSS</option></select>}<button type="button" onClick={()=>fileInput.current?.click()}><Upload size={14}/>{t('从文件读入正文')}</button><input hidden type="file" ref={fileInput} onChange={event=>{const file=event.target.files?.[0];event.target.value='';void readFile(file)}}/></div><p className="rewrite-help">{t('正文最多 8 MiB。Base64 保留内容编码；修改 UTF-8 文本或读入文件会移除旧编码、长度和失效的 ETag 等校验头。')}</p>{encoding==='text'&&text===null?<p className="rewrite-message">{t('当前正文不是有效 UTF-8，请切换 Base64 保留原始字节。')}</p>:<BodyCodeEditor key={`${rule.id}-${encoding}-${editorEpoch}`} label={t('自动替换响应正文')} value={encoding==='base64'?body:text??''} parser={encoding==='base64'?'text':parser==='auto'?replacementParser(rule):parser} formatting={encoding==='text'} disabled={locked} onChange={bodyChange}/>}</>:<><div className="rewrite-section-heading"><h3>{t('按顺序修改')}</h3><span>{rule.modifications.length} / 32</span></div><p className="rewrite-help">{t('仅修改 UTF-8 正文或指定响应头，保留状态码。正文修改会先受限解压，实际改变后移除旧编码与校验头。')}</p><p className="rewrite-help">{t('正文修改不处理 HEAD、204、205、206、304、WebSocket、SSE 或 gRPC 流；不支持的正文保留并记录原因。')}</p><p className="rewrite-help">{t('正文必须是有效 UTF-8 且不含 NUL；显式非 UTF-8 字符集、非 SVG 图片、音视频、PDF 和二进制文件不修改正文。仅改响应头不受这些正文限制。')}</p>{rule.modifications.map((modification,index)=><div className="rewrite-modification" key={index}><header><strong>{t('修改项 {v0}',{v0:index+1})}</strong><div><button type="button" disabled={index===0} aria-label={t('上移修改项 {v0}',{v0:index+1})} onClick={()=>patch(rule.id,{modifications:moveRewriteRule(rule.modifications,index,-1)})}><ArrowUp size={14}/></button><button type="button" disabled={index===rule.modifications.length-1} aria-label={t('下移修改项 {v0}',{v0:index+1})} onClick={()=>patch(rule.id,{modifications:moveRewriteRule(rule.modifications,index,1)})}><ArrowDown size={14}/></button><button type="button" aria-label={t('删除修改项 {v0}',{v0:index+1})} onClick={()=>patch(rule.id,{modifications:rule.modifications.filter((_,i)=>i!==index)})}><Trash2 size={14}/></button></div></header><div className="rewrite-field-grid"><label>{t('修改目标')}<select aria-label={t('修改目标 {v0}',{v0:index+1})} value={modification.target} onChange={event=>changeModification(index,event.target.value==='body'?{target:'body',operation:'replace',name:''}:{target:'header',operation:'set',name:''})}><option value="body">{t('响应正文')}</option><option value="header">{t('响应头')}</option></select></label><label>{t('修改操作')}<select aria-label={t('修改操作 {v0}',{v0:index+1})} value={modification.operation} onChange={event=>changeModification(index,{operation:event.target.value as Modification['operation']})}><option value="replace">{t('查找并替换')}</option>{modification.target==='header'&&<><option value="set">{t('设置响应头')}</option><option value="remove">{t('删除响应头')}</option></>}</select></label>{modification.target==='header'&&<label className="rewrite-wide">{t('响应头名称')}<input aria-label={t('响应头名称 {v0}',{v0:index+1})} required spellCheck={false} placeholder="X-Debug" value={modification.name} onChange={event=>changeModification(index,{name:event.target.value})}/></label>}{modification.operation==='replace'&&<label className="rewrite-wide">{modification.regex?t('查找正则'):t('查找文本')}<textarea aria-label={t('查找内容 {v0}',{v0:index+1})} required spellCheck={false} value={modification.find} onChange={event=>changeModification(index,{find:event.target.value})}/></label>}{modification.operation!=='remove'&&<label className="rewrite-wide">{t('替换内容（可以为空）')}<textarea aria-label={t('替换内容 {v0}',{v0:index+1})} spellCheck={false} value={modification.value} onChange={event=>changeModification(index,{value:event.target.value})}/></label>}</div><div className="rewrite-modification-options">{modification.operation==='replace'&&<><label><input type="checkbox" checked={modification.regex} onChange={event=>changeModification(index,{regex:event.target.checked})}/>{t('使用正则表达式')}</label><label><input type="checkbox" checked={modification.caseSensitive} onChange={event=>changeModification(index,{caseSensitive:event.target.checked})}/>{t('区分大小写')}</label></>}<label>{t('应用次数')}<select aria-label={t('应用次数 {v0}',{v0:index+1})} value={modification.all?'all':'first'} onChange={event=>changeModification(index,{all:event.target.value==='all'})}><option value="first">{t('仅首个匹配')}</option><option value="all">{t('全部匹配')}</option></select></label></div>{modification.regex&&modification.operation==='replace'&&<p className="rewrite-help">{t('正则替换支持 $1 和 ${name} 分组；普通文本替换中的 $ 按字面处理。')}</p>}</div>)}<button type="button" disabled={rule.modifications.length>=32} onClick={()=>patch(rule.id,{modifications:[...rule.modifications,newModification()]})}><Plus size={14}/>{t('添加修改项')}</button></>}
      <p className="rewrite-help">{t('Content-Length、Transfer-Encoding 和 Connection 由内核管理。修改规则不能直接设置这些头。')}</p><div className="rewrite-editor-footer"><span>{t('总开关与规则开关都开启后，已保存规则才会自动执行。')}</span><button type="submit" disabled={!dirty}><Save size={14}/>{t('保存全部规则')}</button><button type="button" className="rewrite-primary" disabled={!dirty&&config.enabled&&rule.enabled} onClick={()=>void save(rule.id)}><Wand2 size={14}/>{t('保存并启用此规则')}</button></div>
    </fieldset></form>}</div></div>
  </section>;
}
