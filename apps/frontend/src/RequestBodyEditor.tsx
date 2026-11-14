import { useEffect, useMemo, useRef, useState } from 'react';
import BodyCodeEditor from './BodyCodeEditor';
import { Check, FileText, Plus, RotateCcw, X } from 'lucide-react';
import type { RequestDraft } from './types';
import { t } from './i18n';
import {
  AUTO_EDIT_LIMIT, bodyByteLength, encodeFormBody, fileRequestBody, inferRequestBodyMode,
  rawBodyParser, rawContentType, readFormBody, requestBodyWorkMatches, requestContentType,
  textBody, utf8Body, withRequestBody,
  type RawBodyParser, type RequestBodyMode, type RequestBodyPart, type RequestBodyUpdateOptions,
} from './requestBody';
import './request-body-editor.css';

type Row = RequestBodyPart & { id: number; kind: 'text' | 'file'; placeholder?: boolean };
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
  const formRows = (values: RequestBodyPart[]) => values.length ? rows(values) : [blankRow()];

  async function loadForm(next: 'form' | 'multipart') {
    const job = work();
    setParts([]); setFormReady(false); setFormDirty(false); setBusy('loading'); setError('');
    try {
      const values = await readFormBody(job.body, job.mime, next);
      if (current(job)) { setParts(formRows(values)); setFormReady(true); }
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
    setParts([blankRow()]); setFormReady(true); setFormDirty(true); setBusy(null); setError('');
  }
  function addField() {
    operation.current++;
    const part = blankRow();
    pendingFieldFocus.current = part.id;
    setParts(values => [...values, part]);
  }
  function focusAddedField(id: number, input: HTMLInputElement | null) {
    if (!input || pendingFieldFocus.current !== id) return;
    pendingFieldFocus.current = null;
    input.focus({ preventScroll: true });
    input.closest('.request-body-row')?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }
  function updateRow(id: number, update: Partial<Row>) {
    operation.current++;
    setParts(values => values.map(part => part.id === id ? { ...part, ...update, placeholder: false } : part));
    setFormDirty(true);
    setError('');
  }
  async function applyForm() {
    if ((mode !== 'form' && mode !== 'multipart') || !formReady || !formDirty || busy !== null) return;
    if (parts.some(part => part.kind === 'file' && !part.file)) {
      setError(t('请为文件字段选择文件，或将其改为文本字段。'));
      return;
    }
    const job = work();
    setBusy('applying'); setError('');
    try {
      const encoded = await encodeFormBody(parts.filter(part => !part.placeholder), mode);
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

  return <section className="request-body-panel">
    <div className="request-body-toolbar">
      <label>{t('正文类型')}<select aria-label={t('正文类型')} value={mode}
        onChange={event => choose(event.target.value as RequestBodyMode)}>
        <option value="none">{t('无正文')}</option>
        <option value="raw">{t('文本 / JSON')}</option>
        <option value="form">{t('URL 编码表单')}</option>
        <option value="multipart">{t('Multipart 表单 / 文件')}</option>
        <option value="file">{t('原始文件 / 二进制')}</option>
        <option value="base64">{t('Base64（高级）')}</option>
      </select></label>
      {mode === 'raw' && <label>{t('正文语言')}<select aria-label={t('请求正文格式')} value={parser}
        onChange={event => setParser(event.target.value as RawBodyParser)}>
        <option value="text">Text</option><option value="json">JSON</option>
        <option value="html">HTML</option><option value="xml">XML</option><option value="babel">JavaScript</option><option value="css">CSS</option>
      </select></label>}
      <span className="request-body-size">{byteLength === null ? t('Base64 编码无效') : t('{count} 字节', { count: byteLength })}</span>
    </div>
    {mode === 'none' && <div className="request-body-empty"><FileText size={28} aria-hidden="true"/><strong>{t('此请求没有正文')}</strong><p>{draft.method.toUpperCase() === 'GET' && draft.url.split('#')[0].includes('?') ? t('URL 中 ? 后面的内容是查询参数，不属于请求体。') : t('需要发送内容时，选择文本、表单或文件。')}</p><div className="request-body-empty-actions"><button type="button" onClick={() => choose('raw')}>{t('添加文本 / JSON')}</button><button type="button" onClick={() => choose('form')}>{t('添加表单字段')}</button><button type="button" onClick={() => choose('file')}>{t('选择文件正文')}</button></div></div>}
    {mime && <div className="request-body-mime"><span>Content-Type</span><code>{mime}</code></div>}

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

    {(mode === 'form' || mode === 'multipart') && <div className="request-body-card request-body-form-card" data-form-mode={mode}>
      <div className="request-body-card-heading">
        <div className="request-body-form-title"><strong>{t('表单字段')}</strong><span>{parts.filter(part => !part.placeholder).length}</span></div>
        <button type="button" className="request-body-reload" disabled={busy !== null} onClick={() => void loadForm(mode)}><RotateCcw size={13} aria-hidden="true"/>{formDirty ? t('放弃修改并重新载入') : t('重新载入正文')}</button>
      </div>
      <p className="request-body-form-help">{mode === 'form' ? t('支持重复字段，按表格顺序发送。') : t('文本与文件可混合添加，支持重复字段。')}</p>
      {busy === 'loading' && <p role="status">{t('正在载入表单…')}</p>}
      {!formReady && busy !== 'loading' && <div className="request-body-notice">
        <p>{t('现有正文不会被空表单覆盖。要重新构造，请先新建表单。')}</p>
        <button type="button" onClick={newForm}>{t('新建表单')}</button>
      </div>}
      {formReady && <fieldset disabled={busy !== null} className="request-body-form-fields">
        {parts.length === 0 && <p className="request-body-hint">{t('没有表单字段，应用后将发送空表单。')}</p>}
        <div className="request-body-table-head" aria-hidden="true"><span>#</span><span>{t('字段名')}</span>{mode === 'multipart' && <span>{t('字段类型')}</span>}<span>{t('字段值')}</span><span/></div>
        <div className="request-body-row-list">{parts.map((part, index) => <div className="request-body-row" key={part.id}>
          <span className="request-body-row-index" aria-hidden="true">{String(index + 1).padStart(2, '0')}</span>
          <label className="request-body-name"><span className="request-body-row-label">{t('字段名')}</span><input ref={input => focusAddedField(part.id, input)} aria-label={t('字段名')} placeholder={t('字段名')} value={part.name}
            onChange={event => updateRow(part.id, { name: event.target.value })}/></label>
          {mode === 'multipart' && <label className="request-body-kind"><span className="request-body-row-label">{t('字段类型')}</span>
            <select aria-label={t('字段类型')} value={part.kind} onChange={event => updateRow(part.id,
              { kind: event.target.value as Row['kind'], file: undefined })}>
              <option value="text">{t('文本')}</option><option value="file">{t('文件')}</option>
            </select>
          </label>}
          <label className="request-body-value"><span className="request-body-row-label">{part.kind === 'file' ? t('文件') : t('字段值')}</span>
            {part.kind === 'file' ? <>
              <input aria-label={t('上传文件')} type="file" onChange={event => {
                const file = event.target.files?.[0];
                if (file) updateRow(part.id, { file });
                event.currentTarget.value = '';
              }}/>
              {part.file && <small>{part.file.name} · {part.file.size} B · {part.file.type || 'application/octet-stream'}</small>}
            </> : <input aria-label={t('字段值')} placeholder={t('字段值')} value={part.value}
              onChange={event => updateRow(part.id, { value: event.target.value })}/>}
          </label>
          <button type="button" className="request-body-remove" aria-label={t('移除字段')} title={t('移除字段')} onClick={() => {
            operation.current++; setParts(values => values.filter(value => value.id !== part.id)); setFormDirty(true);
          }}><X size={14} aria-hidden="true"/></button>
        </div>)}</div>
        <button type="button" className="request-body-add-field" onClick={addField}><Plus size={15} aria-hidden="true"/>{t('添加字段')}</button>
        <div className="request-body-actions request-body-form-footer">
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
