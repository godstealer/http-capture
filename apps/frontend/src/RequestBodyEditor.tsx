import { useEffect, useMemo, useRef, useState } from 'react';
import BodyCodeEditor from './BodyCodeEditor';
import { Check, FileText, MoreHorizontal, Plus, RotateCcw, X } from 'lucide-react';
import type { RequestDraft } from './types';
import { t } from './i18n';
import {
  AUTO_EDIT_LIMIT, bodyByteLength, encodeFormBody, fileRequestBody, inferRequestBodyMode,
  rawBodyParser, rawContentType, readFormBody, requestBodyWorkMatches, requestContentType,
  textBody, utf8Body, withRequestBody,
  type RawBodyParser, type RequestBodyMode, type RequestBodyPart, type RequestBodyUpdateOptions,
} from './requestBody';
import './request-body-editor.css';

type Row = RequestBodyPart & { id: number; kind: 'text' | 'file'; placeholder?: boolean; disabled?: boolean };
type Work = { body: string; mime: string; token: number };
type Busy = 'loading' | 'applying' | 'file' | null;

export default function RequestBodyEditor({ draft, onChange }: {
  draft: RequestDraft; onChange: (draft: RequestDraft) => void;
}) {
  const mime = requestContentType(draft.headers);
  const byteLength = useMemo(() => bodyByteLength(draft.bodyBase64), [draft.bodyBase64]);
  const [mode, setMode] = useState<RequestBodyMode>(() => inferRequestBodyMode(draft.bodyBase64, mime, byteLength));
  const [parser, setParser] = useState<RawBodyParser>(() => rawBodyParser(mime));
  const [parts, setParts] = useState<Row[]>([]);
  const [formReady, setFormReady] = useState(false);
  const [formDirty, setFormDirty] = useState(false);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState<Busy>(null);
  const [fileMime, setFileMime] = useState(mime || 'application/octet-stream');
  const [fileName, setFileName] = useState('');
  const latest = useRef({ draft, onChange });
  latest.current = { draft, onChange };
  const operation = useRef(0), mounted = useRef(true), nextRow = useRef(0);
  const formElement = useRef<HTMLDivElement | null>(null);
  const pendingFieldFocus = useRef<number | null>(null);
  const formDrafts = useRef<Partial<Record<'form' | 'multipart', { body: string; mime: string; parts: Row[]; ready: boolean; dirty: boolean }>>>({});
  const observed = useRef({ body: draft.bodyBase64, mime });
  const ownCommit = useRef<{ body: string; mime: string }>();
  if (observed.current.body !== draft.bodyBase64 || observed.current.mime !== mime) {
    operation.current++;
    observed.current = { body: draft.bodyBase64, mime };
  }
  const text = useMemo(() => mode === 'raw' ? utf8Body(draft.bodyBase64) : null, [mode, draft.bodyBase64]);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; operation.current++; };
  }, []);

  function work(): Work {
    return { body: latest.current.draft.bodyBase64, mime: requestContentType(latest.current.draft.headers), token: ++operation.current };
  }
  function current(job: Work) {
    return mounted.current && requestBodyWorkMatches(job, {
      body: latest.current.draft.bodyBase64, mime: requestContentType(latest.current.draft.headers), token: operation.current,
    });
  }
  function commit(bodyBase64: string, contentType?: string, options?: RequestBodyUpdateOptions) {
    const updated = withRequestBody(latest.current.draft, bodyBase64, contentType, options);
    const next = { body: updated.bodyBase64, mime: requestContentType(updated.headers) };
    operation.current++;
    observed.current = next;
    ownCommit.current = next;
    formDrafts.current = {};
    setFileName('');
    setFileMime(next.mime || 'application/octet-stream');
    latest.current = { ...latest.current, draft: updated };
    latest.current.onChange(updated);
  }
  const rows = (values: RequestBodyPart[]): Row[] => values.map(part => ({
    ...part, id: ++nextRow.current, kind: part.file ? 'file' : 'text',
  }));

  const blankRow = (): Row => ({ id: ++nextRow.current, name: '', value: '', kind: 'text', placeholder: true });
  const formRows = (values: RequestBodyPart[]) => [...rows(values), blankRow()];

  async function loadForm(next: 'form' | 'multipart') {
    const job = work();
    setParts([]); setFormReady(false); setFormDirty(false); setBusy('loading'); setError('');
    try {
      const values = await readFormBody(job.body, job.mime, next);
      if (current(job)) {
        const loaded = formRows(values);
        if (values.length === 0) pendingFieldFocus.current = loaded[0].id;
        setParts(loaded); setFormReady(true);
      }
    } catch (cause) {
      if (current(job)) setError(`${t('无法按此类型载入现有正文，原正文已保留。')}${cause instanceof Error ? ` ${cause.message}` : ''}`);
    } finally { if (current(job)) setBusy(null); }
  }

  useEffect(() => {
    const own = ownCommit.current;
    ownCommit.current = undefined;
    if (own && own.body === draft.bodyBase64 && own.mime === mime) {
      return;
    }
    operation.current++;
    formDrafts.current = {};
    setBusy(null); setError(''); setFileName('');
    setFileMime(mime || 'application/octet-stream');
    const next = inferRequestBodyMode(draft.bodyBase64, mime, byteLength);
    setMode(next); setParser(rawBodyParser(mime));
    if (next === 'form' || next === 'multipart') void loadForm(next);
    else { setParts([]); setFormReady(false); setFormDirty(false); }
  }, [draft.bodyBase64, mime]);

  function choose(next: RequestBodyMode) {
    if (mode === 'form' || mode === 'multipart') {
      formDrafts.current[mode] = { body: latest.current.draft.bodyBase64, mime: requestContentType(latest.current.draft.headers), parts, ready: formReady, dirty: formDirty };
    }
    operation.current++;
    setMode(next); setBusy(null); setError('');
    if (next === 'none') { setParts([]); setFormReady(false); setFormDirty(false); if (latest.current.draft.bodyBase64) commit(''); }
    if (next === 'raw') setParser(rawBodyParser(requestContentType(latest.current.draft.headers)));
    if (next === 'file') setFileMime(requestContentType(latest.current.draft.headers) || 'application/octet-stream');
    if (next === 'form' || next === 'multipart') {
      const cached = formDrafts.current[next];
      if (cached?.ready && cached.body === latest.current.draft.bodyBase64 && cached.mime === requestContentType(latest.current.draft.headers)) {
        setParts(cached.parts); setFormReady(true); setFormDirty(cached.dirty);
      } else void loadForm(next);
    }
  }
  function newForm() {
    operation.current++;
    const first = blankRow();
    pendingFieldFocus.current = first.id;
    setParts([first]); setFormReady(true); setFormDirty(true); setBusy(null); setError('');
  }
  function addField() {
    operation.current++;
    const last = parts[parts.length - 1];
    const part = last?.placeholder ? last : blankRow();
    pendingFieldFocus.current = part.id;
    setParts(values => last?.placeholder ? [...values] : [...values, part]);
  }
  function removeField(id: number) {
    operation.current++;
    const replacement = blankRow();
    const remainingRows = parts.filter(value => value.id !== id);
    pendingFieldFocus.current = (remainingRows[Math.min(parts.findIndex(value => value.id === id), remainingRows.length - 1)] ?? replacement).id;
    setError('');
    setParts(values => {
      const remaining = values.filter(value => value.id !== id);
      return remaining[remaining.length - 1]?.placeholder ? remaining : [...remaining, replacement];
    });
    setFormDirty(true);
  }
  function closeFieldMenu(button: HTMLButtonElement) {
    const menu = button.closest('details');
    menu?.removeAttribute('open');
    menu?.querySelector('summary')?.focus();
  }
  useEffect(() => {
    if (typeof document === 'undefined') return;
    function dismiss(event: PointerEvent | KeyboardEvent) {
      if (event instanceof KeyboardEvent && event.key !== 'Escape') return;
      formElement.current?.querySelectorAll<HTMLDetailsElement>('details[open]').forEach(menu => {
        if (event instanceof PointerEvent && event.target instanceof Node && menu.contains(event.target)) return;
        menu.open = false;
        if (event instanceof KeyboardEvent) menu.querySelector('summary')?.focus();
      });
    }
    document.addEventListener('pointerdown', dismiss);
    document.addEventListener('keydown', dismiss);
    return () => { document.removeEventListener('pointerdown', dismiss); document.removeEventListener('keydown', dismiss); };
  }, []);
  function focusAddedField(id: number, input: HTMLInputElement | null) {
    if (!input || pendingFieldFocus.current !== id) return;
    pendingFieldFocus.current = null;
    input.focus({ preventScroll: true });
    input.closest('.request-body-row')?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }
  function updateRow(id: number, update: Partial<Row>) {
    operation.current++;
    const trailing = blankRow();
    setParts(values => {
      const updated = values.map(part => part.id === id ? { ...part, ...update, placeholder: false } : part);
      return updated[updated.length - 1]?.placeholder ? updated : [...updated, trailing];
    });
    setFormDirty(true);
    setError('');
  }
  async function applyForm() {
    if ((mode !== 'form' && mode !== 'multipart') || !formReady || !formDirty || busy !== null) return;
    if (parts.some(part => !part.disabled && !part.placeholder && part.kind === 'file' && !part.file)) {
      setError(t('请为文件字段选择文件，或将其改为文本字段。'));
      return;
    }
    const job = work();
    setBusy('applying'); setError('');
    try {
      const encoded = await encodeFormBody(parts.filter(part => !part.placeholder && !part.disabled), mode);
      if (current(job)) {
        setBusy(null);
        commit(encoded.bodyBase64, encoded.contentType);
        setFormDirty(false);
      }
    } catch (cause) { if (current(job)) setError(String(cause)); }
    finally { if (current(job)) setBusy(null); }
  }
  async function upload(file: File) {
    const job = work();
    setBusy('file'); setError('');
    try {
      const encoded = await fileRequestBody(file);
      if (current(job)) {
        setBusy(null);
        commit(encoded.bodyBase64, encoded.contentType);
        setFileName(file.name);
      }
    } catch (cause) { if (current(job)) setError(String(cause)); }
    finally { if (current(job)) setBusy(null); }
  }

  return <section className="request-body-panel" data-mode={mode}>
    <div className="request-body-toolbar">
      <label className="request-body-type">{t('正文类型')}<select aria-label={t('正文类型')} value={mode}
        onChange={event => choose(event.target.value as RequestBodyMode)}>
        <option value="none">{t('无正文')}</option>
        <option value="raw">{t('文本 / JSON')}</option>
        <option value="form">x-www-form-urlencoded</option>
        <option value="multipart">Form-data</option>
        <option value="file">{t('原始文件 / 二进制')}</option>
        <option value="base64">{t('Base64（高级）')}</option>
      </select></label>
      {mode === 'raw' && <label className="request-body-type">{t('正文语言')}<select aria-label={t('请求正文格式')} value={parser}
        onChange={event => setParser(event.target.value as RawBodyParser)}>
        <option value="text">Text</option><option value="json">JSON</option>
        <option value="html">HTML</option><option value="xml">XML</option><option value="babel">JavaScript</option><option value="css">CSS</option>
      </select></label>}
      <span className="request-body-size">{byteLength === null ? t('Base64 编码无效') : t('{count} 字节', { count: byteLength })}</span>
      {(mode === 'form' || mode === 'multipart') && <div className="request-body-table-tools">
        <button type="button" className="request-body-tool" disabled={busy !== null || !formReady} title={t('添加字段')} aria-label={t('添加字段')} onClick={addField}><Plus size={16} aria-hidden="true"/><span className="request-body-row-label">{t('添加字段')}</span></button>
        <button type="button" className="request-body-tool" disabled={busy !== null} title={formDirty ? t('放弃修改并重新载入') : t('重新载入正文')} aria-label={formDirty ? t('放弃修改并重新载入') : t('重新载入正文')} onClick={() => void loadForm(mode)}><RotateCcw size={15} aria-hidden="true"/><span className="request-body-row-label">{formDirty ? t('放弃修改并重新载入') : t('重新载入正文')}</span></button>
      </div>}
    </div>
    {mode === 'none' && <div className="request-body-empty"><FileText size={28} aria-hidden="true"/><strong>{t('此请求没有正文')}</strong><p>{draft.method.toUpperCase() === 'GET' && draft.url.split('#')[0].includes('?') ? t('URL 中 ? 后面的内容是查询参数，不属于请求体。') : t('需要发送内容时，选择文本、表单或文件。')}</p><div className="request-body-empty-actions"><button type="button" onClick={() => choose('raw')}>{t('添加文本 / JSON')}</button><button type="button" onClick={() => choose('form')}>{t('添加表单字段')}</button><button type="button" onClick={() => choose('file')}>{t('选择文件正文')}</button></div></div>}
    {mime && mode !== 'form' && mode !== 'multipart' && <div className="request-body-mime"><span>Content-Type</span><code>{mime}</code></div>}

    {mode === 'raw' && (text !== null
      ? <BodyCodeEditor key="raw" formatting parser={parser} value={text} label={t('请求正文')}
          onChange={value => commit(textBody(value), rawContentType(parser, requestContentType(latest.current.draft.headers)))}/>
      : <div className="request-body-notice">
          <p>{t('当前正文包含二进制或无效 UTF-8，不能直接作为文本编辑。')}</p>
          <div className="request-body-actions">
            <button type="button" onClick={() => choose('base64')}>{t('在 Base64 中查看')}</button>
            <button type="button" onClick={() => { setParser('text'); commit('', 'text/plain; charset=utf-8'); }}>{t('替换为空文本')}</button>
          </div>
        </div>)}
    {mode === 'base64' && <>
      <p className="request-body-hint">{t('高级模式直接编辑 Base64；不会对内容执行文本格式化。')}</p>
      <BodyCodeEditor key="base64" formatting={false} parser="text" value={draft.bodyBase64}
        label={t('Base64 请求正文')} onChange={value => commit(value, undefined, { preserveContentEncoding: true })}/>
      {byteLength === null && <p className="request-body-error" role="alert">{t('Base64 编码无效，输入已保留，请修正后再发送。')}</p>}
    </>}

    {(mode === 'form' || mode === 'multipart') && <div ref={formElement} className="request-body-form-card" data-form-mode={mode}>
      {busy === 'loading' && <p className="request-body-hint" role="status">{t('正在载入表单…')}</p>}
      {!formReady && busy !== 'loading' && <div className="request-body-notice">
        <p>{t('现有正文不会被空表单覆盖。要重新构造，请先新建表单。')}</p>
        <button type="button" onClick={newForm}>{t('新建表单')}</button>
      </div>}
      {formReady && <fieldset disabled={busy !== null} className="request-body-form-fields">
        <div className="request-body-row-list">{parts.map((part, index) => <div className="request-body-row" key={part.id} data-placeholder={part.placeholder || undefined} data-disabled={part.disabled || undefined}>
          <label className="request-body-enabled" title={t('启用字段')}><span className="request-body-row-label">{t('启用字段')}</span><input type="checkbox" aria-label={t('启用字段')} checked={!part.placeholder && !part.disabled} disabled={part.placeholder} onChange={event => updateRow(part.id, { disabled: !event.target.checked })}/></label>
          <label className="request-body-name"><span className="request-body-row-label">{t('字段名')}</span><input ref={input => focusAddedField(part.id, input)} aria-label={t('字段名')} placeholder={`${t('字段名')} ${index + 1}`} value={part.name}
            onChange={event => updateRow(part.id, { name: event.target.value })}/></label>
          <label className="request-body-value"><span className="request-body-row-label">{part.kind === 'file' ? t('文件') : t('字段值')}</span>
            {part.kind === 'file' ? <div className="request-body-file-cell">
              <input aria-label={t('上传文件')} type="file" onChange={event => {
                const file = event.target.files?.[0];
                if (file) updateRow(part.id, { file });
                event.currentTarget.value = '';
              }}/>
              {part.file && <small title={part.file.name}>{part.file.name} · {part.file.size} B</small>}
            </div> : <input aria-label={t('字段值')} placeholder={`${t('字段值')} ${index + 1}`} value={part.value}
              onChange={event => updateRow(part.id, { value: event.target.value })}/>}
          </label>
          <details className="request-body-field-menu">
            <summary title={t('字段操作')} aria-label={t('字段操作')} onKeyDown={event => {
              if (event.key !== 'ArrowDown') return;
              event.preventDefault();
              const menu = event.currentTarget.closest('details');
              if (menu) { menu.open = true; menu.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus(); }
            }}><MoreHorizontal size={18} aria-hidden="true"/></summary>
            <div className="request-body-field-options" role="menu" aria-label={t('字段操作')} onKeyDown={event => {
              if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
              event.preventDefault();
              const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
              const currentIndex = items.indexOf(document.activeElement as HTMLButtonElement);
              const index = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1
                : (currentIndex + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
              items[index]?.focus();
            }}>
              {mode === 'multipart' && <>
                <button type="button" role="menuitemradio" aria-checked={part.kind === 'text'} onClick={event => { if (part.kind !== 'text') updateRow(part.id, { kind: 'text', file: undefined }); closeFieldMenu(event.currentTarget); }}><FileText size={14}/>{t('文本')}{part.kind === 'text' && <Check size={13}/>}</button>
                <button type="button" role="menuitemradio" aria-checked={part.kind === 'file'} onClick={event => { if (part.kind !== 'file') updateRow(part.id, { kind: 'file', file: undefined }); closeFieldMenu(event.currentTarget); }}><FileText size={14}/>{t('文件')}{part.kind === 'file' && <Check size={13}/>}</button>
              </>}
              <button type="button" role="menuitem" className="request-body-remove" aria-label={t('移除字段')} disabled={part.placeholder} onClick={event => { removeField(part.id); closeFieldMenu(event.currentTarget); }}><X size={14}/>{t('移除字段')}</button>
            </div>
          </details>
        </div>)}</div>
        <div className={formDirty ? 'request-body-actions request-body-form-footer' : 'request-body-actions request-body-form-footer is-pristine'}>
          <span className={formDirty ? 'request-body-form-status pending' : 'request-body-form-status'} role="status">{formDirty ? t('有修改尚未应用') : t('与当前请求体一致')}</span>
          <button type="button" className="button primary request-body-primary" disabled={!formDirty} title={t('字段修改后点击“应用到请求体”，发送时使用已应用的正文。')} onClick={() => void applyForm()}>
            <Check size={14} aria-hidden="true"/>{busy === 'applying' ? t('正在应用…') : t('应用到请求体')}
          </button>
        </div>
      </fieldset>}
    </div>}

    {mode === 'file' && <div className="request-body-card">
      <strong>{t('原始文件 / 二进制')}</strong>
      {byteLength !== null && byteLength > AUTO_EDIT_LIMIT && <p className="request-body-hint">{t('正文较大，默认不自动解码；可手动切换到文本、表单或 Base64 编辑，现有字节保持不变。')}</p>}
      <p className="request-body-hint">{t('直接将文件原始字节作为请求正文，适合图片、压缩包等文件。')}</p>
      <fieldset disabled={busy !== null} className="request-body-form-fields">
        <label className="request-body-file-picker">{t('选择二进制文件')}
          <input type="file" aria-label={t('选择二进制文件')} onChange={event => {
            const file = event.target.files?.[0];
            if (file) void upload(file);
            event.currentTarget.value = '';
          }}/>
        </label>
        {busy === 'file' && <p role="status">{t('正在读取文件…')}</p>}
        {fileName && <p className="request-body-hint">{t('已载入文件：')}{fileName}</p>}
        <div className="request-body-file-type">
          <label>Content-Type<input aria-label={t('文件 Content-Type')} value={fileMime}
            placeholder="application/octet-stream" onChange={event => setFileMime(event.target.value)}/></label>
          <button type="button" onClick={() => {
            try { setError(''); commit(latest.current.draft.bodyBase64, fileMime.trim() || 'application/octet-stream', { preserveContentEncoding: true }); }
            catch (cause) { setError(String(cause)); }
          }}>{t('应用文件类型')}</button>
        </div>
      </fieldset>
    </div>}
    {error && <p className="request-body-error" role="alert">{error}</p>}
  </section>;
}
