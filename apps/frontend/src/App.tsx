import Plugins from './Plugins';
import McpPage from './McpPage';
import ConfigMigration, {type ConfigImportResult} from './ConfigMigration';
import {acknowledgeConfigurationImport} from './configImportRecovery';
import type {ConfigurationImportStatus} from './configImportRecovery';
import AppUpdater from './AppUpdater';
import './settings-layout.css';
import SettingsTabs, { type SettingsPageId } from './SettingsTabs';
import Certificates from './Certificates';
import {isGrpcContentType} from './grpc';
import GrpcResponse from './GrpcResponse';
import GrpcPanel from './GrpcPanel';
import { FileProgress } from './FileProgress';
import {chooseSave,saveText} from './saveFile';
import { useDesktopClose } from './useDesktopClose';
import ClientHelloEditor from "./ClientHelloEditor";
import WebSocketFrames from './WebSocketFrames';
import DecryptionSettings from './DecryptionSettings';
import { readClipboard, writeClipboard } from './clipboard';
import RequestBodyEditor from './RequestBodyEditor';
import {createHistoryRefresh, flowBodyLength, isFlowMetadata, loadCompleteFlow, loadFlowRecord} from './flowRecords';
import RequestLibrary from './RequestLibrary';
import SaveRequestDialog from './SaveRequestDialog';
import ExportCodeDialog from './ExportCodeDialog';
import { emptyLibrary, resolveEnvironment, suggestedRequestName, withSavedRequest, type Library, type SaveRequestSelection, type SavedRequestContext } from './library';
import { t, useLanguage } from './i18n';
import LanguageSelector from './LanguageSelector';
import TunSettings, { type TunStatus } from './TunSettings';
import ListenerSettings, { type ListenerSettingsHandle } from './ListenerSettings';
import { readListenerPreferences, saveListenerPreferences } from './listenerPreferences';
import SystemSettings from './SystemSettings';
import SystemProxyToggle from './SystemProxyToggle';
import { applyRequestResult, RequestRuns } from './requestRuns';
import ScriptsEditor, { emptyScripts } from './ScriptsEditor';
import OrderedHeaders from './OrderedHeaders';
import { type BlockConfig } from './RequestBlocking';
import RequestPolicies from './RequestPolicies';
import Interception, { type InterceptSnapshot } from './Interception';
import ResponseRewrite from './ResponseRewrite';
import { useEffect, useRef, useState } from 'react';
import { Activity, ArrowDown, ArrowUp, ArrowUpRight, Ban, Check, ChevronDown, CircleHelp, Code2, Copy, FileJson, FolderOpen, Globe2, Layers3, LockKeyhole, Play, Plus, Puzzle, Radio, RotateCw, Save, Search, Send, Settings2, ShieldCheck, Square, Terminal, Trash2, X } from 'lucide-react';
import { isTauri } from '@tauri-apps/api/core';
import { invoke } from './api';
import CaptureView from './CaptureView';
import ResponseBody from './ResponseBody';
import RecordedBody from './RecordedBody';
import HeaderOrder from './HeaderOrder';
import { RequestOverview, TlsDetails } from './RequestDetails';
import EngineSelector from './EngineSelector';
import RequestProxySelector from './RequestProxySelector';
import UpstreamSettings from './UpstreamSettings';
import { useRequestWorkspace, type RequestEditor } from './useRequestWorkspace';
import SplitPane from './SplitPane';
import { emptyRequest, importCurl, looksLikeCurl } from './importCurl';
import { applyTheme, readTheme } from './theme';
import ThemeSelector from './ThemeSelector';
import { readUiZoom } from './uiZoom';
import { useUiZoom } from './useUiZoom';
import { decodeText, encodeText, type Flow, type FlowRecord, type ProxyStatus, type RequestDraft } from './types';

const desktop = isTauri();
const clone = <T,>(v: T): T => JSON.parse(JSON.stringify(v));
const blank: RequestDraft = emptyRequest();
type RequestSaveTarget = SavedRequestContext & { editorId: string; draft: RequestDraft; name: string; collectionId?: string };

export default function App({initialImportStatus}:{initialImportStatus?:ConfigurationImportStatus}={}) {
  useLanguage();
  const [interception, setInterception] = useState<InterceptSnapshot>({config:{request:false,response:false,scope:'all'},items:[]});
  const [rewriteSeed, setRewriteSeed] = useState<{key:string;flow:FlowRecord;action:'replace'|'modify';responseBodyBase64?:string}>();
  function rewriteResponse(flow:FlowRecord,action:'replace'|'modify',responseBodyBase64?:string) {
    setRewriteSeed({key:crypto.randomUUID(),flow,action,responseBodyBase64});
    setView('rewrite');
  }
  async function interceptResponse(flow: FlowRecord) {
    try {
      const latest = await invoke<InterceptSnapshot>('interception');
      const url = flow.request.url.split('#')[0];
      const urlContains = '^' + url.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '$';
      const rules = [...(latest.config.rules ?? [])];
      const existing = rules.findIndex(rule => !rule.exclude && rule.urlRegex && rule.urlContains === urlContains && rule.method === flow.request.method && !rule.host && rule.status === null && !rule.headerName);
      if (existing >= 0) rules[existing] = { ...rules[existing], disabled: false };
      else {
        if (rules.length >= 32) throw new Error(t('最多支持 32 条规则'));
        rules.push({ urlContains, urlRegex: true, method: flow.request.method, host: '', status: null, headerName: '', headerContains: '' });
      }
      await invoke('configure_interception', { config: { ...latest.config, response: true, scope: 'all', rules } });
      setInterception(await invoke<InterceptSnapshot>('interception'));
      setView('interception');
      notify(t('已开启此 URL 和方法的响应拦截，请重新发起请求；已有排除规则仍优先生效。'));
    } catch (error) { notify(String(error)); }
  }
  const [tun, setTun] = useState<TunStatus>({available:false,running:false,helperPath:''});
  const [zoom,setZoom]=useState(readUiZoom);
  useUiZoom(zoom);
  const [theme, setTheme] = useState(readTheme);
  const flowRefreshEpoch = useRef(0);
  const historyGeneration = useRef(0);
  function replaceFlows(rows: FlowRecord[]) { flowRefreshEpoch.current++; historyGeneration.current++; setFlows(rows); }
  const [library,applyLibrary]=useState<Library>(emptyLibrary);
  const libraryLatest=useRef(emptyLibrary);
  function setLibrary(next:Library){libraryLatest.current=next;applyLibrary(next)}
  const [libraryReady,setLibraryReady]=useState(false);
  const [libraryBusy,setLibraryBusy]=useState(false);
  const [libraryMessage,setLibraryMessage]=useState('');
  const [environmentId,setEnvironmentId]=useState('');
  const librarySaved=useRef(JSON.stringify(emptyLibrary));
  const libraryOperation = useRef(false);
  const [requestSaveTarget, setRequestSaveTarget] = useState<RequestSaveTarget | null>(null);
  const [requestSaveError, setRequestSaveError] = useState('');
  const [codeExportRequest, setCodeExportRequest] = useState<RequestDraft | null>(null);
  useEffect(()=>{const leave=(e:BeforeUnloadEvent)=>{if(libraryReady&&JSON.stringify(library)!==librarySaved.current){e.preventDefault();e.returnValue='';}};window.addEventListener('beforeunload',leave);return()=>window.removeEventListener('beforeunload',leave);},[library,libraryReady]);
  async function loadLibrary() {
    if (libraryOperation.current) return;
    libraryOperation.current = true; setLibraryBusy(true);
    try {
      const loaded = await invoke<Library>('request_library');
      setLibrary(loaded); librarySaved.current = JSON.stringify(loaded);
      setLibraryReady(true); setLibraryMessage('');
      return loaded;
    } catch (e) { setLibraryMessage(String(e)); }
    finally { libraryOperation.current = false; setLibraryBusy(false); }
  }
  async function persistLibrary(next: Library) {
    if (libraryOperation.current) throw new Error(t('正在保存，请稍后重试。'));
    libraryOperation.current = true; setLibraryBusy(true);
    try {
      const submitted = structuredClone(next);
      const revision = await invoke<number>('save_request_library', { library: submitted });
      const saved = { ...submitted, revision };
      librarySaved.current = JSON.stringify(saved); setLibrary(saved); setLibraryMessage(t('已保存'));
    } catch (e) { setLibraryMessage(String(e)); throw e; }
    finally { libraryOperation.current = false; setLibraryBusy(false); }
  }
  async function saveLibrary() {
    if (!libraryReady || libraryOperation.current) return;
    try { await persistLibrary(library); } catch { /* The collection page keeps the error and local edits. */ }
  }
  const [flows, setFlows] = useState<FlowRecord[]>([]);
  const [configurationImported,setConfigurationImported]=useState<ConfigImportResult|null>(()=>initialImportStatus?.frozen?{epoch:initialImportStatus.epoch,importedSections:initialImportStatus.importedSections??[],preferences:initialImportStatus.preferences??{},warnings:initialImportStatus.warnings,requiresRestart:true,recoveryAvailable:true}:null);
  const { editors, setEditors, activeEditor, setActiveEditor, ready: workspaceReady, message: workspaceMessage, error: workspaceError, retry: saveWorkspace, conflict: workspaceConflict, recovering: workspaceRecovering, recover: recoverWorkspace, synchronize: synchronizeWorkspace } = useRequestWorkspace(!!configurationImported);
  const updateInProgress = useRef(false);
  const [configurationError,setConfigurationError]=useState('');
  const [configurationPreferencesApplied,setConfigurationPreferencesApplied]=useState(!!initialImportStatus?.frozen);
  const [configurationRestarting,setConfigurationRestarting]=useState(false);
  async function beforeConfigurationTransfer(){
    if(!workspaceReady||!libraryReady)throw Error(t('配置尚未读取完成，请稍后再试。'));
    if(libraryOperation.current||JSON.stringify(libraryLatest.current)!==librarySaved.current)throw Error(t('集合或环境有未保存的修改，请先在集合页保存，再迁移配置。'));
    if(!await saveWorkspace())throw Error(t('草稿保存失败，请合并恢复或保存成功后再迁移配置。'));
  }
  function finishConfigurationImport(result:ConfigImportResult){
    setConfigurationImported(result);setConfigurationError('');
    try{acknowledgeConfigurationImport(result);setConfigurationPreferencesApplied(true)}
    catch(e){setConfigurationPreferencesApplied(false);setConfigurationError(t('内核配置已导入，但界面偏好保存失败；请重试后再重启：{v0}',{v0:String(e)}))}
  }
  async function restartAfterConfigurationImport(){
    if(configurationRestarting||!configurationPreferencesApplied)return;
    setConfigurationRestarting(true);setConfigurationError('');
    try{await invoke('config_restart')}
    catch(e){setConfigurationError(String(e));setConfigurationRestarting(false)}
  }
  const closing = useDesktopClose(saveWorkspace, () => libraryReady && (libraryBusy || JSON.stringify(library) !== librarySaved.current), message => setNotice(message), () => updateInProgress.current);
  async function beforeUpdateInstall() {
    if (!workspaceReady) throw new Error(t('请求工作区尚未读取完成，请稍后再更新。'));
    if (libraryReady && (libraryBusy || JSON.stringify(library) !== librarySaved.current)) {
      throw new Error(t('集合或环境有未保存的修改，请保存后再更新。'));
    }
    if (!await saveWorkspace()) throw new Error(t('请求保存失败，请保存成功后再更新。'));
  }
  const editor = editors.find(item => item.id === activeEditor) ?? editors[0];
  const draft = editor?.draft ?? blank;
  const savedRequest = library.requests.find(request => request.id === editor?.savedRequestId);
  const savedCollection = library.collections.find(collection => collection.id === savedRequest?.collectionId);
  const closedEditors=useRef(new Set<string>());
  const [wsConnecting,setWsConnecting]=useState<Record<string,boolean>>({});
  const [wsDrafts,setWsDrafts]=useState<Record<string,{body:string;opcode:string}>>({});
  const [wsSessions,setWsSessions]=useState<Record<string,string>>({});
  const selected = editor?.selected ?? null;
  function setDraft(value: RequestDraft) { setEditors(items => items.map(item => item.id === editor?.id ? { ...item, draft: value } : item)); }
  function setSelected(value: string | null) { setEditors(items => items.map(item => item.id === editor?.id ? { ...item, selected: value } : item)); }
  function openEditor(f?: Flow, mode: 'draft' | 'result' = 'draft', imported?: RequestDraft, savedRequest?: Library['requests'][number]) {
    if (!workspaceReady) { notify(t("请求工作区尚未加载，请稍后再试。")); return; }
    const id = crypto.randomUUID();
    setEditors(items => [...items, { id, draft: clone(imported ?? (f?.websocket?{...f.request,url:f.request.url.replace(/^http/, 'ws'),method:'GET',bodyBase64:'',engine:'auto',tls:{preset:'native'}}:f?.request) ?? blank), selected: mode === 'result' ? f?.id ?? null : null, parentId: f?.id ?? null, ...(savedRequest ? { savedRequestId: savedRequest.id, ...(savedRequest.grpcDraft ? { grpcDraft: structuredClone(savedRequest.grpcDraft) } : {}) } : {}) }]);
    if (savedRequest?.requestMode) setRequestModes(values => ({ ...values, [id]: savedRequest.requestMode! }));
    setActiveEditor(id); setView('replay'); setTab('headers'); setResponseTab('body'); setResponseCollapsed(false);
    return id;
  }
  async function openRecordedEditor(record: FlowRecord, mode: 'draft' | 'result' = 'draft') {
    try { openEditor(await loadCompleteFlow(record), mode); }
    catch (error) { notify(String(error)); }
  }
  function closeEditor(id: string) {
    closedEditors.current.add(id);
    setWsDrafts(v=>{const next={...v};delete next[id];return next;});
    void runs.cancel(id);
    const selectedId=wsSessions[id]??editors.find(e=>e.id===id)?.selected;
    if(selectedId&&(wsSessions[id]||flows.find(f=>f.id===selectedId)?.websocket))void invoke('cancel_replay',{executionId:selectedId}).catch(e=>notify(String(e)));
    setWsSessions(v=>{const next={...v};delete next[id];return next;});
    if (editors.length === 1) { setEditors([]); setActiveEditor(''); setView('capture'); return; }
    const remaining = editors.filter(item => item.id !== id); setEditors(remaining);
    if (activeEditor === id) setActiveEditor(remaining[remaining.length - 1].id);
  }
  function editorTitle(url: string) { try { return new URL(url).host; } catch { return t("未命名"); } }
  function editorLabel(item: RequestEditor) {
    return library.requests.find(request => request.id === item.savedRequestId)?.name ?? editorTitle(item.draft.url);
  }
  function requestSaveFailure(error: unknown) {
    return /Library changed in another window/.test(String(error))
      ? t('集合已在其他窗口修改，请先在集合页导出当前修改，再重新加载后重试。')
      : t('保存失败：{v0}', { v0: String(error) });
  }
  async function commitRequest(target: RequestSaveTarget, selection: SaveRequestSelection, base: Library, requestId?: string) {
    const next = withSavedRequest(base, target.draft, selection, requestId, { grpcDraft: target.grpcDraft, requestMode: target.requestMode });
    await persistLibrary(next.library);
    setEditors(items => items.map(item => item.id === target.editorId ? { ...item, savedRequestId: next.requestId } : item));
    setRequestSaveError('');
    notify(t('已保存到「{v0}」，可从左侧集合再次打开。', { v0: next.collectionName }));
  }
  async function saveRequest(asCopy = false) {
    if (!editor || !connected || libraryOperation.current || requestSaveTarget) return;
    const source = structuredClone(editor);
    setRequestSaveError('');
    try {
      const base = libraryReady ? library : await loadLibrary();
      if (!base) throw new Error(t('请求集合读取失败，请重试。'));
      const existing = base.requests.find(request => request.id === source.savedRequestId);
      const target: RequestSaveTarget = {
        editorId: source.id, draft: source.draft, grpcDraft: source.grpcDraft, requestMode: currentRequestContext(source).requestMode,
        name: existing ? (asCopy ? t('{v0}（副本）', { v0: existing.name }) : existing.name) : suggestedRequestName(source.draft),
        collectionId: existing?.collectionId,
      };
      if (existing && !asCopy) {
        await commitRequest(target, { name: existing.name, collectionId: existing.collectionId }, base, existing.id);
      } else setRequestSaveTarget(target);
    } catch (error) { const message = requestSaveFailure(error); setRequestSaveError(message); notify(message); }
  }
  async function confirmRequestSave(selection: SaveRequestSelection) {
    if (!requestSaveTarget || libraryOperation.current) return;
    try {
      await commitRequest(requestSaveTarget, selection, library);
      setRequestSaveTarget(null);
    } catch (error) { setRequestSaveError(requestSaveFailure(error)); }
  }
  function openCodeExport() {
    if (!editor || requestMode !== 'http') return;
    try {
      const environment = library.environments.find(item => item.id === environmentId);
      if (environmentId && !environment) throw new Error(t('请选择环境'));
      setCodeExportRequest(resolveEnvironment(draft, environment?.variables ?? {}));
    } catch (error) { notify(String(error)); }
  }


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
  const [blocking, setBlocking] = useState<BlockConfig>({ enabled: false, scope: 'capture', rules: [] });
  const [blockingEditorVersion, setBlockingEditorVersion] = useState(0);
  const blockingActions = useRef<Promise<void>>(Promise.resolve());
  async function loadBlocking() { setBlocking(await invoke<BlockConfig>('blocking')); }
  useEffect(() => { if (connected) void loadBlocking().catch(() => {}); }, [connected]);
  function blockRequest(flow: FlowRecord, kind: 'url' | 'host') {
    const work = blockingActions.current.then(async () => {
    try {
      const current = await invoke<BlockConfig>('blocking');
      const url = new URL(flow.request.url);
      const matchType = kind === 'host' ? 'host' as const : 'urlRegex' as const;
      const pattern = kind === 'host' ? url.hostname : '^' + flow.request.url.split('#')[0].replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '$';
      const rules = [...current.rules];
      const existing = rules.findIndex(rule => rule.matchType === matchType && rule.pattern === pattern && !rule.method);
      if (existing >= 0) rules[existing] = { ...rules[existing], enabled: true };
      else {
        if (rules.length >= 100) throw new Error(t('最多支持 100 条阻止规则'));
        rules.push({ id: crypto.randomUUID(), enabled: true, matchType, pattern, method: '' });
      }
      await invoke('configure_blocking', { config: { ...current, enabled: true, rules } });
      await loadBlocking(); setBlockingEditorVersion(value => value + 1); setView('blocking');
      notify(kind === 'host' ? t('已添加域名阻止规则，后续匹配请求将在本地阻止。') : t('已添加 URL 阻止规则，后续匹配请求将在本地阻止。'));
    } catch (error) { notify(String(error)); }
    });
    blockingActions.current = work.catch(() => {});
    return work;
  }
  useEffect(()=>{if(connected&&!libraryReady)void loadLibrary();},[connected]);
  useEffect(()=>{
    if(!connected||!libraryReady||configurationImported)return;
    let disposed=false;
    let timer:ReturnType<typeof setTimeout>;
    async function synchronize(){
      try{
        const revisions=await invoke<{workspaceRevision:number;libraryRevision:number;configurationFrozen?:boolean}>('workspace_revisions');
        if(disposed)return;
        if(revisions.configurationFrozen){setConfigurationImported({importedSections:[],preferences:{},requiresRestart:true,recoveryAvailable:true});setConfigurationPreferencesApplied(true);return}
        await synchronizeWorkspace(revisions.workspaceRevision);
        const current=libraryLatest.current;
        if(!disposed&&!libraryOperation.current&&revisions.libraryRevision>(current.revision??0)){
          const before=JSON.stringify(current);
          if(before!==librarySaved.current){setLibraryMessage(t('集合已在其他窗口修改，请先在集合页导出当前修改，再重新加载后重试。'));return}
          const remote=await invoke<Library>('request_library');
          if(!disposed&&!libraryOperation.current&&JSON.stringify(libraryLatest.current)===before&&librarySaved.current===before){
            librarySaved.current=JSON.stringify(remote);setLibrary(remote);setLibraryMessage('');
          }
        }
      }catch{/* A disconnected core is handled by the existing connection state. */}
      finally{if(!disposed)timer=setTimeout(()=>void synchronize(),2000)}
    }
    void synchronize();
    return()=>{disposed=true;clearTimeout(timer)};
  },[connected,libraryReady,synchronizeWorkspace,configurationImported]);
  const [notice, setNotice] = useState('');
  const [modal, setModal] = useState<'settings' | 'help' | null>(null);
  const [settingsPage, setSettingsPage] = useState<SettingsPageId>('general');
  const settingsDialog = useRef<HTMLDivElement>(null);
  const [settingsValidationAttempt, setSettingsValidationAttempt] = useState(0);
  function openSettings(page: SettingsPageId = 'general') {
    setSettingsPage(page);
    setModal('settings');
  }
  useEffect(() => {
    if (settingsValidationAttempt) settingsDialog.current?.querySelector<HTMLInputElement>('.settings-listener-fields [aria-invalid="true"]')?.focus();
  }, [settingsValidationAttempt]);
  const [listenerPreferences, setListenerPreferences] = useState(readListenerPreferences);
  const { port, host } = listenerPreferences;
  const setPort = (port: number) => setListenerPreferences(value => ({ ...value, port }));
  const setHost = (host: string) => setListenerPreferences(value => ({ ...value, host }));
  useEffect(() => {
    if (!saveListenerPreferences(listenerPreferences)) notify(t('监听设置仅在本次会话生效，无法保存到本地。'));
  }, [listenerPreferences]);
  const listenerSettings = useRef<ListenerSettingsHandle>(null);
  async function finishModal() {
    if (modal !== 'settings') { setModal(null); return; }
    const current = listenerSettings.current;
    if (!current) return;
    const accepted = await current.commit();
    if (listenerSettings.current !== current) return;
    if (accepted) setModal(null);
    else { setSettingsPage('general'); setSettingsValidationAttempt(value => value + 1); }
  }
  const [useSystemProxy, setUseSystemProxy] = useState(false);
  const listenAddress = status.running && status.address ? status.address : `${host.includes(':') ? '[' + host + ']' : host}:${port}`;
  const wildcard = listenAddress.startsWith('0.0.0.0:') || listenAddress.startsWith('[::]:');
  const clientAddress = wildcard ? t("本机局域网 IP:{v0}", { v0: listenAddress.slice(listenAddress.lastIndexOf(':') + 1) }) : listenAddress;
  useEffect(() => {
    if (!status.running || !status.address) return;
    const split = status.address.lastIndexOf(':');
    setHost(status.address.slice(0, split).replace(/^\[|\]$/g, ''));
    setPort(Number(status.address.slice(split + 1)));
  }, [status.running, status.address]);
  const [curlText, setCurlText] = useState<string | null>(null);
  const [curlError, setCurlError] = useState('');
  const [readingClipboard, setReadingClipboard] = useState(false);
  async function openCurlImport() {
    setReadingClipboard(true);
    setCurlError('');
    let initialText = '';
    try {
      const text = await readClipboard();
      if (looksLikeCurl(text)) initialText = text;
    } catch { notify(t("无法读取剪贴板，请在导入窗口中手动粘贴 cURL。")); }
    finally { setReadingClipboard(false); }
    setCurlText(initialText);
  }
  function confirmCurl() {
    try { const request = importCurl(curlText ?? ''); openEditor(undefined, 'draft', request); setCurlText(null); }
    catch (error) { setCurlError(error instanceof Error ? error.message : String(error)); }
  }
  const flow = flows.find(f => f.id === (activeRun?.executionId ?? selected));
  const responseBodyLength=flow?flowBodyLength(flow,'response'):undefined;
  const [requestModes,setRequestModes]=useState<Record<string,string>>(()=>{try{return JSON.parse(localStorage.getItem('http-capture.request-modes')||'{}')}catch{return {}}});
  useEffect(()=>{if(!configurationImported)localStorage.setItem('http-capture.request-modes',JSON.stringify(requestModes))},[requestModes,configurationImported]);
  const requestMode=requestModes[editor?.id??'']??(/^wss?:/i.test(draft.url)?'websocket':draft.headers.some(h=>!h.disabled&&h.name.toLowerCase()==='content-type'&&isGrpcContentType(h.value))?'grpc':'http');
  const grpcRequest=requestMode==='grpc';
  const wsRequest=requestMode==='websocket';
  function currentRequestContext(item?: RequestEditor): SavedRequestContext {
    if (!item) return {};
    const mode = requestModes[item.id] ?? (/^wss?:/i.test(item.draft.url) ? 'websocket' : item.draft.headers.some(h => !h.disabled && h.name.toLowerCase() === 'content-type' && isGrpcContentType(h.value)) ? 'grpc' : 'http');
    return { grpcDraft: item.grpcDraft, requestMode: mode === 'grpc' || mode === 'websocket' ? mode : 'http' };
  }
  useEffect(()=>{setTab(requestMode==='grpc'?'grpc':'headers')},[requestMode,editor?.id]);
  function changeRequestMode(mode:string){
    if(!editor)return;
    setRequestModes(v=>({...v,[editor.id]:mode}));
    let url=draft.url;
    if(mode==='websocket')url=url.replace(/^http:/i,'ws:').replace(/^https:/i,'wss:');else url=url.replace(/^ws:/i,'http:').replace(/^wss:/i,'https:');
    setDraft({...draft,url,method:mode==='grpc'?'POST':mode==='websocket'?'GET':draft.method});
  }
  const wsActive=(!!flow?.websocket&&['connecting','open'].includes(flow.websocket.state))|| (!!wsSessions[editor?.id??'']&&!flows.some(f=>f.id===wsSessions[editor?.id??'']));
  const engines = status.sendEngines ?? [];
  const selectedEngine = engines.find(e => e.id === (draft.engine ?? 'auto'));
  function merge(f: Flow) {
    const epoch=++flowRefreshEpoch.current,generation=historyGeneration.current;
    void loadFlowRecord(f).then(record=>{
      if(epoch!==flowRefreshEpoch.current||generation!==historyGeneration.current)return;
      historyGeneration.current++;
      setFlows(previous=>[record,...previous.filter(value=>value.id!==record.id)]);
    }).catch(error=>notify(String(error)));
  }
  function notify(text: string) { setNotice(text); }
  useEffect(() => { if (!notice) return; const timer = setTimeout(() => setNotice(''), 5000); return () => clearTimeout(timer); }, [notice]);
  useEffect(() => {
    let disposed = false;
    const refreshFlows=createHistoryRefresh(()=>flowRefreshEpoch.current);
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const [, state] = await Promise.all([refreshFlows(rows => { if (!disposed) { historyGeneration.current++; setFlows(rows); } }), invoke<ProxyStatus>('proxy_status')]);
        const tunState = await invoke<TunStatus>('tun_status');
        if (!disposed) setTun(tunState);
        const interceptState = await invoke<InterceptSnapshot>('interception');
        if (!disposed) setInterception(interceptState);
        if (!disposed) { setStatus(state); setConnected(true); }
      } catch { if (!disposed) { setConnected(false); setStatus(s => ({ ...s, running: false })); } }
      finally { if (!disposed) timer = setTimeout(() => void refresh(), 1500); }
    }
    void refresh();
    return () => { disposed = true; clearTimeout(timer); };
  }, []);

  async function capture() {
    if (proxyBusy) return;
    if (!connected) { notify(t("抓包服务未连接，请运行 npm run capture。")); return; }
    setProxyBusy(true);
    try {
      if (status.running) await invoke('stop_proxy');
      else {
        if (useSystemProxy) {
          const system = await invoke<{supported:boolean;pendingRecovery:boolean}>('system_proxy_status');
          if (!system.supported) throw new Error(t("此平台暂需手动配置系统代理。"));
          if (system.pendingRecovery) throw new Error(t("请先恢复原代理，再启用自动设置。"));
        }
        await invoke('start_proxy', { port, host: host.trim() });
        if (useSystemProxy) {
          try { await invoke('set_system_proxy', {enabled:true}); }
          catch (e) { notify(t("监听已启动，但设置系统代理失败：") + String(e)); }
        }
      }
    } catch (e) { notify(String(e)); }
    finally {
      try { setStatus(await invoke<ProxyStatus>('proxy_status')); } catch { /* Polling reports connection failures. */ }
      setProxyBusy(false);
    }
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
  async function connectWebSocket(editorId:string,resolved:RequestDraft){
      setWsConnecting(v=>({...v,[editorId]:true}));
      try{const id=await invoke<string>('websocket_connect',{request:resolved});
        if(closedEditors.current.has(editorId)){await invoke('cancel_replay',{executionId:id});return;}
        setWsSessions(v=>({...v,[editorId]:id}));setEditors(items=>items.map(e=>e.id===editorId?{...e,selected:id}:e));setResponseTab('body');setResponseCollapsed(false);
      }catch(e){notify(String(e));}finally{setWsConnecting(v=>{const next={...v};delete next[editorId];return next;});}
  }
  async function send() {
    if (busy || !editor) return;
    let resolved:RequestDraft;
    try {
      const environment=library.environments.find(v=>v.id===environmentId);
      if(environmentId&&!environment)throw Error(t('请选择环境'));
      resolved=resolveEnvironment(draft,environment?.variables??{});
      if(!['http:', 'https:', 'ws:', 'wss:'].includes(new URL(resolved.url).protocol))throw Error(t("请输入有效的 HTTP 或 HTTPS URL。"));
    } catch(error){notify(String(error));return;}

    if (!connected) { notify(t("抓包服务未连接，请运行 npm run capture。")); return; }
    if(/^wss?:/i.test(resolved.url)){
      if(wsConnecting[editor.id]||wsActive)return;await connectWebSocket(editor.id,resolved);
      return;
    }
    if (!selectedEngine?.available) { notify(selectedEngine?.reason ?? t("所选发送引擎不可用，请刷新内核连接。")); return; }
    setSelected(null);
    await sendRequest(editor.id, resolved, selected ?? editor.parentId);
  }
  async function replayCaptured(f: Flow) {
    if (!connected) { notify(t("抓包服务未连接。")); return; }
    if(f.websocket){const id=openEditor(f);if(id)await connectWebSocket(id,{...clone(f.request),url:f.request.url.replace(/^http/,'ws'),method:'GET',bodyBase64:'',engine:'auto',tls:{preset:'native'}});return;}
    const request = clone(f.request);
    request.engine ??= 'auto';
    const engine = engines.find(e => e.id === request.engine);
    if (!engine?.available) { notify(engine?.reason ?? t("发送引擎不可用")); return; }
    const id = openEditor(f);
    if (id) await sendRequest(id, request, f.id);
  }
  async function copy(value: string) { try { await writeClipboard(value); notify(t("已复制到剪贴板")); } catch { notify(t("当前环境无法访问剪贴板")); } }
  const [certificateBusy,setCertificateBusy]=useState(false);
  async function exportCa() {
    if(certificateBusy)return;
    setCertificateBusy(true);
    try {
      const handle=await chooseSave("http-capture-ca.pem");if(handle===null){notify(t("已取消保存"));return;}
      const pem = await invoke<string>('export_certificate');
      notify(await saveText(pem,'http-capture-ca.pem','pem',handle));
    } catch (e) { notify(String(e)); } finally {setCertificateBusy(false);}
  }

  async function copyResponse() {
    if(!flow?.response)return;
    try{
      const full=await loadCompleteFlow(flow);
      if(full.response)await copy(decodeText(full.response.bodyBase64));
    }catch(error){notify(String(error))}
  }

  useEffect(() => {
    const shortcut = (event: KeyboardEvent) => {
      if(configurationImported)return;
      if (event.defaultPrevented || !(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== 's' || view !== 'replay' || !editor) return;
      event.preventDefault();
      if (!requestSaveTarget && !codeExportRequest && !modal && curlText === null) void saveRequest(event.shiftKey);
    };
    window.addEventListener('keydown', shortcut);
    return () => window.removeEventListener('keydown', shortcut);
  });

  if(configurationImported)return <div className="config-restart" role="dialog" aria-modal="true" aria-label={t('配置导入完成')}><div><h2>{t('配置导入完成')}</h2><p>{t('网络功能已停止。导入前的配置已保留为恢复点，重启后可在配置迁移页导出恢复。')}</p><p>{desktop?t('重启应用后使用新配置；导入的脚本、插件保持禁用，监听和 MCP 需要手动启用。'):t('请先停止并重新启动开发内核，再刷新此页面。仅刷新页面不会重启内核。')}</p>{!!configurationImported.warnings?.length&&<ul>{configurationImported.warnings.map((warning,i)=><li key={i}>{warning}</li>)}</ul>}{configurationError&&<p className="error-box" role="alert">{configurationError}</p>}{!configurationPreferencesApplied&&<button className="button primary" onClick={()=>finishConfigurationImport(configurationImported)}>{t('重试保存界面偏好')}</button>}{desktop&&<button className="button primary" autoFocus disabled={!configurationPreferencesApplied||configurationRestarting} onClick={()=>void restartAfterConfigurationImport()}>{configurationRestarting?t('正在重启…'):t('重启应用')}</button>}</div></div>;
  return <div className="app-shell">
    {closing && <div className="modal-backdrop" style={{zIndex:10000}} role="alertdialog" aria-modal="true" aria-label={t("正在保存并关闭…")}><div className="modal">{t("正在保存并关闭…")}</div></div>}
    <aside className="app-sidebar" aria-label={t('功能导航')}>
      <div className="sidebar-brand" title="HTTP Capture"><Activity size={23} strokeWidth={2}/></div>
      <nav>
        <button className={view === 'capture' || view === 'replay' ? 'sidebar-item active' : 'sidebar-item'} aria-current={view === 'capture' || view === 'replay' ? 'page' : undefined} onClick={() => setView('capture')}><Radio size={21}/><span>{t('抓包')}</span></button>
        <button className={view === 'interception' ? 'sidebar-item active' : 'sidebar-item'} aria-current={view === 'interception' ? 'page' : undefined} title={t('拦截') + (interception.config.request || interception.config.response ? ' · ' + t('已开启') : '')} onClick={() => setView('interception')}><Square size={20}/><span>{t('拦截')}</span>{interception.items.length > 0 ? <b className="sidebar-badge">{interception.items.length}</b> : (interception.config.request || interception.config.response) && <i className="sidebar-indicator is-enabled"/>}</button>
        <button className={view === 'blocking' ? 'sidebar-item active' : 'sidebar-item'} aria-current={view === 'blocking' ? 'page' : undefined} onClick={() => setView('blocking')}><Ban size={21}/><span>{t('阻止请求')}</span>{blocking.enabled && <i className="sidebar-indicator"/>}</button>
        <button className={view === 'rewrite' ? 'sidebar-item active' : 'sidebar-item'} aria-current={view === 'rewrite' ? 'page' : undefined} onClick={() => setView('rewrite')}><RotateCw size={21}/><span>{t('响应重写')}</span></button>
        <div className="sidebar-divider"/>
        <button className={view === 'library' ? 'sidebar-item active' : 'sidebar-item'} aria-current={view === 'library' ? 'page' : undefined} title={t('集合与环境')} aria-label={t('集合与环境')} onClick={() => { setView('library'); if (!libraryReady) void loadLibrary(); }}><FolderOpen size={21}/><span>{t('集合')}</span></button>
        <button className={view === 'plugins' ? 'sidebar-item active' : 'sidebar-item'} aria-current={view === 'plugins' ? 'page' : undefined} onClick={() => setView('plugins')}><Puzzle size={21}/><span>{t('插件')}</span></button>
        <button className={view === 'mcp' ? 'sidebar-item active' : 'sidebar-item'} aria-current={view === 'mcp' ? 'page' : undefined} onClick={() => setView('mcp')}><Code2 size={21}/><span>MCP</span></button>
      </nav>
      <button className="sidebar-item sidebar-help" onClick={() => setModal('help')}><CircleHelp size={21}/><span>{t('使用帮助')}</span></button>
    </aside>
    <div className="workspace">
      <header className="proxy-toolbar">
        <div className="workspace-heading"><span>HTTP CAPTURE</span><strong>{view === 'capture' ? t('流量捕获') : view === 'replay' ? t('请求重放') : view === 'interception' ? t('拦截') : view === 'rewrite' ? t('响应重写') : view === 'blocking' ? t('阻止请求') : view === 'library' ? t('集合与环境') : view === 'mcp' ? 'MCP' : t('插件')}</strong></div>
        <div className="proxy-address"><span className={status.running ? 'dot green' : 'dot'}/><div><span>{!connected ? t('内核未连接') : status.running ? t('代理已就绪') : t('代理已停止')}</span><code>{listenAddress}</code></div><button className="icon-button" title={wildcard ? t('所有网卡监听：客户端请填写本机局域网 IP 和端口') : t('复制代理地址')} disabled={wildcard} onClick={() => void copy(listenAddress)}><Copy size={14}/></button></div>
        <button className="upstream-chip" aria-label={t('上游代理设置')} title={t('上游代理设置') + ' · ' + (status.upstream?.enabled ? t('上游：{v0}', {v0: status.upstream.profileName ?? status.upstream.url}) : t('上游：直连'))} onClick={() => openSettings('upstream')}><Globe2 size={14}/><span>{status.upstream?.enabled ? t('上游：{v0}', {v0: status.upstream.profileName ?? status.upstream.url}) : t('上游：直连')}</span></button>
        <div className="toolbar-actions"><AppUpdater beforeInstall={beforeUpdateInstall} onBusyChange={value => { updateInProgress.current = value; }}/><SystemProxyToggle running={status.running} connected={connected} locked={proxyBusy} preferred={useSystemProxy} onPreference={setUseSystemProxy} onError={notify}/><ThemeSelector value={theme} onChange={value => { setTheme(value); applyTheme(value); }}/><button className="proxy-settings-button" aria-label={t('设置')} title={t('管理应用设置')} onClick={() => openSettings()}><Settings2 size={17}/><span>{t('设置')}</span></button><button className={'capture-switch' + (status.running ? ' is-running' : '')} disabled={proxyBusy} onClick={() => void capture()}>{status.running ? <Square size={15}/> : <Play size={15}/>}<span>{proxyBusy ? t('处理中…') : status.running ? t('停止') : t('开始捕获')}</span></button></div>
      </header>
      <nav className="work-tabs" aria-label={t("工作区标签")}>
        <div className="work-tab-items">
        <button className={view === 'capture' ? 'work-tab active' : 'work-tab'} onClick={() => setView('capture')}><Radio size={17} />{t("调试")}<span className="tab-count">({flows.filter(f => f.source === 'capture').length})</span></button>
        {editors.map(item => <div key={item.id} className={view === 'replay' && activeEditor === item.id ? 'work-tab active' : 'work-tab'}>
          <button onClick={() => { setActiveEditor(item.id); setView('replay'); }} title={item.draft.url}><Send size={16} />{editorLabel(item)}{runs.state(item.id) ? <span>{runs.state(item.id)?.cancelling ? t("取消中…") : t("发送中…")}</span> : <span className="dot green" />}</button>
          <button className="close-tab" aria-label={(t("关闭 ") + editorLabel(item) + t(" 标签"))} onClick={() => closeEditor(item.id)}><X size={13} /></button>
        </div>)}
        <button className="new-tab" title={t("新建请求")} aria-label={t("新建请求")} disabled={!workspaceReady} onClick={() => openEditor()}><Plus size={17} /><span>{t('新建请求')}</span></button>
        <button className="curl-import-button" disabled={readingClipboard || !workspaceReady} onClick={() => void openCurlImport()}><Terminal size={15}/>{t("导入 cURL")}</button>
        </div>
        <details className="request-history"><summary>{t("重放历史 (")}{flows.filter(f => f.source === 'replay').length})</summary><div>{flows.filter(f => f.source === 'replay').map(f => <button key={f.id} onClick={event => { void openRecordedEditor(f, 'result'); event.currentTarget.closest('details')?.removeAttribute('open'); }}>{f.request.method} {f.request.url} <span>{f.response?.status ?? 'ERR'}</span></button>)}</div></details>
      </nav>
      {workspaceError && <div className="workspace-save-status workspace-save-error" role="status"><CircleHelp size={14}/><span>{t(workspaceMessage)}</span>{workspaceReady && (workspaceConflict?<button disabled={workspaceRecovering} onClick={() => void recoverWorkspace()}>{workspaceRecovering?t('正在合并…'):t('保留草稿并合并重载')}</button>:<button disabled={workspaceRecovering} onClick={() => void saveWorkspace()}>{t('重试保存')}</button>)}</div>}
      {!connected && !workspaceError && <div className="preview-strip"><span><Layers3 size={14} />{t("界面预览 · 尚未连接抓包内核")}</span><span>{t("仅显示实际捕获的请求")}</span></div>}

      {interception.items.length > 0 && view !== 'interception' && <button className="preview-strip" onClick={() => setView('interception')}>{t("有")}{interception.items.length} {t("项等待拦截放行，点击处理")}</button>}
      {view === 'plugins' && <Plugins />}
      {view === 'mcp' && <McpPage />}
      {view === 'interception' && <Interception snapshot={interception} onUpdate={setInterception} />}
      <ResponseRewrite active={view === 'rewrite'} connected={connected} seed={rewriteSeed} />
      {view === 'blocking' && <RequestPolicies key={blockingEditorVersion} connected={connected} onSaved={loadBlocking} />}
      {view==='library'&&<RequestLibrary value={library} onChange={v=>{if(!libraryOperation.current){setLibrary(v);setLibraryMessage(t('有未保存的修改'));}}} onSave={()=>void saveLibrary()} onReload={()=>void loadLibrary()} onOpen={v=>openEditor(undefined,'draft',v.draft,v)} draft={editor?.draft} draftContext={currentRequestContext(editor)} message={libraryMessage} busy={libraryBusy||!libraryReady}/>}
      <div className="capture-page" hidden={view !== 'capture'}><CaptureView onRewriteResponse={connected ? rewriteResponse : undefined} onBlock={connected?(flow,kind)=>void blockRequest(flow,kind):undefined} onInterceptResponse={f => void interceptResponse(f)} onFlowsChanged={replaceFlows} active={view === 'capture'} busy={false} onReplayNow={f => void replayCaptured(f)} flows={flows} desktop={connected} running={status.running} address={clientAddress} onCopy={value => void copy(value)} onReplay={f => openEditor(f)} /></div>
      <div className="composer-page" hidden={view !== 'replay'}>
        <section className="detail-panel panel">
          <div className="library-toolbar composer-library-toolbar"><label>{t('发送环境')} <select value={environmentId} onChange={e=>setEnvironmentId(e.target.value)}><option value="">{t('无环境')}</option>{library.environments.map(v=><option key={v.id} value={v.id}>{v.name}</option>)}</select></label><select aria-label={t('应用脚本模板')} value="" onChange={e=>{const saved=library.scripts.find(v=>v.id===e.target.value);if(saved)setDraft({...draft,scripts:structuredClone(saved.scripts)});}}><option value="">{t('应用脚本模板')}</option>{library.scripts.map(v=><option key={v.id} value={v.id}>{v.name}</option>)}</select>{savedRequest && <button disabled={libraryBusy||!connected} onClick={()=>void saveRequest(true)}>{t('另存为')}</button>}{savedCollection && <button className="request-save-location" onClick={()=>setView('library')} title={t('查看集合')}><FolderOpen size={14}/><span>{savedCollection.name} / {savedRequest?.name}</span></button>}<div className="composer-toolbar-actions"><button disabled={!editor||requestMode!=='http'} title={requestMode!=='http'?t('当前支持导出 HTTP 请求代码。'):undefined} onClick={openCodeExport}><Code2 size={15}/>{t('导出代码')}</button><button className="request-save-button" disabled={libraryBusy||!connected||!editor} aria-keyshortcuts="Control+s Meta+s" onClick={()=>void saveRequest()}><Save size={15}/>{libraryBusy?t('正在保存…'):t('保存请求')}</button></div></div>
          {requestSaveError && !requestSaveTarget && <p className="request-save-inline-error" role="alert">{requestSaveError}</p>}
          <div className="request-bar"><select aria-label={t("请求类型")} value={requestMode} onChange={e=>changeRequestMode(e.target.value)}><option value="http">HTTP</option><option value="grpc">gRPC</option><option value="websocket">WebSocket</option></select><select aria-label={t("请求方法")} disabled={wsRequest||grpcRequest} value={wsRequest?'GET':draft.method} onChange={e => setDraft({ ...draft, method: e.target.value })}>{['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'].map(m => <option key={m}>{m}</option>)}</select><div className="url-field"><LockKeyhole size={14} /><input aria-label={t("请求 URL")} placeholder="https://example.com / wss://example.com/socket" value={draft.url} onChange={e => setDraft({ ...draft, url: e.target.value })} /></div><button className="request-tls" title={t("当前 TLS 预设，点击设置；实际握手版本见响应栏")} onClick={() => setTab('tls')}>TLS · {draft.tls.preset}</button>{!grpcRequest&&<button className="button primary replay" disabled={busy||(['sending','receiving'].includes(flow?.state??'')&&flow?.request.headers.some(h=>!h.disabled&&h.name.toLowerCase()==='content-type'&&isGrpcContentType(h.value)))||!!wsConnecting[editor?.id??'']||wsActive} onClick={() => void send()}><Send size={14} />{busy||wsConnecting[editor?.id??''] ? t("处理中…") : wsActive?(flow?.websocket?.state==='open'?t("WebSocket 已连接"):t("WebSocket 连接中…")):wsRequest?t("连接 WebSocket"):t("发送请求")}</button>}{busy && <button className="button" disabled={activeRun?.cancelling} onClick={() => editor && void runs.cancel(editor.id)}>{activeRun?.cancelling ? t("取消中…") : t("取消请求")}</button>}</div>
          <SplitPane collapsedSecond={responseCollapsed} storageKey="composer-columns" label={t("调整请求与响应宽度")} direction="row" initial={50} min={25} max={75}><div className="replay-request">          <div className="tabs">{[['overview', t("总览"), null], ['headers', t("请求头"), draft.headers.length], ['body', t("请求体"), null], ['grpc', 'gRPC', null], ['tls', 'TLS', null], ['order', 'Header Order', null], ['scripts', t("脚本"), null], ['settings', t("设置"), null]].filter(([key])=>key==='grpc'?grpcRequest:key==='body'?requestMode==='http':key==='scripts'?!grpcRequest:true).map(([key, label, count]) => <button className={tab === key ? 'tab active' : 'tab'} key={key} onClick={() => setTab(String(key))}>{label}{count !== null && <span>{count}</span>}{key === 'tls' && <span className="tiny-dot" />}</button>)}<div className="tabs-right"><ShieldCheck size={13} />{t("有序字段")}</div></div>

          <div className="request-content">
          {editors.filter(item=>(requestModes[item.id]??(item.draft.headers.some(h=>!h.disabled&&h.name.toLowerCase()==='content-type'&&isGrpcContentType(h.value))?'grpc':'http'))==='grpc').map(item=><div key={item.id} hidden={item.id!==editor?.id||tab!=='grpc'}><GrpcPanel connected={connected} visible={view==='replay'&&item.id===editor?.id&&tab==='grpc'} savedDraft={item.grpcDraft} onDraftChange={patch=>setEditors(items=>items.map(e=>e.id===item.id?{...e,grpcDraft:{method:'',json:'{}',...e.grpcDraft,...patch}}:e))} environment={library.environments.find(v=>v.id===environmentId)?.variables??{}} environmentValid={!environmentId||library.environments.some(v=>v.id===environmentId)} flows={flows} draft={item.draft} flow={flows.find(f=>f.id===item.selected)} onChange={value=>setEditors(items=>items.map(e=>e.id===item.id?{...e,draft:value}:e))} onStarted={id=>{setEditors(items=>items.map(e=>e.id===item.id?{...e,selected:id}:e));setResponseCollapsed(false);setResponseTab("body")}}/></div>)}
          {tab === 'overview' && <RequestOverview request={draft} flow={flow} />}
          {tab === 'scripts' && <ScriptsEditor key={editor?.id} value={draft.scripts ?? emptyScripts} onChange={scripts=>setDraft({...draft,scripts})} logs={[...(flow?.notes??[]),flow?.error??'']} />}
          {tab === 'settings' && <div className="tls-editor"><EngineSelector draft={draft} engines={engines} onChange={setDraft} /><RequestProxySelector draft={draft} onChange={setDraft} onManage={() => openSettings('upstream')} /></div>}
          {tab === 'headers' && <OrderedHeaders key={editor?.id} headers={draft.headers} onChange={headers => setDraft({...draft,headers})} />}
          {tab === 'order' && <HeaderOrder onCopy={value => void copy(value)} headers={[...(draft.pseudoHeaders ?? []), ...draft.headers]} sent={flow?.response?.sentRequestHeaders} />}
          {tab === 'body' && <RequestBodyEditor key={editor?.id} draft={draft} onChange={setDraft}/>}
          {tab === 'tls' && <div className="tls-editor"><EngineSelector draft={draft} engines={engines} onChange={setDraft} /><TlsDetails flow={flow} /><div className="tls-intro"><ShieldCheck size={21} /><div><strong>{t("ClientHello 配置")}</strong><p>{t("选择浏览器预设，或调整握手参数。每次重放使用新连接。")}</p></div></div><div className="preset-grid">{[['native', 'Native', draft.engine === 'auto' ? t("Auto · h2 / h1 协商") : draft.engine === 'h2' ? t("HTTP/2 · 默认 TLS") : draft.engine === 'h3' ? t("HTTP/3 · 默认 TLS") : t("HTTP/1.1 · 原生保序")], ['chrome', 'Chrome', t("TLS + HTTP/2 预设")], ['firefox', 'Firefox', t("TLS + HTTP/2 预设")]].map(([key, label, desc]) => <button disabled={!selectedEngine?.available || !selectedEngine.profiles.includes(key)} title={!selectedEngine?.profiles.includes(key) ? t("当前发送引擎不支持此预设") : undefined} className={draft.tls.preset === key ? 'preset active' : 'preset'} key={key} onClick={() => setDraft({ ...draft, tls: { preset: key, clientHelloHex: draft.tls.clientHelloHex } })}><Globe2 size={16} /><strong>{label}</strong><small>{desc}</small>{draft.tls.preset === key && <Check size={13} />}</button>)}</div>{draft.tls.preset !== 'native' && <label className="setting-label">{t('浏览器 TLS 版本')}<select aria-label={t('浏览器 TLS 版本')} value={draft.tls.browserVersion ?? 'auto'} onChange={e => setDraft({ ...draft, tls: { ...draft.tls, browserVersion: e.target.value === 'auto' ? null : e.target.value } })}><option value="auto">{t('跟随 User-Agent（默认）')}</option><option value="latest">{t('使用库支持的最新版本')}</option>{(selectedEngine?.browserVersions?.[draft.tls.preset] ?? []).map(version => <option key={version} value={String(version)}>{version}</option>)}{draft.tls.browserVersion && !['auto','latest'].includes(draft.tls.browserVersion) && !(selectedEngine?.browserVersions?.[draft.tls.preset] ?? []).includes(Number(draft.tls.browserVersion)) && <option value={draft.tls.browserVersion}>{draft.tls.browserVersion} · {t('当前构建不支持')}</option>}</select><small>{t('自动模式读取当前浏览器类型的 UA 版本；无对应 UA 时使用库内最新版本。不存在的版本会报错，请手动选择。此设置不修改请求头。')}</small></label>}{draft.engine === 'httpcloak' && <ClientHelloEditor tls={draft.tls} onChange={tls => setDraft({...draft,tls})} />}{draft.engine !== 'httpcloak' && <div className="tls-fields"><label>{t('TLS 版本')}<select aria-label={t('TLS 版本')} disabled={draft.tls.preset !== 'native'} value={draft.tls.version ?? 'auto'} onChange={e => setDraft({ ...draft, tls: { ...draft.tls, version: e.target.value === 'auto' ? null : e.target.value } })}><option value="auto">{t('自动（TLS 1.3 / 1.2）')}</option><option value="1.2" disabled={draft.engine === 'h3'}>TLS 1.2</option><option value="1.3">TLS 1.3</option></select></label><p>{t('原生引擎支持 TLS 版本、密码套件和密钥交换组。列表用冒号分隔，留空使用默认配置；ALPN 跟随发送引擎。HTTP/3 仅支持 TLS 1.3。')}</p><p>{t('密码套件使用 IANA 名称，按输入顺序提供。原生引擎不模拟浏览器指纹，不支持自定义签名算法、GREASE 或扩展排列。')}</p>{[['cipherList', 'Cipher suites', t("继承预设 · BoringSSL cipher list")], ['sigalgsList', 'Signature algorithms', t("继承预设")], ['curvesList', 'Supported groups', t("继承预设")]].map(([key, label, placeholder]) => <label key={key}>{label}<input disabled={draft.tls.preset === 'native' && key === 'sigalgsList'} placeholder={draft.tls.preset === 'native' ? key === 'cipherList' ? 'TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384' : 'X25519:secp256r1:secp384r1' : placeholder} value={String(draft.tls[key as keyof typeof draft.tls] ?? '')} onChange={e => setDraft({ ...draft, tls: { ...draft.tls, [key]: e.target.value || null } })} /></label>)}<div className="tls-toggles">{[['grease', 'GREASE'], ['permuteExtensions', t("扩展随机排列")]].map(([key, label]) => <label key={key}>{label}<select disabled={draft.tls.preset === 'native'} value={draft.tls[key as 'grease'] == null ? 'default' : String(draft.tls[key as 'grease'])} onChange={e => setDraft({ ...draft, tls: { ...draft.tls, [key]: e.target.value === 'default' ? null : e.target.value === 'true' } })}><option value="default">{t("继承预设")}</option><option value="true">{t("开启")}</option><option value="false">{t("关闭")}</option></select></label>)}</div></div>}</div>}
          </div>

          </div><div className="replay-response"><div className="response-heading"><button className="response-collapse" aria-expanded={!responseCollapsed} aria-label={responseCollapsed ? t("展开响应面板") : t("收起响应面板")} onClick={() => setResponseCollapsed(value => !value)}><ChevronDown size={15} style={{ transform: responseCollapsed ? 'rotate(-90deg)' : undefined }} />{t("响应 ·")}{responseCollapsed ? t("展开") : t("收起")}</button>{flow?.response && <div className="response-meta"><span className="response-protocol" title={t("代理到上游服务器的实际 TLS 版本")}>{flow.state === 'blocked' ? '—' : flow.response.tlsVersion ?? (flow.request.url.startsWith('https:') ? t("TLS 未记录") : t("明文"))}</span><span className="response-protocol" aria-label={t("响应协议")} title={t("本次上游响应实际使用的协议")}>{flow.state === 'blocked' ? t('本地阻止') : flow.response.version}</span><span className={(flow.response.status >= 400 ? 'bad' : 'success')}>{flow.response.status} {flow.response.status === 200 ? 'OK' : ''}</span><span>{flow.durationMs} ms</span><span>{responseBodyLength===undefined?'—':`${(responseBodyLength/1024).toFixed(2)} KB`}</span></div>}</div>
          <div className="tabs response-tabs">{[['body', t("响应体")], ['headers', t("响应头")], ['notes', t("发送说明")]].map(([key, label]) => <button className={responseTab === key ? 'tab active' : 'tab'} key={key} onClick={() => setResponseTab(key)}>{label}</button>)}<div className="tabs-right"><FileJson size={13} />{responseTab === 'body' ? 'RESPONSE' : 'DETAILS'}<button hidden={responseTab === 'body'} className="icon-button" title={t("复制响应")} disabled={!flow?.response||(isFlowMetadata(flow)&&!connected)} onClick={() => void copyResponse()}><Copy size={13} /></button></div></div>
          <div className="response-content">{flow?.state === 'blocked' && <div className="blocked-banner" role="status">{t('请求已被 Block 规则阻止')} · {flow.notes.join(' · ')}</div>}{flow?.error && <div className="error-box">{flow.error}</div>}{view==='replay'&&!responseCollapsed&&(responseTab === 'body' ? flow?.websocket ? <WebSocketFrames key={flow.id} flow={flow} messageDraft={wsDrafts[editor?.id??'']} onMessageDraftChange={patch=>{const id=editor?.id;if(id)setWsDrafts(v=>({...v,[id]:{...(v[id]??{body:'',opcode:'1'}),...patch}}));}} onCopy={value=>void copy(value)} /> : flow?.response ? flow.response.headers.some(h=>!h.disabled&&h.name.toLowerCase()==='content-type'&&isGrpcContentType(h.value)) ? <GrpcResponse flow={flow} visible={view==='replay'&&!responseCollapsed&&responseTab==='body'} connected={connected}/> : isFlowMetadata(flow) ? <RecordedBody key={flow.id} flow={flow} variant="response" headers={flow.response.headers} connected={connected} onCopy={value=>void copy(value)} /> : <ResponseBody key={flow.id} body={flow.response.bodyBase64} headers={flow.response.headers} onCopy={value => void copy(value)} /> : <div className="empty"><Code2 size={27} /><p>{(busy||["sending","receiving"].includes(flow?.state??"")) ? t("正在发送；可以切换标签或继续编辑，改动将在下次发送时生效。") : t("发送请求后在这里查看响应")}</p></div> : responseTab === 'headers' ? <div className="response-header-list">{flow?.response?.headers.map((h, i) => <div key={i}><span>{h.name}</span><code>{h.value}</code></div>)}</div> : <div className="notes">{(flow?.notes ?? [t("尚未发送。")]).map((note, i) => <p key={i}><CircleHelp size={14} />{note}</p>)}</div>)}</div>
          </div></SplitPane>
        </section>
      </div>
      {certificateBusy && <FileProgress message={t("正在导出证书…")} />}
      <footer className="statusbar"><span><span className={status.running ? 'dot green' : 'dot'} />{connected ? status.running ? t("代理已就绪") : t("等待启动代理") : t("未连接内核")}<span className="footer-divider" />{flows.filter(f => f.source === 'capture').length} {t("个捕获请求")}</span><span className="workspace-save-inline">{!workspaceError && t(workspaceMessage)}</span><span><LockKeyhole size={12}/>{t('本地工作空间')}</span></footer>
    </div>
    {codeExportRequest && <ExportCodeDialog request={codeExportRequest} onClose={()=>setCodeExportRequest(null)}/>}
    {requestSaveTarget && <SaveRequestDialog collections={library.collections} initialName={requestSaveTarget.name} initialCollectionId={requestSaveTarget.collectionId} defaultCollectionName={t('我的请求')} busy={libraryBusy} error={requestSaveError} onSave={selection=>void confirmRequestSave(selection)} onClose={()=>{if(!libraryOperation.current){setRequestSaveTarget(null);setRequestSaveError('');}}}/>}
    {notice && <div className="toast" role="status"><CircleHelp size={17} /><span>{notice}</span><button onClick={() => setNotice('')} aria-label={t("关闭提示")}><X size={15} /></button></div>}
    {curlText !== null && <div className="modal-backdrop" onKeyDown={e => { if (e.key === 'Escape') setCurlText(null); }}><div className="modal curl-import-modal" role="dialog" aria-modal="true" aria-label={t("导入 cURL")}><button className="modal-close icon-button" aria-label={t("关闭导入")} onClick={() => setCurlText(null)}><X size={19} /></button><h2>{t("导入 cURL")}</h2><p>{t("可从剪贴板导入请求，或粘贴命令。导入后可编辑，点击发送才会发出请求。")}</p><textarea autoFocus aria-label={t("cURL 命令")} placeholder="curl 'https://example.com' -H 'Accept: application/json'" value={curlText} onChange={e => { setCurlText(e.target.value); setCurlError(''); }} />{curlError && <p role="alert">{curlError}</p>}<div className="curl-import-actions"><button className="button" onClick={() => { setCurlText(null); openEditor(); }}>{t("创建空请求")}</button><button className="button primary" disabled={!curlText.trim()} onClick={confirmCurl}>{t("导入请求")}</button></div></div></div>}
    {modal && <div className="modal-backdrop settings-backdrop" onClick={() => setModal(null)}>
      <div className={`modal settings-layout ${modal === 'settings' ? 'settings-modal' : 'help-modal'}`}
        ref={settingsDialog}
        role="dialog" aria-modal="true" aria-label={modal === 'settings' ? t("设置") : t("使用帮助")}
        onClick={event => event.stopPropagation()}>
        <header className="settings-dialog-header">
          <div className="settings-dialog-symbol">{modal === 'settings' ? <Settings2 size={22}/> : <Terminal size={22}/>}</div>
          <div className="settings-dialog-title">
            <span>HTTP CAPTURE</span>
            <h2>{modal === 'settings' ? t("设置") : t("开始使用 HTTP Capture")}</h2>
          </div>
          <button type="button" className="modal-close icon-button" aria-label={t("关闭")} onClick={() => setModal(null)}><X size={19}/></button>
        </header>
        {modal === 'settings' ? <SettingsTabs active={settingsPage} onChange={setSettingsPage} pages={[
          { id: 'general', label: t('常规'), icon: <Settings2 size={16}/>, content: <>
            <section className="settings-section settings-appearance" aria-labelledby="settings-appearance-title">
              <div className="settings-section-heading"><Settings2 size={17}/><h3 id="settings-appearance-title">{t("外观偏好")}</h3></div>
              <div className="settings-field-grid">
                <LanguageSelector/>
                <label className="setting-label">{t("界面缩放")}<select aria-label={t("界面缩放")} value={zoom} onChange={event => setZoom(Number(event.target.value))}>
                  {[75,90,100,110,125,150].map(value => <option key={value} value={value}>{value}%</option>)}
                </select></label>
              </div>
            </section>

            <section className="settings-section settings-listener" aria-labelledby="settings-listener-title">
              <div className="settings-section-heading"><Radio size={17}/><h3 id="settings-listener-title">{t("监听与连接")}</h3></div>
              <p className="settings-section-description">{t("停止代理后可以修改监听地址和端口。局域网模式允许其他设备使用此抓包代理。")}</p>
              <div className="settings-listener-fields">
                <ListenerSettings ref={listenerSettings} host={host} port={port} running={status.running} onHost={setHost} onPort={setPort}/>
              </div>
              <div className="settings-address-card">
                <span>{t("客户端 HTTP 和 HTTPS 代理地址：")}</span><code>{clientAddress}</code>
              </div>
              <p className="muted settings-connection-help">
                {t("HTTPS 解密需要在客户端设备上信任本地 CA。")}
                {wildcard && t("0.0.0.0 / :: 是监听地址，不要填入客户端代理设置。")}
                {host !== '127.0.0.1' && t("请在可信局域网使用，并按需允许防火墙入站；当前抓包入口不提供代理认证。")}
                {t("不会自动修改防火墙、系统代理或证书信任。")}
              </p>
            </section>

          </> },
          { id: 'upstream', label: t('上游代理'), icon: <Globe2 size={16}/>, content:
            <section className="settings-section settings-upstream">
              <UpstreamSettings current={status.upstream} connected={connected} onSaved={async () => setStatus(await invoke<ProxyStatus>('proxy_status'))}/>
            </section>
          },
          { id: 'system', label: t('系统与证书'), icon: <ShieldCheck size={16}/>, content:
            <section className="settings-section settings-system" aria-labelledby="settings-system-title">
              <div className="settings-section-heading"><ShieldCheck size={17}/><h3 id="settings-system-title">{t("系统与证书")}</h3></div>
              <SystemSettings running={status.running} useSystemProxy={useSystemProxy} onUseSystemProxy={setUseSystemProxy}/>
              <div className="settings-certificate-export">
                <div className="setting-card">
                  <ShieldCheck size={18}/>
                  <div><strong>{t("本地 CA 证书")}</strong><p>{status.caPath || t("启动桌面端后生成，每台设备独立保存。")}</p></div>
                </div>
                <button type="button" className="button" disabled={!connected} onClick={() => void exportCa()}>{t("导出 CA 证书")}</button>
              </div>
            </section>
          },
          { id: 'certificates', label: t('证书管理'), icon: <ShieldCheck size={16}/>, content:
            <Certificates connected={connected} active={modal==='settings'&&settingsPage==='certificates'}/>
          },
          { id: 'decryption', label: t('HTTPS 解密'), icon: <LockKeyhole size={16}/>, content:
            <section className="settings-section settings-decryption"><DecryptionSettings/></section>
          },
          { id: 'tun', label: 'TUN', icon: <Layers3 size={16}/>, content:
            <section className="settings-section settings-tun"><TunSettings status={tun} onUpdate={setTun}/></section>
          },
          { id: 'migration', label: t('配置迁移'), icon: <FileJson size={16}/>, content:
            <ConfigMigration connected={connected} beforeTransfer={beforeConfigurationTransfer} onImported={finishConfigurationImport}/>
          },
        ]}/> : <div className="settings-dialog-body"><div className="settings-help-content">
            <p>{desktop ? t("启动捕获后，将客户端 HTTP / HTTPS 代理设置为下方地址。") : t("主页面只列出实际捕获的请求。当前预览未连接内核，列表为空。")}</p>
            <div className="command">{clientAddress}</div>
            <ol>
              <li>{t("点击请求，查看请求头和响应。")}</li>
              <li>{t("点击“发送到重放”创建可编辑的请求副本。")}</li>
              <li>{t("在重放页编辑请求头，配置 TLS 并发送。")}</li>
              <li>{t("真实抓包需通过")}<code>npm run desktop</code> {t("启动。")}</li>
            </ol>
          </div></div>}
        <footer className="settings-dialog-footer">
          {modal === 'settings' && <span className="settings-save-note">{t('上游代理与解密策略请在各页保存。')}</span>}
          <button type="button" className="button primary modal-done" onClick={() => void finishModal()}>{t("完成")}</button>
        </footer>
      </div>
    </div>}
  </div>;
}
