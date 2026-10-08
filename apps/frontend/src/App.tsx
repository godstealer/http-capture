import { t, useLanguage } from './i18n';
import LanguageSelector from './LanguageSelector';
import TunSettings, { type TunStatus } from './TunSettings';
import ListenerSettings from './ListenerSettings';
import { applyRequestResult, RequestRuns } from './requestRuns';
import ScriptsEditor, { emptyScripts } from './ScriptsEditor';
import OrderedHeaders from './OrderedHeaders';
import Interception, { type InterceptSnapshot } from './Interception';
import { useEffect, useRef, useState } from 'react';
import { Activity, ArrowDown, ArrowUp, ArrowUpRight, Check, ChevronDown, CircleHelp, Code2, Copy, FileJson, Globe2, Layers3, LockKeyhole, Play, Plus, Radio, RotateCw, Search, Send, Settings2, ShieldCheck, Square, Terminal, Trash2, X } from 'lucide-react';
import { isTauri } from '@tauri-apps/api/core';
import { invoke } from './api';
import { listen } from '@tauri-apps/api/event';
import CaptureView from './CaptureView';
import ResponseBody from './ResponseBody';
import HeaderOrder from './HeaderOrder';
import { RequestOverview, TlsDetails } from './RequestDetails';
import EngineSelector from './EngineSelector';
import RequestProxySelector from './RequestProxySelector';
import UpstreamSettings from './UpstreamSettings';
import { useRequestWorkspace } from './useRequestWorkspace';
import SplitPane from './SplitPane';
import { emptyRequest, importCurl, looksLikeCurl } from './importCurl';
import { applyTheme, readTheme, type Theme } from './theme';
import { decodeText, encodeText, type Flow, type ProxyStatus, type RequestDraft } from './types';

const desktop = isTauri();
const clone = <T,>(v: T): T => JSON.parse(JSON.stringify(v));
const blank: RequestDraft = emptyRequest();

export default function App() {
  useLanguage();
  const [interception, setInterception] = useState<InterceptSnapshot>({config:{request:false,response:false,scope:'all'},items:[]});
  const [tun, setTun] = useState<TunStatus>({available:false,running:false,helperPath:''});
  const [theme, setTheme] = useState(readTheme);
  const [flows, setFlows] = useState<Flow[]>([]);
  const { editors, setEditors, activeEditor, setActiveEditor, ready: workspaceReady, message: workspaceMessage, error: workspaceError, retry: saveWorkspace } = useRequestWorkspace();
  const editor = editors.find(item => item.id === activeEditor) ?? editors[0];
  const draft = editor?.draft ?? blank;
  const selected = editor?.selected ?? null;
  function setDraft(value: RequestDraft) { setEditors(items => items.map(item => item.id === editor?.id ? { ...item, draft: value } : item)); }
  function setSelected(value: string | null) { setEditors(items => items.map(item => item.id === editor?.id ? { ...item, selected: value } : item)); }
  function openEditor(f?: Flow, mode: 'draft' | 'result' = 'draft', imported?: RequestDraft) {
    if (!workspaceReady) { notify(t("请求工作区尚未加载，请稍后再试。")); return; }
    const id = crypto.randomUUID();
    setEditors(items => [...items, { id, draft: clone(imported ?? f?.request ?? blank), selected: mode === 'result' ? f?.id ?? null : null, parentId: f?.id ?? null }]);
    setActiveEditor(id); setView('replay'); setTab('headers'); setResponseTab('body'); setResponseCollapsed(false);
    return id;
  }
  function closeEditor(id: string) {
    void runs.cancel(id);
    if (editors.length === 1) { setEditors([]); setActiveEditor(''); setView('capture'); return; }
    const remaining = editors.filter(item => item.id !== id); setEditors(remaining);
    if (activeEditor === id) setActiveEditor(remaining[remaining.length - 1].id);
  }
  function editorTitle(url: string) { try { return new URL(url).host; } catch { return t("未命名"); } }


  const [tab, setTab] = useState('headers');
  const [responseTab, setResponseTab] = useState('body');
  const [responseCollapsed, setResponseCollapsed] = useState(false);
  const [view, setView] = useState('capture');
  const [status, setStatus] = useState<ProxyStatus>({ running: false, address: null, caPath: '', browserReplay: desktop });
  const [, refreshRuns] = useState(0);
  const runsRef = useRef<RequestRuns | null>(null);
  if (!runsRef.current) runsRef.current = new RequestRuns(invoke, () => refreshRuns(n => n + 1), text => setNotice(text));
  const runs = runsRef.current;
  const activeRun = runs.state(editor?.id ?? '');
  const busy = !!activeRun;
  const [proxyBusy, setProxyBusy] = useState(false);
  const [connected, setConnected] = useState(desktop);
  const [notice, setNotice] = useState('');
  const [modal, setModal] = useState<'settings' | 'help' | null>(null);
  const [port, setPort] = useState(8080);
  const [host, setHost] = useState('127.0.0.1');
  const listenAddress = status.running && status.address ? status.address : `${host.includes(':') ? '[' + host + ']' : host}:${port}`;
  const wildcard = listenAddress.startsWith('0.0.0.0:') || listenAddress.startsWith('[::]:');
  const clientAddress = wildcard ? t("本机局域网 IP:{v0}", { v0: listenAddress.slice(listenAddress.lastIndexOf(':') + 1) }) : listenAddress;
  useEffect(() => {
    if (!status.running || !status.address) return;
    const split = status.address.lastIndexOf(':');
    setHost(status.address.slice(0, split).replace(/^\[|\]$/g, ''));
    setPort(Number(status.address.slice(split + 1)));
  }, [status.running, status.address]);
  const [bodyMode, setBodyMode] = useState('text');
  const [curlText, setCurlText] = useState<string | null>(null);
  const [curlError, setCurlError] = useState('');
  const [readingClipboard, setReadingClipboard] = useState(false);
  async function openCurlImport() {
    setReadingClipboard(true);
    setCurlError('');
    let initialText = '';
    try {
      const text = await navigator.clipboard.readText();
      if (looksLikeCurl(text)) initialText = text;
    } catch { notify(t("无法读取剪贴板，请在导入窗口中手动粘贴 cURL。")); }
    finally { setReadingClipboard(false); }
    setCurlText(initialText);
  }
  function confirmCurl() {
    try { const request = importCurl(curlText ?? ''); openEditor(undefined, 'draft', request); setCurlText(null); }
    catch (error) { setCurlError(error instanceof Error ? error.message : String(error)); }
  }
  const flow = flows.find(f => f.id === selected);
  const engines = status.sendEngines ?? [];
  const selectedEngine = engines.find(e => e.id === (draft.engine ?? 'auto'));
  function merge(f: Flow) { setFlows(previous => [f, ...previous.filter(x => x.id !== f.id)]); }
  function notify(text: string) { setNotice(text); }
  useEffect(() => { if (!notice) return; const timer = setTimeout(() => setNotice(''), 5000); return () => clearTimeout(timer); }, [notice]);
  useEffect(() => {
    let disposed = false;
    const subscriptions = desktop ? [
      listen<Flow>('flow-recorded', () => { void invoke<Flow[]>('list_flows').then(v => { if (!disposed) setFlows(v); }).catch(() => {}); }),
      listen('flows-refresh', () => { void invoke<Flow[]>('list_flows').then(v => { if (!disposed) setFlows(v); }); }),
    ] : [];
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const [list, state] = await Promise.all([invoke<Flow[]>('list_flows'), invoke<ProxyStatus>('proxy_status')]);
        const tunState = await invoke<TunStatus>('tun_status');
        if (!disposed) setTun(tunState);
        const interceptState = await invoke<InterceptSnapshot>('interception');
        if (!disposed) setInterception(interceptState);
        if (!disposed) { setFlows(list); setStatus(state); setConnected(true); }
      } catch { if (!disposed) { setConnected(false); setStatus(s => ({ ...s, running: false })); } }
      finally { if (!disposed) timer = setTimeout(() => void refresh(), 1500); }
    }
    void refresh();
    return () => { disposed = true; clearTimeout(timer); subscriptions.forEach(p => { void p.then(unlisten => unlisten()); }); };
  }, []);

  async function capture() {
    if (proxyBusy) return;
    if (!connected) { notify(t("抓包服务未连接，请运行 npm run capture。")); return; }
    setProxyBusy(true);
    try { await invoke(status.running ? 'stop_proxy' : 'start_proxy', { port, host: host.trim() }); setStatus(await invoke<ProxyStatus>('proxy_status')); }
    catch (e) { notify(String(e)); } finally { setProxyBusy(false); }
  }
  async function sendRequest(id: string, request: RequestDraft, parentId: string | null) {
    const snapshot = clone(request);
    const serialized = JSON.stringify(snapshot);
    await runs.start(id, snapshot, parentId, result => {
      merge(result);
      setEditors(items => applyRequestResult(items, id, serialized, result));
      if (result.error) notify(result.error);
    });
  }
  async function send() {
    if (busy || !editor) return;
    try { if (!['http:', 'https:'].includes(new URL(draft.url).protocol)) throw new Error(); }
    catch { notify(t("请输入有效的 HTTP 或 HTTPS URL。")); return; }
    if (!connected) { notify(t("抓包服务未连接，请运行 npm run capture。")); return; }
    if (!selectedEngine?.available) { notify(selectedEngine?.reason ?? t("所选发送引擎不可用，请刷新内核连接。")); return; }
    setSelected(null);
    await sendRequest(editor.id, draft, selected ?? editor.parentId);
  }
  async function replayCaptured(f: Flow) {
    if (!connected) { notify(t("抓包服务未连接。")); return; }
    const request = clone(f.request);
    request.engine ??= 'auto';
    const engine = engines.find(e => e.id === request.engine);
    if (!engine?.available) { notify(engine?.reason ?? t("发送引擎不可用")); return; }
    const id = openEditor(f);
    if (id) await sendRequest(id, request, f.id);
  }
  async function copy(value: string) { try { await navigator.clipboard.writeText(value); notify(t("已复制到剪贴板")); } catch { notify(t("当前环境无法访问剪贴板")); } }
  async function exportCa() {
    try {
      const pem = await invoke<string>('export_certificate');
      const url = URL.createObjectURL(new Blob([pem], { type: 'application/x-pem-file' }));
      const link = document.createElement('a'); link.href = url; link.download = 'http-capture-ca.pem'; link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      notify(t("已导出 CA 公钥证书；请在测试客户端中信任该证书。"));
    } catch (e) { notify(String(e)); }
  }
  const responseText = flow?.response ? decodeText(flow.response.bodyBase64) : '';

  return <div className="app-shell">
    <div className="workspace">
      <header className="proxy-toolbar"><div className="proxy-address"><span className={status.running ? 'dot green' : 'dot'} />
        <span>{!connected ? t("内核未连接") : status.running ? 'Proxying on' : t("代理已停止")} {connected && listenAddress}</span>
        <button className="icon-button" title={wildcard ? t("所有网卡监听：客户端请填写本机局域网 IP 和端口") : t("复制代理地址")} disabled={wildcard} onClick={() => void copy(listenAddress)}><Copy size={17} /></button>
        <button className="text-button" onClick={() => setView('interception')}>{t("拦截 ·")}{interception.items.length}{interception.config.request || interception.config.response ? t(" · 已开启") : ''}</button><button className="text-button" title={t("上游代理设置")} onClick={() => setModal('settings')}>{status.upstream?.enabled ? t("上游：{v0}", { v0: status.upstream.profileName ?? status.upstream.url }) : t("上游：直连")}</button><button className="proxy-settings-button" aria-label={t("代理设置")} title={t("修改监听端口、配置上游代理")} onClick={() => setModal('settings')}><Settings2 size={18} /><span>{t("代理设置")}</span></button>
        <button className="text-button" onClick={() => setModal('settings')}>TUN · {tun.running ? t("运行中") : t("未开启")}</button><span className="toolbar-spacer" /><LanguageSelector /><label className="theme-picker">{t("主题")}<select aria-label={t("界面主题")} value={theme} onChange={e => { const value = e.target.value as Theme; setTheme(value); applyTheme(value); }}><option value="light">{t("浅色 · 琥珀")}</option><option value="dark">{t("深色 · 薄荷")}</option></select></label><span className="app-wordmark">HTTP CAPTURE</span>
        <button className="icon-button" title={t("导出 CA 证书")} disabled={!connected} onClick={() => void exportCa()}><ShieldCheck size={21} /></button>
        <button className="icon-button" title={t("使用帮助")} onClick={() => setModal('help')}><CircleHelp size={19} /></button>
      </div><button className="capture-switch" disabled={proxyBusy} onClick={() => void capture()}>{status.running ? <Square size={16} /> : <Play size={16} />}{proxyBusy ? t("处理中…") : status.running ? t("停止") : t("开始捕获")}</button></header>
      <nav className="work-tabs" aria-label={t("工作区标签")}>
        <button className={view === 'capture' ? 'work-tab active' : 'work-tab'} onClick={() => setView('capture')}><Radio size={17} />{t("调试")}<span className="tab-count">({flows.filter(f => f.source === 'capture').length})</span></button>
        {editors.map(item => <div key={item.id} className={view === 'replay' && activeEditor === item.id ? 'work-tab active' : 'work-tab'}>
          <button onClick={() => { setActiveEditor(item.id); setView('replay'); }} title={item.draft.url}><Send size={16} />{editorTitle(item.draft.url)}{runs.state(item.id) ? <span>{runs.state(item.id)?.cancelling ? t("取消中…") : t("发送中…")}</span> : <span className="dot green" />}</button>
          <button className="close-tab" aria-label={(t("关闭 ") + editorTitle(item.draft.url) + t(" 标签"))} onClick={() => closeEditor(item.id)}><X size={13} /></button>
        </div>)}
        <button className="new-tab" title={t("新建请求")} aria-label={t("新建请求")} disabled={!workspaceReady} onClick={() => openEditor()}><Plus size={21} /></button>
        <button className="curl-import-button" disabled={readingClipboard || !workspaceReady} onClick={() => void openCurlImport()}>{t("导入 cURL")}</button>
        <details className="request-history"><summary>{t("重放历史 (")}{flows.filter(f => f.source === 'replay').length})</summary><div>{flows.filter(f => f.source === 'replay').map(f => <button key={f.id} onClick={event => { openEditor(f, 'result'); event.currentTarget.closest('details')?.removeAttribute('open'); }}>{f.request.method} {f.request.url} <span>{f.response?.status ?? 'ERR'}</span></button>)}</div></details>
      </nav>
      <div className="workspace-save-status" role="status"><span>{t(workspaceMessage)}</span>{workspaceError && workspaceReady && <button onClick={() => void saveWorkspace()}>{t("重试保存")}</button>}</div>
      {!connected && <div className="preview-strip"><span><Layers3 size={14} />{t("界面预览 · 尚未连接抓包内核")}</span><span>{t("仅显示实际捕获的请求")}</span></div>}

      {interception.items.length > 0 && view !== 'interception' && <button className="preview-strip" onClick={() => setView('interception')}>{t("有")}{interception.items.length} {t("项等待拦截放行，点击处理")}</button>}
      {view === 'interception' && <Interception snapshot={interception} onUpdate={setInterception} />}
      <div className="capture-page" hidden={view !== 'capture'}><CaptureView onFlowsChanged={setFlows} active={view === 'capture'} busy={false} onReplayNow={f => void replayCaptured(f)} flows={flows} desktop={connected} running={status.running} address={clientAddress} onCopy={value => void copy(value)} onReplay={f => openEditor(f)} /></div>
      <div className="composer-page" hidden={view !== 'replay'}>
        <section className="detail-panel panel">
          <div className="request-bar"><select aria-label={t("请求方法")} value={draft.method} onChange={e => setDraft({ ...draft, method: e.target.value })}>{['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'].map(m => <option key={m}>{m}</option>)}</select><div className="url-field"><LockKeyhole size={14} /><input aria-label={t("请求 URL")} value={draft.url} onChange={e => setDraft({ ...draft, url: e.target.value })} /></div><button className="request-tls" title={t("当前 TLS 预设，点击设置；实际握手版本见响应栏")} onClick={() => setTab('tls')}>TLS · {draft.tls.preset}</button><button className="button primary replay" disabled={busy} onClick={() => void send()}><Send size={14} />{busy ? t("处理中…") : t("发送请求")}</button>{busy && <button className="button" disabled={activeRun?.cancelling} onClick={() => editor && void runs.cancel(editor.id)}>{activeRun?.cancelling ? t("取消中…") : t("取消请求")}</button>}</div>
          <SplitPane collapsedSecond={responseCollapsed} storageKey="composer-columns" label={t("调整请求与响应宽度")} direction="row" initial={50} min={25} max={75}><div className="replay-request">          <div className="tabs">{[['overview', t("总览"), null], ['headers', t("请求头"), draft.headers.length], ['body', t("请求体"), null], ['tls', 'TLS', null], ['order', 'Header Order', null], ['scripts', t("脚本"), null], ['settings', t("设置"), null]].map(([key, label, count]) => <button className={tab === key ? 'tab active' : 'tab'} key={key} onClick={() => setTab(String(key))}>{label}{count !== null && <span>{count}</span>}{key === 'tls' && <span className="tiny-dot" />}</button>)}<div className="tabs-right"><ShieldCheck size={13} />{t("有序字段")}</div></div>

          <div className="request-content">
          {tab === 'overview' && <RequestOverview request={draft} flow={flow} />}
          {tab === 'scripts' && <ScriptsEditor key={editor?.id} value={draft.scripts ?? emptyScripts} onChange={scripts=>setDraft({...draft,scripts})} logs={[...(flow?.notes??[]),flow?.error??'']} />}
          {tab === 'settings' && <div className="tls-editor"><EngineSelector draft={draft} engines={engines} onChange={setDraft} /><RequestProxySelector draft={draft} onChange={setDraft} onManage={() => setModal('settings')} /></div>}
          {tab === 'headers' && <OrderedHeaders headers={draft.headers} onChange={headers => setDraft({...draft,headers})} />}
          {tab === 'order' && <HeaderOrder onCopy={value => void copy(value)} headers={[...(draft.pseudoHeaders ?? []), ...draft.headers]} sent={flow?.response?.sentRequestHeaders} />}
          {tab === 'body' && <div className="body-editor"><div className="editor-toolbar"><span>{t("请求正文")}</span><select aria-label={t("正文编码")} value={bodyMode} onChange={e => setBodyMode(e.target.value)}><option value="text">UTF-8</option><option value="base64">Base64</option></select></div><textarea spellCheck={false} aria-label={t("请求正文")} placeholder={t("此请求尚无正文")} value={bodyMode === 'base64' ? draft.bodyBase64 : decodeText(draft.bodyBase64)} onChange={e => setDraft({ ...draft, bodyBase64: bodyMode === 'base64' ? e.target.value : encodeText(e.target.value) })} /></div>}
          {tab === 'tls' && <div className="tls-editor"><EngineSelector draft={draft} engines={engines} onChange={setDraft} /><TlsDetails flow={flow} /><div className="tls-intro"><ShieldCheck size={21} /><div><strong>{t("ClientHello 配置")}</strong><p>{t("选择浏览器预设，或调整握手参数。每次重放使用新连接。")}</p></div></div><div className="preset-grid">{[['native', 'Native', draft.engine === 'auto' ? t("Auto · h2 / h1 协商") : draft.engine === 'h2' ? t("HTTP/2 · 默认 TLS") : draft.engine === 'h3' ? t("HTTP/3 · 默认 TLS") : t("HTTP/1.1 · 原生保序")], ['chrome', 'Chrome', t("TLS + HTTP/2 预设")], ['firefox', 'Firefox', t("TLS + HTTP/2 预设")]].map(([key, label, desc]) => <button disabled={!selectedEngine?.available || !selectedEngine.profiles.includes(key)} title={!selectedEngine?.profiles.includes(key) ? t("当前发送引擎不支持此预设") : undefined} className={draft.tls.preset === key ? 'preset active' : 'preset'} key={key} onClick={() => setDraft({ ...draft, tls: { preset: key } })}><Globe2 size={16} /><strong>{label}</strong><small>{desc}</small>{draft.tls.preset === key && <Check size={13} />}</button>)}</div>{draft.tls.preset !== 'native' && <label className="setting-label">{t('浏览器 TLS 版本')}<select aria-label={t('浏览器 TLS 版本')} value={draft.tls.browserVersion ?? 'auto'} onChange={e => setDraft({ ...draft, tls: { ...draft.tls, browserVersion: e.target.value === 'auto' ? null : e.target.value } })}><option value="auto">{t('跟随 User-Agent（默认）')}</option><option value="latest">{t('使用库支持的最新版本')}</option>{(selectedEngine?.browserVersions?.[draft.tls.preset] ?? []).map(version => <option key={version} value={String(version)}>{version}</option>)}{draft.tls.browserVersion && !['auto','latest'].includes(draft.tls.browserVersion) && !(selectedEngine?.browserVersions?.[draft.tls.preset] ?? []).includes(Number(draft.tls.browserVersion)) && <option value={draft.tls.browserVersion}>{draft.tls.browserVersion} · {t('当前构建不支持')}</option>}</select><small>{t('自动模式读取当前浏览器类型的 UA 版本；无对应 UA 时使用库内最新版本。不存在的版本会报错，请手动选择。此设置不修改请求头。')}</small></label>}<div className="tls-fields"><label>{t('TLS 版本')}<select aria-label={t('TLS 版本')} disabled={draft.tls.preset !== 'native'} value={draft.tls.version ?? 'auto'} onChange={e => setDraft({ ...draft, tls: { ...draft.tls, version: e.target.value === 'auto' ? null : e.target.value } })}><option value="auto">{t('自动（TLS 1.3 / 1.2）')}</option><option value="1.2" disabled={draft.engine === 'h3'}>TLS 1.2</option><option value="1.3">TLS 1.3</option></select></label><p>{t('原生引擎支持 TLS 版本、密码套件和密钥交换组。列表用冒号分隔，留空使用默认配置；ALPN 跟随发送引擎。HTTP/3 仅支持 TLS 1.3。')}</p><p>{t('密码套件使用 IANA 名称，按输入顺序提供。原生引擎不模拟浏览器指纹，不支持自定义签名算法、GREASE 或扩展排列。')}</p>{[['cipherList', 'Cipher suites', t("继承预设 · BoringSSL cipher list")], ['sigalgsList', 'Signature algorithms', t("继承预设")], ['curvesList', 'Supported groups', t("继承预设")]].map(([key, label, placeholder]) => <label key={key}>{label}<input disabled={draft.tls.preset === 'native' && key === 'sigalgsList'} placeholder={draft.tls.preset === 'native' ? key === 'cipherList' ? 'TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384' : 'X25519:secp256r1:secp384r1' : placeholder} value={String(draft.tls[key as keyof typeof draft.tls] ?? '')} onChange={e => setDraft({ ...draft, tls: { ...draft.tls, [key]: e.target.value || null } })} /></label>)}<div className="tls-toggles">{[['grease', 'GREASE'], ['permuteExtensions', t("扩展随机排列")]].map(([key, label]) => <label key={key}>{label}<select disabled={draft.tls.preset === 'native'} value={draft.tls[key as 'grease'] == null ? 'default' : String(draft.tls[key as 'grease'])} onChange={e => setDraft({ ...draft, tls: { ...draft.tls, [key]: e.target.value === 'default' ? null : e.target.value === 'true' } })}><option value="default">{t("继承预设")}</option><option value="true">{t("开启")}</option><option value="false">{t("关闭")}</option></select></label>)}</div></div></div>}
          </div>

          </div><div className="replay-response"><div className="response-heading"><button className="response-collapse" aria-expanded={!responseCollapsed} aria-label={responseCollapsed ? t("展开响应面板") : t("收起响应面板")} onClick={() => setResponseCollapsed(value => !value)}><ChevronDown size={15} style={{ transform: responseCollapsed ? 'rotate(-90deg)' : undefined }} />{t("响应 ·")}{responseCollapsed ? t("展开") : t("收起")}</button>{flow?.response && <div className="response-meta"><span className="response-protocol" title={t("代理到上游服务器的实际 TLS 版本")}>{flow.response.tlsVersion ?? (flow.request.url.startsWith('https:') ? t("TLS 未记录") : t("明文"))}</span><span className="response-protocol" aria-label={t("响应协议")} title={t("本次上游响应实际使用的协议")}>{flow.response.version}</span><span className={(flow.response.status >= 400 ? 'bad' : 'success')}>{flow.response.status} {flow.response.status === 200 ? 'OK' : ''}</span><span>{flow.durationMs} ms</span><span>{(flow.response.bodyBase64.length * 0.75 / 1024).toFixed(2)} KB</span></div>}</div>
          <div className="tabs response-tabs">{[['body', t("响应体")], ['headers', t("响应头")], ['notes', t("发送说明")]].map(([key, label]) => <button className={responseTab === key ? 'tab active' : 'tab'} key={key} onClick={() => setResponseTab(key)}>{label}</button>)}<div className="tabs-right"><FileJson size={13} />{responseTab === 'body' ? 'RESPONSE' : 'DETAILS'}<button hidden={responseTab === 'body'} className="icon-button" title={t("复制响应")} onClick={() => void copy(responseText)}><Copy size={13} /></button></div></div>
          <div className="response-content">{flow?.error ? <div className="error-box">{flow.error}</div> : responseTab === 'body' ? flow?.response ? <ResponseBody key={flow.id} body={flow.response.bodyBase64} headers={flow.response.headers} onCopy={value => void copy(value)} /> : <div className="empty"><Code2 size={27} /><p>{busy ? t("正在发送；可以切换标签或继续编辑，改动将在下次发送时生效。") : t("发送请求后在这里查看响应")}</p></div> : responseTab === 'headers' ? <div className="response-header-list">{flow?.response?.headers.map((h, i) => <div key={i}><span>{h.name}</span><code>{h.value}</code></div>)}</div> : <div className="notes">{(flow?.notes ?? [t("尚未发送。")]).map((note, i) => <p key={i}><CircleHelp size={14} />{note}</p>)}</div>}</div>
          </div></SplitPane>
        </section>
      </div>
      <footer className="statusbar"><span><span className={status.running ? 'dot green' : 'dot'} />{connected ? status.running ? t("代理已就绪") : t("等待启动代理") : t("未连接内核")}<span className="footer-divider" />{flows.filter(f => f.source === 'capture').length} {t("个捕获请求")}</span><span><LockKeyhole size={11} />{t("本地工作空间")}</span></footer>
    </div>
    {notice && <div className="toast" role="status"><CircleHelp size={17} /><span>{notice}</span><button onClick={() => setNotice('')} aria-label={t("关闭提示")}><X size={15} /></button></div>}
    {curlText !== null && <div className="modal-backdrop" onKeyDown={e => { if (e.key === 'Escape') setCurlText(null); }}><div className="modal curl-import-modal" role="dialog" aria-modal="true" aria-label={t("导入 cURL")}><button className="modal-close icon-button" aria-label={t("关闭导入")} onClick={() => setCurlText(null)}><X size={19} /></button><h2>{t("导入 cURL")}</h2><p>{t("可从剪贴板导入请求，或粘贴命令。导入后可编辑，点击发送才会发出请求。")}</p><textarea autoFocus aria-label={t("cURL 命令")} placeholder="curl 'https://example.com' -H 'Accept: application/json'" value={curlText} onChange={e => { setCurlText(e.target.value); setCurlError(''); }} />{curlError && <p role="alert">{curlError}</p>}<div className="curl-import-actions"><button className="button" onClick={() => { setCurlText(null); openEditor(); }}>{t("创建空请求")}</button><button className="button primary" disabled={!curlText.trim()} onClick={confirmCurl}>{t("导入请求")}</button></div></div></div>}
    {modal && <div className="modal-backdrop" onClick={() => setModal(null)}><div className="modal" role="dialog" aria-modal="true" aria-label={modal === 'settings' ? t("代理设置") : t("使用帮助")} onClick={e => e.stopPropagation()}><button className="modal-close icon-button" aria-label={t("关闭")} onClick={() => setModal(null)}><X size={19} /></button><div className="modal-symbol">{modal === 'settings' ? <Settings2 /> : <Terminal />}</div><h2>{modal === 'settings' ? t("代理设置") : t("开始使用 HTTP Capture")}</h2>{modal === 'settings' ? <><p>{t("停止代理后可以修改监听地址和端口。局域网模式允许其他设备使用此抓包代理。")}</p><LanguageSelector /><ListenerSettings host={host} port={port} running={status.running} onHost={setHost} onPort={setPort} /><TunSettings status={tun} onUpdate={setTun} /><UpstreamSettings current={status.upstream} connected={connected} onSaved={async () => setStatus(await invoke<ProxyStatus>('proxy_status'))} /><div className="setting-card"><ShieldCheck size={18} /><div><strong>{t("本地 CA 证书")}</strong><p>{status.caPath || t("启动桌面端后生成，每台设备独立保存。")}</p></div></div><button className="button" disabled={!connected} onClick={() => void exportCa()}>{t("导出 CA 证书")}</button><p className="muted">{t("客户端 HTTP 和 HTTPS 代理地址：")}{clientAddress}{t("。HTTPS 解密需要在客户端设备上信任上方 CA。")}{wildcard && t("0.0.0.0 / :: 是监听地址，不要填入客户端代理设置。")}{host !== '127.0.0.1' && t("请在可信局域网使用，并按需允许防火墙入站；当前抓包入口不提供代理认证。")}{t("不会自动修改防火墙、系统代理或证书信任。")}</p></> : <><p>{desktop ? t("启动捕获后，将客户端 HTTP / HTTPS 代理设置为下方地址。") : t("主页面只列出实际捕获的请求。当前预览未连接内核，列表为空。")}</p><div className="command">{clientAddress}</div><ol><li>{t("点击请求，查看请求头和响应。")}</li><li>{t("点击“发送到重放”创建可编辑的请求副本。")}</li><li>{t("在重放页编辑请求头，配置 TLS 并发送。")}</li><li>{t("真实抓包需通过")}<code>npm run desktop</code> {t("启动。")}</li></ol></>}<button className="button primary modal-done" onClick={() => setModal(null)}>{t("完成")}</button></div></div>}
  </div>;
}
