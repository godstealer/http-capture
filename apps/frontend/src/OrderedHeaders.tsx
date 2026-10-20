import { useId, useState } from 'react';
import { ArrowUp, ArrowDown, X, Plus, Check, Copy, ClipboardPaste, FileText, Split, Merge } from 'lucide-react';
import { t } from './i18n';
import { readClipboard, writeClipboard } from './clipboard';
import { parseHeaderText, serializeHeaders, HeaderTextError } from './headerText';
import { cookieHeaderActions, splitCookieHeader, mergeCookieHeaders } from './cookieHeaders';
import type { Header } from './types';
import './ordered-headers.css';

function replaceEnabled(current: Header[], incoming: Header[]): Header[] {
  const result: Header[] = [];
  let index = 0;
  for (const header of current) {
    if (header.disabled) result.push(header);
    else if (index < incoming.length) result.push(incoming[index++]);
  }
  return [...result, ...incoming.slice(index)];
}

export default function OrderedHeaders({ headers, onChange, context = 'request' }: { headers: Header[]; onChange: (headers: Header[]) => void; context?: 'request' | 'response' }) {
  const response = context === 'response';
  const textId = useId();
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState('');
  const [mode, setMode] = useState('replace');
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const enabled = headers.filter(header => !header.disabled).length;
  function message(error: unknown) {
    if (error instanceof HeaderTextError) return response ? t('第 {v0} 行响应头格式无效，请检查字段名、值或换行。', { v0: error.line }) : t('第 {v0} 行请求头格式无效，请检查字段名、值或换行。', { v0: error.line });
    return String(error);
  }
  function currentText() { return serializeHeaders(headers.filter(header => header.name || header.value)); }
  function openText() {
    setError(''); setNotice('');
    try { setText(currentText()); setMode('replace'); setEditing(true); }
    catch (error) { setError(message(error)); }
  }
  async function paste() {
    setError(''); setNotice(''); setBusy(true); setEditing(true); setMode('replace');
    try { setText(await readClipboard()); }
    catch { setError(response ? t('无法读取剪贴板，请在下方编辑框中手动粘贴响应头。') : t('无法读取剪贴板，请在下方编辑框中手动粘贴请求头。')); }
    finally { setBusy(false); }
  }
  async function copy() {
    setError(''); setNotice('');
    try { await writeClipboard(currentText()); setNotice(response ? t('已复制启用的响应头') : t('已复制启用的请求头')); }
    catch (error) { setError(error instanceof HeaderTextError ? message(error) : t('当前环境无法访问剪贴板')); }
  }
  function apply() {
    setError(''); setNotice('');
    try {
      const parsed = parseHeaderText(text);
      onChange(mode === 'append' ? [...headers, ...parsed.headers] : replaceEnabled(headers, parsed.headers));
      setEditing(false);
      setNotice((response ? t('已应用 {v0} 个响应头', { v0: parsed.headers.length }) : t('已应用 {v0} 个请求头', { v0: parsed.headers.length })) + (parsed.pseudoHeaders.length ? response ? t('；已忽略 {v0} 个 HTTP/2 伪头，状态码保持当前设置。', { v0: parsed.pseudoHeaders.length }) : t('；已忽略 {v0} 个 HTTP/2 伪头，URL 和方法保持当前设置。', { v0: parsed.pseudoHeaders.length }) : ''));
    } catch (error) { setError(message(error)); }
  }
  function update(index: number, patch: Partial<Header>) { onChange(headers.map((header, i) => i === index ? { ...header, ...patch } : header)); }
  function move(index: number, offset: number) {
    const next = [...headers];
    [next[index], next[index + offset]] = [next[index + offset], next[index]];
    onChange(next);
  }
  function changeCookies(index: number, action: 'split' | 'merge') {
    const available = cookieHeaderActions(headers, index);
    if (action === 'split' ? !available.canSplit : !available.canMerge) return;
    const next = action === 'split' ? splitCookieHeader(headers, index) : mergeCookieHeaders(headers, index);
    setError('');
    onChange(next);
    setNotice(action === 'split'
      ? t('已将第 {v0} 行 Cookie 拆分为 {v1} 行，启用状态保持不变。', { v0: index + 1, v1: next.length - headers.length + 1 })
      : t('已合并 {v0} 行同状态 Cookie，启用状态保持不变。', { v0: available.mergeCount }));
  }
  return <div className="ordered-headers">
    <div className="headers-toolbar">
      <span>{t('已启用 {v0} / {v1}', { v0: enabled, v1: headers.length })}</span>
      <div className="headers-toolbar-actions">
        <button type="button" aria-expanded={editing} onClick={openText}><FileText size={14} />{t('文本编辑')}</button>
        <button type="button" disabled={busy} onClick={() => void paste()}><ClipboardPaste size={14} />{t('粘贴导入')}</button>
        <button type="button" onClick={() => void copy()}><Copy size={14} />{response ? t('复制响应头') : t('复制请求头')}</button>
      </div>
    </div>
    {editing && <div className="headers-text-editor">
      <label htmlFor={textId}>{response ? t('响应头文本') : t('请求头文本')}</label>
      <p>{t('支持 Name: value 和浏览器复制的名称、值分行格式；保留重复字段与顺序。')}</p>
      <textarea id={textId} aria-label={response ? t('响应头文本') : t('请求头文本')} disabled={busy} value={text} onChange={event => { setText(event.target.value); setError(''); }} spellCheck={false} wrap="off" placeholder={response ? 'Content-Type: application/json\nCache-Control: no-cache\nX-Test: value' : 'Accept: application/json\nUser-Agent: Mozilla/5.0\nX-Test: value'} />
      <div className="headers-text-actions">
        <select aria-label={response ? t('响应头导入方式') : t('请求头导入方式')} value={mode} onChange={event => setMode(event.target.value)}>
          <option value="replace">{t('替换启用项，保留禁用项')}</option>
          <option value="append">{t('追加到列表')}</option>
        </select>
        <button type="button" className="headers-apply" disabled={busy} onClick={apply}><Check size={14} />{t('应用到列表')}</button>
        <button type="button" onClick={() => { setEditing(false); setError(''); }}>{t('取消')}</button>
      </div>
    </div>}
    {error && <p className="headers-message headers-error" role="alert">{error}</p>}
    {notice && <p className="headers-message" role="status">{notice}</p>}
    <div className="headers-table">
      <div className="table-head">
        <input type="checkbox" aria-label={t('启用全部字段')} title={t('启用或禁用全部字段')} checked={headers.length > 0 && enabled === headers.length} ref={input => { if (input) input.indeterminate = enabled > 0 && enabled < headers.length; }} disabled={!headers.length} onChange={event => onChange(headers.map(header => ({ ...header, disabled: event.target.checked ? undefined : true })))} />
        <span>#</span><span>{t('字段名')}</span><span>{t('值')}</span><span />
      </div>
      <div className="headers-scroll">{headers.map((header, index) => {
        const cookie = header.name.toLowerCase() === 'cookie' ? cookieHeaderActions(headers, index) : undefined;
        return <div className={'header-row' + (header.disabled ? ' header-disabled' : '')} key={index}>
        <input type="checkbox" aria-label={t('启用字段 {v0}', { v0: index + 1 })} checked={!header.disabled} onChange={event => update(index, { disabled: event.target.checked ? undefined : true })} />
        <span className="row-number">{String(index + 1).padStart(2, '0')}</span>
        <div className="header-name-cell">
          <input aria-label={t('字段名 {v0}', { v0: index + 1 })} placeholder={t('字段名')} value={header.name} spellCheck={false} onChange={event => update(index, { name: event.target.value })} />
          {cookie && <div className="cookie-header-actions">
            <button type="button" aria-label={t('拆分 Cookie {v0}', { v0: index + 1 })} title={t('拆分 Cookie {v0}：按分号拆为多行，保留启用状态；需要多个有效键值对。', { v0: index + 1 })} aria-disabled={!cookie.canSplit} onClick={() => changeCookies(index, 'split')}><Split size={13} aria-hidden="true" /></button>
            <button type="button" aria-label={t('合并 Cookie {v0}', { v0: index + 1 })} title={t('合并 Cookie {v0}：至少两行有效 Cookie；仅合并相同启用状态。', { v0: index + 1 })} aria-disabled={!cookie.canMerge} onClick={() => changeCookies(index, 'merge')}><Merge size={13} aria-hidden="true" /></button>
          </div>}
        </div>
        <input aria-label={t('字段值 {v0}', { v0: index + 1 })} placeholder={t('值')} value={header.value} spellCheck={false} onChange={event => update(index, { value: event.target.value })} />
        <div className="row-actions">
          <button type="button" title={t('上移')} aria-label={t('上移字段 {v0}', { v0: index + 1 })} disabled={index === 0} onClick={() => move(index, -1)}><ArrowUp size={13} /></button>
          <button type="button" title={t('下移')} aria-label={t('下移字段 {v0}', { v0: index + 1 })} disabled={index === headers.length - 1} onClick={() => move(index, 1)}><ArrowDown size={13} /></button>
          <button type="button" title={t('删除字段')} aria-label={t('删除字段 {v0}', { v0: index + 1 })} onClick={() => onChange(headers.filter((_, i) => i !== index))}><X size={13} /></button>
        </div>
      </div>})}</div>
    </div>
    <button type="button" className="add-header" onClick={() => onChange([...headers, { name: '', value: '' }])}><Plus size={13} />{response ? t('添加响应头') : t('添加请求头')}</button>
    <div className="field-note"><Check size={12} />{t('保留顺序、重复字段和原始大小写；禁用字段不发送。')}</div>
    {headers.some(header => header.name.toLowerCase() === 'cookie') && <div className="field-note cookie-header-note">{t('Cookie 可拆分或合并；合并仅处理相同启用状态。')}</div>}
  </div>;
}
