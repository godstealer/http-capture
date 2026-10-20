import { writeClipboard } from './clipboard';
import { t } from './i18n';
import { CaptureScripts } from './ScriptsEditor';
import { useLayoutEffect, useRef, useState } from 'react';
import { invoke } from './api';
import { encodeText, type Flow } from './types';
import OrderedHeaders from './OrderedHeaders';
import ResponseBody from './ResponseBody';
import BodyCodeEditor from './BodyCodeEditor';
import { utf8ContentType } from './requestBody';
import './interception.css';
type Rule = {disabled?:boolean;exclude?:boolean;group?:string;urlRegex?:boolean;urlContains:string;host:string;method:string;status:number|null;headerName:string;headerContains:string};
type Config = {request:boolean;response:boolean;scope:string;rules?:Rule[]};
type Item = {id:string;stage:string;flow:Flow};
export type InterceptSnapshot = {hits?:number[];config:Config;items:Item[]};
export default function Interception({snapshot,onUpdate}:{snapshot:InterceptSnapshot;onUpdate:(s:InterceptSnapshot)=>void}) {
 const [selected,setSelected]=useState(''); const [error,setError]=useState(''); const [saving,setSaving]=useState(false);
 const item=snapshot.items.find(i=>i.id===selected)??snapshot.items[0];
 async function configure(config:Config) { setSaving(true); try {await invoke('configure_interception',{config:{...config,scope:config.scope||'all'}});onUpdate(await invoke<InterceptSnapshot>('interception'));setError('');return true;} catch(e){setError(String(e));return false;} finally {setSaving(false);} }
 return <section className="interception-page"><div className="interception-controls"><strong>{t("拦截")}</strong>{(['request','response'] as const).map(stage=><label key={stage}><input disabled={saving} type="checkbox" checked={snapshot.config[stage]} onChange={e=>void configure({...snapshot.config,[stage]:e.target.checked})}/>{stage==='request'?t("请求发送前"):t("响应返回后")}</label>)}<select aria-label={t("拦截来源")} disabled={saving} value={snapshot.config.scope||'all'} onChange={e=>void configure({...snapshot.config,scope:e.target.value})}><option value="all">{t("全部来源")}</option><option value="capture">{t("抓包流量")}</option><option value="replay">{t("手动发送")}</option></select><span>{t("每阶段最多等待 120 秒；关闭开关将原样放行。")}</span></div><p className="interception-response-help">{t("响应替换：在抓包详情点击“拦截替换响应”，重新发起请求，再在等待项中修改状态码、响应头或正文并放行。已完成的历史响应不会改变。")}</p>{error&&<p role="alert">{error}</p>}<Rules hits={snapshot.hits??[]} config={snapshot.config} save={configure} saving={saving}/><CaptureScripts/><div className="interception-workspace"><aside>{snapshot.items.map(i=><button key={i.id} className={item?.id===i.id?'active':''} onClick={()=>setSelected(i.id)}><strong>{i.stage==='request'?t("等待请求放行"):t("等待响应放行")} · {i.flow.source==='capture'?t("抓包"):t("手动")}</strong><span>{i.flow.request.method} {i.flow.request.url}</span></button>)}{!snapshot.items.length&&<p>{t("暂无等待项。开启拦截后，新的流量会在这里暂停。")}</p>}</aside>{item&&<InterceptionEditor key={item.id} item={item} onDone={async()=>onUpdate(await invoke<InterceptSnapshot>('interception'))}/>}</div></section>;
}
function Rules({config,save,saving,hits}:{hits:number[];config:Config;save:(c:Config)=>Promise<boolean>;saving:boolean}) {
 const fileInput=useRef<HTMLInputElement>(null);const [fileError,setFileError]=useState('');
 const [draft,setDraft]=useState<Rule[]|null>(null); const rules=draft??config.rules??[];
 const update=(index:number,patch:Partial<Rule>)=>setDraft(rules.map((r,i)=>i===index?{...r,...patch}:r));
 function exportRules(){const url=URL.createObjectURL(new Blob([JSON.stringify({format:'http-capture-rules',version:1,rules},null,2)],{type:'application/json'}));const a=document.createElement('a');a.href=url;a.download='capture-rules.json';a.click();setTimeout(()=>URL.revokeObjectURL(url),30000);}
 async function importRules(file:File){try{if(file.size>1024*1024)throw Error('Rules file exceeds 1 MiB');const value=JSON.parse(await file.text());if(value.format!=='http-capture-rules'||value.version!==1||!Array.isArray(value.rules)||value.rules.length>32)throw Error('Invalid rules file');for(const r of value.rules){if(!r||!['urlContains','host','method','headerName','headerContains'].every(k=>typeof r[k]==='string')||!['disabled','exclude','urlRegex'].every(k=>r[k]===undefined||typeof r[k]==='boolean')||(r.group!==undefined&&typeof r.group!=='string')||(r.status!==null&&(!Number.isInteger(r.status)||r.status<100||r.status>599)))throw Error('Invalid rule');}setDraft(value.rules);setFileError('');}catch(e){setFileError(String(e));}}
 return <details open className="interception-rules"><summary>{t("匹配规则 · 已应用")}{config.rules?.length??0} {t("条")}</summary><p>{t("只拦截匹配的流量；没有规则时全部通过。同一行条件须同时满足，多行满足任意一行即可。状态码条件只匹配响应阶段。正则匹配完整 URL 中的任意位置；用 ^ 和 $ 限定边界，(?i) 忽略大小写，不需要 /…/ 分隔符。不支持环视或反向引用。")}</p>{rules.map((r,i)=><div className="interception-controls interception-rule" key={i}>
 <label><input type="checkbox" checked={!r.disabled} onChange={e=>update(i,{disabled:!e.target.checked})}/>{t('启用')}</label><label><input type="checkbox" checked={!!r.exclude} onChange={e=>update(i,{exclude:e.target.checked})}/>{t('排除')}</label><input aria-label={t('规则分组')} placeholder={t('规则分组')} value={r.group??''} onChange={e=>update(i,{group:e.target.value})}/><small>{t('断点命中')} {hits[i]??0}</small>
 <select aria-label={t("URL 匹配方式 {v0}", { v0: i+1 })} value={r.urlRegex?'regex':'contains'} onChange={e=>update(i,{urlRegex:e.target.value==='regex'})}><option value="contains">{t("URL 包含")}</option><option value="regex">{t("URL 正则表达式")}</option></select><input aria-label={t("URL 条件 {v0}", { v0: i+1 })} placeholder={r.urlRegex?t("如 ^https://httpbin\\.org/.*"):t("URL 包含（区分大小写）")} value={r.urlContains} onChange={e=>update(i,{urlContains:e.target.value})}/>
 <input aria-label={t("域名 {v0}", { v0: i+1 })} placeholder={t("域名精确匹配，如 httpbin.org")} value={r.host} onChange={e=>update(i,{host:e.target.value})}/>
 <input aria-label={t("方法 {v0}", { v0: i+1 })} placeholder={t("方法，如 POST")} value={r.method} onChange={e=>update(i,{method:e.target.value})}/>
 <input aria-label={t("状态码 {v0}", { v0: i+1 })} type="number" min="100" max="599" placeholder={t("响应状态码")} value={r.status??''} onChange={e=>update(i,{status:e.target.value?Number(e.target.value):null})}/>
 <input aria-label={t("请求头名称 {v0}", { v0: i+1 })} placeholder={t("请求头名称")} value={r.headerName} onChange={e=>update(i,{headerName:e.target.value})}/>
 <input aria-label={t("请求头值包含 {v0}", { v0: i+1 })} placeholder={t("请求头值包含（空为存在即可）")} value={r.headerContains} onChange={e=>update(i,{headerContains:e.target.value})}/>
 <button onClick={()=>setDraft(rules.filter((_,n)=>n!==i))}>{t("删除规则")}{i+1}</button></div>)}
 <div className="interception-controls"><button onClick={exportRules}>{t('导出规则')}</button><button disabled={saving} onClick={()=>fileInput.current?.click()}>{t('导入规则')}</button><input hidden type="file" accept=".json" ref={fileInput} onChange={e=>{const file=e.target.files?.[0];e.target.value='';if(file)void importRules(file);}}/>{fileError&&<span role="alert">{fileError}</span>}</div><p>{t('排除规则优先；分组仅用于标记。命中次数为本次配置的断点暂停次数，修改配置后归零。')}</p><div className="interception-controls"><button disabled={rules.length>=32} onClick={()=>setDraft([...rules,{urlContains:'',host:'',method:'',status:null,headerName:'',headerContains:''}])}>{t("添加规则")}</button><button disabled={saving||draft===null} onClick={()=>void save({...config,rules}).then(ok=>{if(ok)setDraft(null);})}>{t("应用规则")}</button><button disabled={draft===null} onClick={()=>setDraft(null)}>{t("取消修改")}</button>{draft!==null&&<span>{t("有未应用的修改")}</span>}</div></details>;
}
const defaultResponse = (item: Item): NonNullable<Flow['response']> => item.flow.response ?? {
  status: 200, version: 'HTTP/1.1', headers: [{name: 'Content-Type', value: 'text/plain; charset=utf-8'}], bodyBase64: '',
};
function bodyLanguage(headers: Flow['request']['headers']) {
  const mime = headers.find(h => !h.disabled && h.name.toLowerCase() === 'content-type')?.value.split(';')[0].trim().toLowerCase() ?? '';
  if (mime === 'application/json' || mime.endsWith('+json')) return 'json';
  if (['text/html', 'application/xhtml+xml', 'image/svg+xml'].includes(mime)) return 'html';
  if (/(?:java|ecma)script$/.test(mime)) return 'babel';
  return mime === 'text/css' ? 'css' : 'text';
}
export function InterceptionEditor({item, onDone}: {item: Item; onDone: () => Promise<void>}) {
  const [request, setRequest] = useState(item.flow.request);
  const [response, setResponse] = useState(() => defaultResponse(item));
  const [replace, setReplace] = useState(false), [base64, setBase64] = useState(false), [preview, setPreview] = useState(false);
  const [parser, setParser] = useState('auto'), [editorEpoch, setEditorEpoch] = useState(0);
  const [error, setError] = useState(''), [busy, setBusy] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null), fileReader = useRef<FileReader>(), fileRevision = useRef(0), alive = useRef(true);
  useLayoutEffect(() => { alive.current = true; return () => { alive.current = false; fileRevision.current++; fileReader.current?.abort(); }; }, []);
  const editingResponse = item.stage === 'response' || replace, message = editingResponse ? response : request;
  const language = parser === 'auto' ? bodyLanguage(message.headers) : parser;
  const context = `${editingResponse}-${base64}-${preview}-${editorEpoch}-${busy}`;
  const bodyContext = useRef(context);
  bodyContext.current = context;
  let text: string | null;
  try { text = new TextDecoder('utf-8', {fatal: true}).decode(Uint8Array.from(atob(message.bodyBase64), c => c.charCodeAt(0))); }
  catch { text = null; }
  async function replaceFile(file: File) {
    const revision = ++fileRevision.current;
    fileReader.current?.abort();
    setBusy(true);
    try {
      if (file.size > 8 * 1024 * 1024) throw new Error(t('正文超过 8 MiB'));
      const bodyBase64 = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader(); fileReader.current = reader;
        reader.onload = () => resolve(String(reader.result).split(',', 2)[1]);
        reader.onerror = () => reject(reader.error);
        reader.onabort = () => reject(new Error('cancelled'));
        reader.readAsDataURL(file);
      });
      if (!alive.current || revision !== fileRevision.current) return;
      setResponse(current => ({...current, bodyBase64, headers: [
        ...current.headers.filter(h => h.disabled || !['content-type', 'content-encoding', 'content-length', 'transfer-encoding'].includes(h.name.toLowerCase())),
        {name: 'Content-Type', value: file.type || 'application/octet-stream'},
      ]}));
      setEditorEpoch(n => n + 1); setPreview(true); setError('');
    } catch (e) { if (alive.current && revision === fileRevision.current) setError(String(e)); }
    finally { if (revision === fileRevision.current) { fileReader.current = undefined; if (alive.current) setBusy(false); } }
  }
  const setHeaders = (headers: typeof request.headers) => {
    if (busy) return;
    if (editingResponse) setResponse(current => ({...current, headers})); else setRequest(current => ({...current, headers}));
  };
  function setBody(value: string) {
    if (busy || !alive.current || bodyContext.current !== context) return;
    const bodyBase64 = base64 ? value : encodeText(value);
    const headers = (current: typeof request.headers) => current.filter(h => h.disabled ||
      (!['content-length', 'transfer-encoding'].includes(h.name.toLowerCase()) && (base64 || h.name.toLowerCase() !== 'content-encoding')))
      .map(h => !base64 && !h.disabled && h.name.toLowerCase() === 'content-type' ? {...h, value: utf8ContentType(h.value)} : h);
    if (editingResponse) setResponse(current => ({...current, bodyBase64, headers: headers(current.headers)}));
    else setRequest(current => ({...current, bodyBase64, headers: headers(current.headers)}));
  }
  async function resolve(action: string) {
    setBusy(true);
    try {
      await invoke('resolve_interception', {decision: {id: item.id, action,
        request: action === 'modify' && !editingResponse ? request : null,
        response: action === 'replace' || action === 'modify' && editingResponse ? response : null}});
      await onDone();
    } catch (e) { if (alive.current) setError(String(e)); }
    finally { if (alive.current) setBusy(false); }
  }
  return <div className="interception-editor">
    <fieldset disabled={busy} className="interception-edit-fields">
      <div className="interception-controls interception-editor-heading">
        <strong>{editingResponse ? t('响应编辑') : t('请求编辑')}</strong>
        {item.stage === 'request' && <label><input type="checkbox" checked={replace} onChange={e => { setReplace(e.target.checked); setParser('auto'); }}/>{t('直接替换响应（不发送请求）')}</label>}
        <button type="button" onClick={() => { setRequest(item.flow.request); setResponse(defaultResponse(item)); setEditorEpoch(n => n + 1); setError(''); }}>{t('恢复原始内容')}</button>
      </div>
      {editingResponse ? <label className="interception-status">{t('状态码')}<input aria-label={t('拦截响应状态码')} type="number" min="100" max="599" value={response.status} onChange={e => { const status = Number(e.target.value); setResponse(current => ({...current, status})); }}/></label>
        : <div className="request-bar"><input aria-label={t('拦截请求方法')} value={request.method} onChange={e => { const method = e.target.value; setRequest(current => ({...current, method})); }}/><input aria-label={t('拦截请求 URL')} value={request.url} onChange={e => { const url = e.target.value; setRequest(current => ({...current, url})); }}/></div>}
      <div className="interception-section"><OrderedHeaders key={`${editingResponse}-${editorEpoch}`} headers={message.headers} onChange={setHeaders}/></div>
      <section className="interception-section interception-body">
        <div className="interception-controls">
          <strong>{t('正文')}</strong>
          <select aria-label={t('拦截正文编码')} value={base64 ? 'base64' : 'text'} onChange={e => setBase64(e.target.value === 'base64')}><option value="text">UTF-8</option><option value="base64">{t('Base64（原始字节）')}</option></select>
          {!base64 && <select aria-label={t('拦截正文语言')} value={parser} onChange={e => setParser(e.target.value)}>
            <option value="auto">{t('自动识别')}</option><option value="text">Text</option><option value="json">JSON</option><option value="html">HTML / SVG</option><option value="babel">JavaScript</option><option value="css">CSS</option>
          </select>}
          {editingResponse && <><button type="button" onClick={() => fileInput.current?.click()}>{t('从文件替换响应正文')}</button><input hidden type="file" ref={fileInput} onChange={e => { const file = e.target.files?.[0]; e.target.value = ''; if (file) void replaceFile(file); }}/></>}
          <button type="button" aria-pressed={preview} onClick={() => setPreview(!preview)}>{preview ? t('编辑') : t('预览')}</button>
        </div>
        {editingResponse && <p className="interception-hint">{t('Base64 编辑保留原始压缩声明；UTF-8 编辑会移除压缩声明并将已有 charset 改为 UTF-8。修改正文会移除长度和传输编码头。')}</p>}
        {preview ? <ResponseBody body={message.bodyBase64} headers={message.headers} onCopy={v => void writeClipboard(v).catch(e => { if (alive.current) setError(String(e)); })}/>
          : !base64 && text === null ? <p className="interception-hint">{t('正文包含非 UTF-8 字节，请使用 Base64 编辑，原始字节已保留。')} <button type="button" onClick={() => setBase64(true)}>{t('切换到 Base64')}</button></p>
          : <BodyCodeEditor key={`${editingResponse}-${base64}-${editorEpoch}`} label={t('拦截正文')} value={base64 ? message.bodyBase64 : text!} parser={base64 ? 'text' : language} onChange={setBody} formatting={!base64} disabled={busy}/>}
      </section>
      <details className="interception-original"><summary>{t('原始内容（保留备查）')}</summary><pre>{JSON.stringify(item.stage === 'request' ? item.flow.request : item.flow.response, null, 2)}</pre></details>
    </fieldset>
    {error && <p role="alert">{error}</p>}
    <div className="interception-controls interception-actions"><button disabled={busy} onClick={() => void resolve('continue')}>{t('原样放行')}</button><button className="interception-primary" disabled={busy} onClick={() => void resolve(replace ? 'replace' : 'modify')}>{replace ? t('返回自定义响应') : item.stage === 'response' ? t('替换响应并放行') : t('修改后放行')}</button><button disabled={busy} onClick={() => void resolve('abort')}>{t('终止')}</button></div>
  </div>;
}
