import { t } from './i18n';
import { invoke } from './api';
import { importSession } from './importSession';
import { saveSession } from './sessions';
import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowDown, ArrowUp, Copy, Radio, Search, Send, X, Globe2, FileText, Braces } from 'lucide-react';
import { decodeText, type Flow, type Header } from './types';
import SplitPane from './SplitPane';
import ResponseBody from './ResponseBody';
import WebSocketFrames from './WebSocketFrames';
import { captureColumns, readColumns, saveColumns } from './captureColumns';
import { flowType } from './flowType';
import HeaderOrder from './HeaderOrder';
import { RequestOverview, TlsDetails } from './RequestDetails';
import RequestMenu, { type MenuTarget } from './RequestMenu';
import { toCurl } from './curl';

function Headers({ headers }: { headers: Header[] }) {
  return <div className="inspector-headers">{headers.map((h, i) => <div key={i}><span>{i + 1}</span><strong>{h.name}</strong><code>{h.value}</code></div>)}</div>;
}

function MessageView({ tab, headers, body, raw }: { tab: string; headers: Header[]; body: string; raw?: string | null }) {
  if (tab === 'headers') return <Headers headers={headers} />;
  // atob preserves the original header bytes via a one-byte character mapping.
  // UTF-8 decoding would replace valid non-UTF-8 field values.
  if (tab === 'raw') return <pre>{raw ? atob(raw) : t("未记录原始头部")}</pre>;
  if (tab === 'base64') return <pre>{body || t("（空正文）")}</pre>;
  const encoding = headers.find(h => h.name.toLowerCase() === 'content-encoding')?.value;
  return <>{encoding && encoding.toLowerCase() !== 'identity' && <p>{t("正文保留 Content-Encoding:")}{encoding}{t("，尚未解压；可切换 Base64 查看原始正文数据。")}</p>}<pre>{decodeText(body) || t("（空正文）")}</pre></>;
}

const messageTabs = [['headers', '头部'], ['raw', '原始头部'], ['body', '正文'], ['base64', 'Base64']];

function bodySize(value: string) {
  return value.length * 3 / 4 - (value.endsWith('==') ? 2 : value.endsWith('=') ? 1 : 0);
}
const filters = [['all', 'All'], ['websocket', 'WS / WSS'], ['http', 'Http'], ['https', 'Https'], ['HTTP/1.1', 'HTTP/1.1'], ['HTTP/2', 'h2'], ['HTTP/3', 'h3'], ['json', 'JSON'], ['text', '文本'], ['html', 'HTML'], ['javascript', 'JS'], ['image', '图片'], ['2', '2xx'], ['3', '3xx'], ['4', '4xx'], ['5', '5xx'], ['errors', '错误']];
function matchesFilter(f: Flow, filter: string) {
  const type = f.response?.headers.find(h => h.name.toLowerCase() === 'content-type')?.value.toLowerCase() ?? '';
  if (filter === 'all') return true;
  if (filter === 'websocket') return !!f.websocket;
  if (filter === 'http' || filter === 'https') return f.request.url.startsWith(filter + ':');
  if (filter === 'errors') return !!f.error || (f.response?.status ?? 0) >= 400;
  if (/^[2345]$/.test(filter)) return String(f.response?.status ?? '').startsWith(filter);
  if (filter.startsWith('HTTP/')) return f.clientProtocol === filter;
  return type.includes(filter);
}

export default function CaptureView({ flows, desktop, running, address, onReplay, onReplayNow, busy, active, onCopy, onFlowsChanged }: {
  onFlowsChanged: (flows: Flow[]) => void; flows: Flow[]; desktop: boolean; running: boolean; address: string;
  onReplay: (flow: Flow) => void; onCopy: (text: string) => void;
  onReplayNow: (flow: Flow) => void; busy: boolean; active: boolean;
}) {
  const [widths,setWidths]=useState<Record<string,number>>(()=>{try{const data=JSON.parse(localStorage.getItem('http-capture.capture.column-widths')??'{}');return Object.fromEntries(Object.entries(data).filter(([,v])=>typeof v==='number'&&v>=32&&v<=1200)) as Record<string,number>;}catch{return {};}});
  function resizeColumn(id:string,width:number){setWidths(previous=>{const next={...previous,[id]:Math.max(32,Math.min(1200,Math.round(width)))};try{localStorage.setItem('http-capture.capture.column-widths',JSON.stringify(next));}catch{}return next;});}
  const resizeStart=useRef<{id:string;x:number;width:number}|null>(null);
  const [columns, setColumns] = useState(readColumns);
  const [columnsOpen, setColumnsOpen] = useState(false);
  const columnPanel = useRef<HTMLDivElement>(null);
  const columnButton = useRef<HTMLButtonElement>(null);
  const visibleColumns = captureColumns.filter(c => columns.includes(c.id));
  function updateColumns(next: string[]) { setColumns(next); saveColumns(next); }
  useEffect(() => {
    if (!columnsOpen) return;
    const close = (e: PointerEvent) => { if (!columnPanel.current?.contains(e.target as Node) && !columnButton.current?.contains(e.target as Node)) setColumnsOpen(false); };
    const escape = (e: KeyboardEvent) => { if (e.key === 'Escape') { setColumnsOpen(false); columnButton.current?.focus(); } };
    document.addEventListener('pointerdown', close); document.addEventListener('keydown', escape);
    return () => { document.removeEventListener('pointerdown', close); document.removeEventListener('keydown', escape); };
  }, [columnsOpen]);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const anchor = useRef<string | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const [fileBusy, setFileBusy] = useState(false);
  const [fileMessage, setFileMessage] = useState('');
  async function save(items: Flow[], format: 'json' | 'har' = 'json') {
    if (fileBusy) return;
    setFileBusy(true);
    try { setFileMessage(await saveSession(items, format)); } catch (e) { setFileMessage(String(e)); } finally { setFileBusy(false); }
  }
  async function remove(ids: string[]) {
    if (fileBusy || !ids.length) return;
    setFileBusy(true);
    try { for (let i = 0; i < ids.length; i += 10000) await invoke('delete_flows', { ids: ids.slice(i, i + 10000) }); const removed = new Set(ids); onFlowsChanged(flows.filter(f => !removed.has(f.id))); setSelected(previous => previous && ids.includes(previous) ? null : previous); setChecked(previous => new Set([...previous].filter(id => !ids.includes(id)))); setFileMessage(t("已删除 {v0} 条记录", { v0: ids.length })); }
    catch (e) { setFileMessage(String(e)); } finally { setFileBusy(false); }
  }
  async function importFile(file: File) {
    setFileBusy(true);
    try {
      const count = await importSession(file, (done, total) => setFileMessage(`${done} / ${total}`));
      onFlowsChanged(await invoke<Flow[]>('list_flows')); setQuery(''); setFilter('all');
      setFileMessage(t("已导入 {v0} 条记录", { v0: count }));
    } catch (e) { setFileMessage(String(e)); } finally { setFileBusy(false); }
  }
  async function databaseAction(command: string) {
    if (fileBusy) return;
    setFileBusy(true);
    try {
      const result = await invoke<string | number>(command);
      if (command === 'clear_database') { onFlowsChanged([]); setChecked(new Set()); }
      setFileMessage(command === 'backup_database' ? `${t("备份已保存：")}${result}` : t("操作完成"));
    } catch (error) { setFileMessage(String(error)); } finally { setFileBusy(false); }
  }
  const [selected, setSelected] = useState<string | null>(null);
  const menuIds = useRef<string[]>([]);
  const [menu, setMenu] = useState<MenuTarget | null>(null);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState('all');
  const [idSort, setIdSort] = useState<'asc' | 'desc'>(() => {
    try { return localStorage.getItem('http-capture.capture.id-sort') === 'asc' ? 'asc' : 'desc'; }
    catch { return 'desc'; }
  });
  function toggleIdSort() {
    const next = idSort === 'asc' ? 'desc' : 'asc';
    setIdSort(next);
    try { localStorage.setItem('http-capture.capture.id-sort', next); } catch { /* Optional preference. */ }
  }
  const [requestTab, setRequestTab] = useState('overview');
  const [responseTab, setResponseTab] = useState('body');
  const [direction, setDirection] = useState<'row' | 'column'>(() => {
    try { return localStorage.getItem('http-capture.layout.inspector-direction') === 'column' ? 'column' : 'row'; }
    catch { return 'row'; }
  });
  const captured = useMemo(() => flows.filter(f => f.source === 'capture'), [flows]);
  const filtered = useMemo(() => {
    const items = captured.filter(f =>
    `${f.request.method} ${f.request.url} ${f.response?.status ?? ''}`.toLowerCase().includes(query.toLowerCase()) &&
    matchesFilter(f, filter));
    return idSort === 'asc' ? items.reverse() : items;
  }, [captured, query, filter, idSort]);
  const tableWrap = useRef<HTMLDivElement>(null);
  const [viewport, setViewport] = useState({ top: 0, height: 600 });
  useEffect(() => {
    const element = tableWrap.current;
    if (!element) return;
    const update = () => setViewport({ top: element.scrollTop, height: element.clientHeight });
    const observer = new ResizeObserver(update); observer.observe(element); update();
    return () => observer.disconnect();
  }, []);
  useEffect(() => { if (tableWrap.current) tableWrap.current.scrollTop = 0; }, [query, filter, idSort]);
  const rowHeight = 32;
  const start = Math.min(Math.max(0, Math.floor((viewport.top - 36) / rowHeight) - 12), Math.max(0, filtered.length - 1));
  const end = Math.min(filtered.length, start + Math.ceil(viewport.height / rowHeight) + 26);
  const ids = useMemo(() => new Map(captured.map((f, i) => [f.id, captured.length - i])), [captured]);
  const picked = filtered.filter(f => checked.has(f.id));
  // Drop hidden selections so changing filters cannot delete unseen requests.
  useEffect(() => {
    const visible = new Set(filtered.map(f => f.id));
    setChecked(previous => {
      const next = new Set([...previous].filter(id => visible.has(id)));
      return next.size === previous.size ? previous : next;
    });
  }, [filtered]);
  function selectRow(id: string, modifiers: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }, toggle = false) {
    setSelected(id);
    const from = filtered.findIndex(f => f.id === anchor.current), to = filtered.findIndex(f => f.id === id);
    setChecked(previous => {
      if (modifiers.shiftKey && from >= 0) {
        const next = new Set(modifiers.ctrlKey || modifiers.metaKey || toggle ? previous : []);
        filtered.slice(Math.min(from, to), Math.max(from, to) + 1).forEach(f => next.add(f.id));
        return next;
      }
      if (toggle || modifiers.ctrlKey || modifiers.metaKey) {
        const next = new Set(previous); if (next.has(id)) next.delete(id); else next.add(id); return next;
      }
      return new Set([id]);
    });
    if (!modifiers.shiftKey || from < 0) anchor.current = id;
  }
  function selectContext(id: string) {
    setSelected(id);
    menuIds.current = checked.has(id) ? picked.map(f => f.id) : [id];
    if (!checked.has(id)) { setChecked(new Set([id])); anchor.current = id; }
  }
  function requestDelete(ids: string[]) { if (desktop && !fileBusy && ids.length) void remove(ids); }
  const flow = filtered.find(f => f.id === selected);
  return <main className={`capture-workbench ${flow ? 'with-inspector' : ''}`}>
    <div className="capture-filter" aria-label={t("会话管理")}>
      <button ref={columnButton} aria-expanded={columnsOpen} aria-controls="capture-column-settings" onClick={() => setColumnsOpen(!columnsOpen)}>{t("列设置")}</button>
      <button disabled={!desktop || fileBusy} onClick={() => fileInput.current?.click()}>{t("导入 HAR / 会话")}</button>
      <input ref={fileInput} hidden type="file" accept=".har,.json,application/json" onChange={e => { const file = e.target.files?.[0]; e.target.value = ''; if (file) void importFile(file); }} />
      <button disabled={!filtered.length} onClick={() => void save(filtered)}>{t("保存当前列表（")}{filtered.length}）</button>
      <button disabled={fileBusy || !filtered.length} onClick={() => void save(filtered, 'har')}>{t("导出列表 HAR")}</button>
      <button disabled={fileBusy || !picked.length} onClick={() => void save(picked, 'har')}>{t("导出选中 HAR")}</button>
      <button disabled={!picked.length} onClick={() => void save(picked)}>{t("保存选中（")}{picked.length}）</button>
      <button disabled={!desktop || !picked.length || fileBusy} onClick={() => requestDelete(picked.map(f => f.id))}>{t("删除选中（")}{picked.length}）</button><span>{t("Ctrl+A 全选 · Ctrl / ⌘ 点选 · Shift 连选")}</span>
      <button disabled={!desktop || fileBusy} onClick={() => void databaseAction('backup_database')}>{t("备份数据库")}</button>
      <button disabled={!desktop || fileBusy} onClick={() => void databaseAction('compact_database')}>{t("回收数据库空间")}</button>
      <button disabled={!desktop || fileBusy} onClick={() => { if (window.confirm(t("清空全部抓包及重放历史？此操作不可撤销，请先备份数据库。"))) void databaseAction('clear_database'); }}>{t("清空全部记录")}</button>
      <span role="status">{fileBusy ? `${t("处理中…")} ${fileMessage}` : fileMessage}</span>
    </div>
    {columnsOpen && <div ref={columnPanel} id="capture-column-settings" className="capture-column-settings" role="group" aria-label={t("列设置")}>
      <div className="column-settings-heading"><strong>{t("显示列")}</strong><button onClick={() => updateColumns(captureColumns.filter(c => c.default).map(c => c.id))}>{t("恢复默认")}</button><button aria-label={t("关闭")} onClick={() => { setColumnsOpen(false); columnButton.current?.focus(); }}><X size={16} /></button></div>
      <div className="column-settings-options">{captureColumns.map(c => <label key={c.id}><input type="checkbox" checked={columns.includes(c.id)} disabled={columns.length === 1 && columns.includes(c.id)} onChange={e => updateColumns(e.target.checked ? [...columns, c.id] : columns.filter(id => id !== c.id))} />{t(c.label)}</label>)}</div>
    </div>}
    <div className="capture-filter"><div className="search-box"><Search size={15} /><input aria-label={t("过滤捕获请求")} placeholder={t("过滤 URL、方法、状态码…")} value={query} onChange={e => setQuery(e.target.value)} />{query && <button aria-label={t("清除筛选")} onClick={() => setQuery('')}><X size={13} /></button>}</div>
      { filters.map(([key, label]) => <button key={key} className={`filter ${filter === key ? 'selected' : ''}`} onClick={() => setFilter(key)}>{t(label)}</button>)}<span className="capture-total">{filtered.length} / {captured.length} {t("个请求")}</span>
    </div>
    <SplitPane storageKey="capture-list" label={t("调整请求列表与详情高度")} direction="column" initial={55} min={15} max={80}>
    <div ref={tableWrap} onScroll={e => setViewport({ top: e.currentTarget.scrollTop, height: e.currentTarget.clientHeight })} className="session-table-wrap" tabIndex={0} aria-label={t("请求列表，Ctrl+A 全选")} onKeyDown={e => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') {
        e.preventDefault(); e.stopPropagation(); setChecked(new Set(filtered.map(f => f.id)));
      } else if (e.key === 'Delete') { e.preventDefault(); requestDelete(picked.map(f => f.id)); }
    }}><table className="session-table" style={{ minWidth: visibleColumns.reduce((sum, c) => sum + (widths[c.id]??c.width), 0) }} aria-label={t("捕获的请求")}><thead onContextMenu={e => { e.preventDefault(); setColumnsOpen(true); }}><tr>{visibleColumns.map(c => <th key={c.id} title={t(c.label)} aria-label={c.id === 'indicator' ? t(c.label) : undefined} style={{ width: widths[c.id]??c.width }} aria-sort={c.id === 'id' ? (idSort === 'asc' ? 'ascending' : 'descending') : undefined}>{c.id === 'id' ? <button className="id-sort-button" onClick={toggleIdSort} title={idSort === 'asc' ? t("ID 倒序") : t("ID 正序")}>ID {idSort === 'asc' ? '↑' : '↓'}</button> : c.id === 'indicator' ? '●' : t(c.label)}<span className="column-resizer" role="separator" tabIndex={0} aria-label={`${t('调整列宽')} ${t(c.label)}`} aria-orientation="vertical" aria-valuemin={32} aria-valuemax={1200} aria-valuenow={widths[c.id]??c.width} onPointerDown={e=>{e.preventDefault();e.stopPropagation();resizeStart.current={id:c.id,x:e.clientX,width:widths[c.id]??c.width};e.currentTarget.setPointerCapture(e.pointerId);}} onPointerMove={e=>{const start=resizeStart.current;if(start?.id===c.id)resizeColumn(c.id,start.width+e.clientX-start.x);}} onPointerUp={()=>{resizeStart.current=null;}} onLostPointerCapture={()=>{resizeStart.current=null;}} onKeyDown={e=>{if(e.key==='ArrowLeft'||e.key==='ArrowRight'){e.preventDefault();resizeColumn(c.id,(widths[c.id]??c.width)+(e.key==='ArrowRight'?10:-10));}}}/></th>)}</tr></thead><tbody>{start > 0 && <tr aria-hidden="true" className="virtual-spacer"><td colSpan={visibleColumns.length} style={{ height: start * rowHeight }} /></tr>}{filtered.slice(start, end).map(f => {

      let url: URL | undefined; try { url = new URL(f.request.url); } catch { /* Imported URL may be incomplete. */ }
      const contentType = f.response?.headers.find(h => h.name.toLowerCase() === 'content-type')?.value.split(';')[0] ?? '—';
      const cells: Record<string, React.ReactNode> = {'indicator': <td key="indicator"><span className={f.error ? 'traffic-dot failed' : f.response ? 'traffic-dot complete' : 'traffic-dot pending'} /></td>,'id': <td key="id">{ids.get(f.id)}</td>,'icon': <td key="icon">{contentType.includes('json') ? <Braces size={16} /> : contentType.includes('html') ? <Globe2 size={16} /> : <FileText size={16} />}</td>,'method': <td key="method">{f.request.method}</td>,'url': <td key="url" title={f.request.url}>{f.request.url}</td>,'type': <td key="type" title={f.response?.headers.find(h => h.name.toLowerCase() === "content-type")?.value ?? flowType(f)}>{t(flowType(f))}</td>,'status': <td key="status">{f.error ? 'ERR' : f.response?.status ?? '…'}</td>,'clientProtocol': <td key="clientProtocol" title={t("客户端协议")}>{f.clientProtocol ?? t("未记录")}</td>,'upstreamProtocol': <td key="upstreamProtocol" title={t("上游协议")}>{f.response?.version ?? (f.error ? t("未记录") : t("等待响应"))}</td>,'tls': <td key="tls">{f.response?.tlsVersion ?? (f.request.url.startsWith('https:') ? t("未记录") : '—')}</td>,'duration': <td key="duration">{(f.durationMs / 1000).toFixed(2)}s</td>,'size': <td key="size">{f.response ? `${(bodySize(f.response.bodyBase64) / 1024).toFixed(2)} KB` : '—'}</td>,'host': <td key="host" title={String(url?.hostname ?? '—')}>{url?.hostname ?? '—'}</td>,'path': <td key="path" title={String((url ? url.pathname + url.search : undefined) ?? '—')}>{(url ? url.pathname + url.search : undefined) ?? '—'}</td>,'scheme': <td key="scheme" title={String(url?.protocol.replace(':', '') ?? '—')}>{url?.protocol.replace(':', '') ?? '—'}</td>,'mime': <td key="mime" title={String(contentType ?? '—')}>{contentType ?? '—'}</td>,'started': <td key="started" title={String(new Date(f.startedAt).toLocaleString() ?? '—')}>{new Date(f.startedAt).toLocaleString() ?? '—'}</td>,'requestSize': <td key="requestSize" title={String(`${bodySize(f.request.bodyBase64)} B`)}>{`${bodySize(f.request.bodyBase64)} B`}</td>,'encoding': <td key="encoding" title={String(f.response?.headers.find(h => h.name.toLowerCase() === 'content-encoding')?.value ?? '—')}>{f.response?.headers.find(h => h.name.toLowerCase() === 'content-encoding')?.value ?? '—'}</td>,'cipher': <td key="cipher" title={String(f.response?.upstreamTls?.cipherSuite ?? '—')}>{f.response?.upstreamTls?.cipherSuite ?? '—'}</td>,'error': <td key="error" title={String(f.error ?? '—')}>{f.error ?? '—'}</td>};
      return <tr key={f.id} className={checked.has(f.id) ? 'selected' : ''} aria-selected={checked.has(f.id)} aria-haspopup="menu" tabIndex={0}
        onContextMenu={e => { e.preventDefault(); selectContext(f.id); setMenu({ flow: f, x: e.clientX, y: e.clientY, anchor: e.currentTarget }); }}
        onDoubleClick={() => onReplay(f)} onClick={e => selectRow(f.id, e)} onKeyDown={e => {
          if (e.target !== e.currentTarget) return;
          if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'c') {
            e.preventDefault(); onCopy(e.shiftKey ? toCurl(f.request) : f.request.url);
          } else if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key === 'Enter') {
            e.preventDefault(); onReplay(f);
          } else if (e.key === 'ContextMenu' || e.shiftKey && e.key === 'F10') {
            e.preventDefault(); const rect = e.currentTarget.getBoundingClientRect(); selectContext(f.id);
            setMenu({ flow: f, x: rect.left + 40, y: rect.bottom, anchor: e.currentTarget });
          } else if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); selectRow(f.id, e, e.key === ' '); }
        }}>
        {visibleColumns.map(c => cells[c.id])}
      </tr>;
    })}{end < filtered.length && <tr aria-hidden="true" className="virtual-spacer"><td colSpan={visibleColumns.length} style={{ height: (filtered.length - end) * rowHeight }} /></tr>}</tbody></table>
    {!filtered.length && <div className="capture-empty"><div className="capture-empty-symbol"><Radio size={30} strokeWidth={1.3} /></div><h2>{captured.length ? t("没有匹配的请求") : running ? t("正在等待请求") : t("暂无捕获的请求")}</h2><p>{captured.length ? t("调整筛选条件以查看其他请求。") : desktop ? t("启动捕获，将客户端代理设置为 {v0}。", { v0: address }) : t("连接桌面抓包内核后，实际请求会显示在这里。")}</p><small>{captured.length ? t("已捕获的记录仍保留在当前会话中") : t("选中请求查看详情 · 发送到重放后进行编辑")}</small></div>}
    </div>
    {flow && <section className="capture-inspector"><div className="inspector-heading"><span className={`method ${flow.request.method.toLowerCase()}`}>{flow.request.method}</span><code title={flow.request.url}>{flow.request.url}</code>{flow.notes.includes("SSE 接收中") && <button className="text-button" onClick={() => void invoke("cancel_replay", { executionId: flow.id }).catch(e => setFileMessage(String(e)))}>{t("停止 SSE")}</button>}<button className="text-button" onClick={() => onReplay(flow)}><Send size={13} />{t("发送到重放")}</button><div className="layout-direction" aria-label={t("请求响应布局")}>{([['row', t("左右")], ['column', t("上下")]] as const).map(([value, label]) => <button key={value} aria-pressed={direction === value} onClick={() => { setDirection(value); try { localStorage.setItem('http-capture.layout.inspector-direction', value); } catch { /* Optional persistence. */ } }}>{t(label)}</button>)}</div><button className="icon-button" aria-label={t("关闭详情")} onClick={() => setSelected(null)}><X size={15} /></button></div>
      <details className="capture-notes"><summary>{t("转发说明 ·")}{flow.notes.length} {t("项调整与限制")}</summary><p>{t("原始头部显示接收到的 HTTP/1 报文字节（按单字节字符映射展示）；正文已移除传输分块边界，保留内容编码。原始请求头不代表代理实际发出的头部。")}</p>{flow.notes.map((note, index) => <p key={index}>{note}</p>)}</details><SplitPane key={direction} storageKey={`capture-messages-${direction}`} label={t("调整请求与响应大小")} direction={direction} className="inspector-split" min={20} max={80}><section><div className="inspector-tabs"><strong><ArrowUp size={13} />{t("请求")}</strong>{[['overview', t("总览")], ...messageTabs, ['tls', 'TLS'], ['order', 'Header Order']].map(([key, label]) => <button key={key} className={`tab ${requestTab === key ? 'active' : ''}`} onClick={() => setRequestTab(key)}>{t(label)}</button>)}<button className="icon-button" title={t("复制请求 URL")} onClick={() => onCopy(flow.request.url)}><Copy size={13} /></button></div><div className="inspector-scroll">{requestTab === 'overview' ? <RequestOverview request={flow.request} flow={flow} /> : requestTab === 'tls' ? <TlsDetails flow={flow} /> : requestTab === 'order' ? <HeaderOrder onCopy={onCopy} captured original={!!flow.rawRequestHeadBase64 || !!flow.request.pseudoHeaders?.length} headers={[...((flow.originalRequest ?? flow.request).pseudoHeaders ?? []), ...(flow.originalRequest ?? flow.request).headers]} sent={flow.response?.sentRequestHeaders} /> : <MessageView tab={requestTab} headers={[...((flow.originalRequest ?? flow.request).pseudoHeaders ?? []), ...(flow.originalRequest ?? flow.request).headers]} body={(flow.originalRequest ?? flow.request).bodyBase64} raw={flow.rawRequestHeadBase64} />}</div></section>
      <section><div className="inspector-tabs"><strong><ArrowDown size={13} />{t("响应")}</strong>{messageTabs.map(([key, label]) => <button key={key} className={`tab ${responseTab === key ? 'active' : ''}`} onClick={() => setResponseTab(key)}>{t(label)}</button>)}<span className="inspector-status">{flow.response?.status} · {flow.durationMs} ms</span></div><div className={responseTab === 'body' ? 'inspector-body' : 'inspector-scroll'}>{flow.error && <div className="error-box">{flow.error}</div>}{flow.websocket && responseTab === 'body' ? <WebSocketFrames flow={flow} onCopy={onCopy} /> : flow.response ? responseTab === 'body' ? <ResponseBody key={flow.id} body={flow.response.bodyBase64} headers={flow.response.headers} onCopy={onCopy} /> : <MessageView tab={responseTab} headers={flow.response.headers} body={flow.response.bodyBase64} raw={flow.response.rawHeadBase64} /> : <p>{t("尚无响应")}</p>}</div></section></SplitPane>
    </section>}
    </SplitPane>
    {menu && active && <RequestMenu selectionCount={menuIds.current.length} onSave={() => void save(flows.filter(f => menuIds.current.includes(f.id)))} onDelete={desktop && !fileBusy ? () => requestDelete(menuIds.current) : undefined} target={menu} onClose={() => setMenu(null)} onEdit={onReplay} onReplay={onReplayNow} onCopy={onCopy} replayDisabled={!desktop || busy} />}
  </main>;
}
