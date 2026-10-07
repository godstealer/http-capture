import { t } from './i18n';
import { invoke } from './api';
import { readSession, saveSession } from './sessions';
import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowDown, ArrowUp, Copy, Radio, Search, Send, X, Globe2, FileText, Braces } from 'lucide-react';
import { decodeText, type Flow, type Header } from './types';
import SplitPane from './SplitPane';
import ResponseBody from './ResponseBody';
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
const filters = [['all', 'All'], ['http', 'Http'], ['https', 'Https'], ['HTTP/1.1', 'HTTP/1.1'], ['HTTP/2', 'h2'], ['HTTP/3', 'h3'], ['json', 'JSON'], ['text', '文本'], ['html', 'HTML'], ['javascript', 'JS'], ['image', '图片'], ['2', '2xx'], ['3', '3xx'], ['4', '4xx'], ['5', '5xx'], ['errors', '错误']];
function matchesFilter(f: Flow, filter: string) {
  const type = f.response?.headers.find(h => h.name.toLowerCase() === 'content-type')?.value.toLowerCase() ?? '';
  if (filter === 'all') return true;
  if (filter === 'http' || filter === 'https') return f.request.url.startsWith(filter + ':');
  if (filter === 'errors') return !!f.error || (f.response?.status ?? 0) >= 400;
  if (/^[2345]$/.test(filter)) return String(f.response?.status ?? '').startsWith(filter);
  if (filter.startsWith('HTTP/')) return (f.clientProtocol ?? f.response?.version) === filter;
  return type.includes(filter);
}

export default function CaptureView({ flows, desktop, running, address, onReplay, onReplayNow, busy, active, onCopy, onFlowsChanged }: {
  onFlowsChanged: (flows: Flow[]) => void; flows: Flow[]; desktop: boolean; running: boolean; address: string;
  onReplay: (flow: Flow) => void; onCopy: (text: string) => void;
  onReplayNow: (flow: Flow) => void; busy: boolean; active: boolean;
}) {
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const anchor = useRef<string | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const [fileBusy, setFileBusy] = useState(false);
  const [fileMessage, setFileMessage] = useState('');
  async function save(items: Flow[]) {
    try { setFileMessage(await saveSession(items)); } catch (e) { setFileMessage(String(e)); }
  }
  async function remove(ids: string[]) {
    if (fileBusy || !ids.length) return;
    setFileBusy(true);
    try { await invoke('delete_flows', { ids }); onFlowsChanged(flows.filter(f => !ids.includes(f.id))); setSelected(previous => previous && ids.includes(previous) ? null : previous); setChecked(previous => new Set([...previous].filter(id => !ids.includes(id)))); setFileMessage(t("已删除 {v0} 条记录", { v0: ids.length })); }
    catch (e) { setFileMessage(String(e)); } finally { setFileBusy(false); }
  }
  async function importFile(file: File) {
    setFileBusy(true);
    try {
      if (file.size > 5 * 1024 * 1024) throw new Error(t("当前支持最大 5 MB 的 HAR / 会话文件"));
      const imported = readSession(await file.text());
      const count = await invoke<number>('import_flows', { flows: imported });
      onFlowsChanged(await invoke<Flow[]>('list_flows')); setQuery(''); setFilter('all');
      setFileMessage(t("已导入 {v0} 条记录（列表展示最近 200 条）", { v0: count }));
    } catch (e) { setFileMessage(String(e)); } finally { setFileBusy(false); }
  }
  const [selected, setSelected] = useState<string | null>(null);
  const menuIds = useRef<string[]>([]);
  const [menu, setMenu] = useState<MenuTarget | null>(null);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState('all');
  const [requestTab, setRequestTab] = useState('overview');
  const [responseTab, setResponseTab] = useState('body');
  const [direction, setDirection] = useState<'row' | 'column'>(() => {
    try { return localStorage.getItem('http-capture.layout.inspector-direction') === 'column' ? 'column' : 'row'; }
    catch { return 'row'; }
  });
  const captured = useMemo(() => flows.filter(f => f.source === 'capture'), [flows]);
  const filtered = useMemo(() => captured.filter(f =>
    `${f.request.method} ${f.request.url} ${f.response?.status ?? ''}`.toLowerCase().includes(query.toLowerCase()) &&
    matchesFilter(f, filter)), [captured, query, filter]);
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
      <button disabled={!desktop || fileBusy} onClick={() => fileInput.current?.click()}>{t("导入 HAR / 会话")}</button>
      <input ref={fileInput} hidden type="file" accept=".har,.json,application/json" onChange={e => { const file = e.target.files?.[0]; e.target.value = ''; if (file) void importFile(file); }} />
      <button disabled={!filtered.length} onClick={() => void save(filtered)}>{t("保存当前列表（")}{filtered.length}）</button>
      <button disabled={!picked.length} onClick={() => void save(picked)}>{t("保存选中（")}{picked.length}）</button>
      <button disabled={!desktop || !picked.length || fileBusy} onClick={() => requestDelete(picked.map(f => f.id))}>{t("删除选中（")}{picked.length}）</button><span>{t("Ctrl+A 全选 · Ctrl / ⌘ 点选 · Shift 连选")}</span>
      <span role="status">{fileBusy ? t("处理中…") : fileMessage}</span>
    </div>
    <div className="capture-filter"><div className="search-box"><Search size={15} /><input aria-label={t("过滤捕获请求")} placeholder={t("过滤 URL、方法、状态码…")} value={query} onChange={e => setQuery(e.target.value)} />{query && <button aria-label={t("清除筛选")} onClick={() => setQuery('')}><X size={13} /></button>}</div>
      { filters.map(([key, label]) => <button key={key} className={`filter ${filter === key ? 'selected' : ''}`} onClick={() => setFilter(key)}>{t(label)}</button>)}<span className="capture-total">{filtered.length} / {captured.length} {t("个请求")}</span>
    </div>
    <SplitPane storageKey="capture-list" label={t("调整请求列表与详情高度")} direction="column" initial={55} min={15} max={80}>
    <div className="session-table-wrap" tabIndex={0} aria-label={t("请求列表，Ctrl+A 全选")} onKeyDown={e => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') {
        e.preventDefault(); e.stopPropagation(); setChecked(new Set(filtered.map(f => f.id)));
      } else if (e.key === 'Delete') { e.preventDefault(); requestDelete(picked.map(f => f.id)); }
    }}><table className="session-table" aria-label={t("捕获的请求")}><thead><tr><th></th><th>ID</th><th>{t("图标")}</th><th>{t("方法")}</th><th>URL</th><th>{t("状态")}</th><th>{t("协议")}</th><th>{t("TLS（上游）")}</th><th>{t("时长")}</th><th>{t("大小")}</th></tr></thead><tbody>{filtered.map((f, i) => {
      let host = f.request.url, path = '/'; try { const u = new URL(f.request.url); host = u.host; path = u.pathname + u.search; } catch { /* Keep malformed recorded URLs visible. */ }
      const contentType = f.response?.headers.find(h => h.name.toLowerCase() === 'content-type')?.value.split(';')[0] ?? '—';
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
        <td><span className={f.error ? 'traffic-dot failed' : f.response ? 'traffic-dot complete' : 'traffic-dot pending'} /></td><td>{captured.length - captured.indexOf(f)}</td><td>{contentType.includes('json') ? <Braces size={16} /> : contentType.includes('html') ? <Globe2 size={16} /> : <FileText size={16} />}</td><td>{f.request.method}</td><td title={f.request.url}>{f.request.url}</td><td>{f.error ? 'ERR' : f.response?.status ?? '…'}</td><td title={t("客户端：") + (f.clientProtocol ?? t("未记录")) + t(" / 上游：") + (f.response?.version ?? t("等待响应"))}>{f.clientProtocol ?? f.response?.version ?? '—'}{f.clientProtocol && f.response && f.clientProtocol !== f.response.version ? ' → ' + f.response.version : ''}</td><td>{f.response?.tlsVersion ?? (f.request.url.startsWith('https:') ? t("未记录") : '—')}</td><td>{(f.durationMs / 1000).toFixed(2)}s</td><td>{f.response ? `${(bodySize(f.response.bodyBase64) / 1024).toFixed(2)} KB` : '—'}</td>
      </tr>;
    })}</tbody></table>
    {!filtered.length && <div className="capture-empty"><div className="capture-empty-symbol"><Radio size={30} strokeWidth={1.3} /></div><h2>{captured.length ? t("没有匹配的请求") : running ? t("正在等待请求") : t("暂无捕获的请求")}</h2><p>{captured.length ? t("调整筛选条件以查看其他请求。") : desktop ? t("启动捕获，将客户端代理设置为 {v0}。", { v0: address }) : t("连接桌面抓包内核后，实际请求会显示在这里。")}</p><small>{captured.length ? t("已捕获的记录仍保留在当前会话中") : t("选中请求查看详情 · 发送到重放后进行编辑")}</small></div>}
    </div>
    {flow && <section className="capture-inspector"><div className="inspector-heading"><span className={`method ${flow.request.method.toLowerCase()}`}>{flow.request.method}</span><code title={flow.request.url}>{flow.request.url}</code><button className="text-button" onClick={() => onReplay(flow)}><Send size={13} />{t("发送到重放")}</button><div className="layout-direction" aria-label={t("请求响应布局")}>{([['row', t("左右")], ['column', t("上下")]] as const).map(([value, label]) => <button key={value} aria-pressed={direction === value} onClick={() => { setDirection(value); try { localStorage.setItem('http-capture.layout.inspector-direction', value); } catch { /* Optional persistence. */ } }}>{t(label)}</button>)}</div><button className="icon-button" aria-label={t("关闭详情")} onClick={() => setSelected(null)}><X size={15} /></button></div>
      <details className="capture-notes"><summary>{t("转发说明 ·")}{flow.notes.length} {t("项调整与限制")}</summary><p>{t("原始头部显示接收到的 HTTP/1 报文字节（按单字节字符映射展示）；正文已移除传输分块边界，保留内容编码。原始请求头不代表代理实际发出的头部。")}</p>{flow.notes.map((note, index) => <p key={index}>{note}</p>)}</details><SplitPane key={direction} storageKey={`capture-messages-${direction}`} label={t("调整请求与响应大小")} direction={direction} className="inspector-split" min={20} max={80}><section><div className="inspector-tabs"><strong><ArrowUp size={13} />{t("请求")}</strong>{[['overview', t("总览")], ...messageTabs, ['tls', 'TLS'], ['order', 'Header Order']].map(([key, label]) => <button key={key} className={`tab ${requestTab === key ? 'active' : ''}`} onClick={() => setRequestTab(key)}>{t(label)}</button>)}<button className="icon-button" title={t("复制请求 URL")} onClick={() => onCopy(flow.request.url)}><Copy size={13} /></button></div><div className="inspector-scroll">{requestTab === 'overview' ? <RequestOverview request={flow.request} flow={flow} /> : requestTab === 'tls' ? <TlsDetails flow={flow} /> : requestTab === 'order' ? <HeaderOrder onCopy={onCopy} captured original={!!flow.rawRequestHeadBase64 || !!flow.request.pseudoHeaders?.length} headers={[...((flow.originalRequest ?? flow.request).pseudoHeaders ?? []), ...(flow.originalRequest ?? flow.request).headers]} sent={flow.response?.sentRequestHeaders} /> : <MessageView tab={requestTab} headers={[...((flow.originalRequest ?? flow.request).pseudoHeaders ?? []), ...(flow.originalRequest ?? flow.request).headers]} body={(flow.originalRequest ?? flow.request).bodyBase64} raw={flow.rawRequestHeadBase64} />}</div></section>
      <section><div className="inspector-tabs"><strong><ArrowDown size={13} />{t("响应")}</strong>{messageTabs.map(([key, label]) => <button key={key} className={`tab ${responseTab === key ? 'active' : ''}`} onClick={() => setResponseTab(key)}>{t(label)}</button>)}<span className="inspector-status">{flow.response?.status} · {flow.durationMs} ms</span></div><div className={responseTab === 'body' ? 'inspector-body' : 'inspector-scroll'}>{flow.error ? <div className="error-box">{flow.error}</div> : flow.response ? responseTab === 'body' ? <ResponseBody key={flow.id} body={flow.response.bodyBase64} headers={flow.response.headers} onCopy={onCopy} /> : <MessageView tab={responseTab} headers={flow.response.headers} body={flow.response.bodyBase64} raw={flow.response.rawHeadBase64} /> : <p>{t("尚无响应")}</p>}</div></section></SplitPane>
    </section>}
    </SplitPane>
    {menu && active && <RequestMenu selectionCount={menuIds.current.length} onSave={() => void save(flows.filter(f => menuIds.current.includes(f.id)))} onDelete={desktop && !fileBusy ? () => requestDelete(menuIds.current) : undefined} target={menu} onClose={() => setMenu(null)} onEdit={onReplay} onReplay={onReplayNow} onCopy={onCopy} replayDisabled={!desktop || busy} />}
  </main>;
}
