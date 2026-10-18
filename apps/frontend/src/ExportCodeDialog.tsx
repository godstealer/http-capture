import { lazy, Suspense, useEffect, useId, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, Copy, Download, FileCode2, X } from 'lucide-react';
import type { RequestDraft } from './types';
import { codeTargets, generateRequestCode, type CodeWarning } from './codeExport';
import { writeClipboard } from './clipboard';
import { saveCodeFile } from './saveFile';
import { t } from './i18n';
import './code-export.css';
const CodeView = lazy(() => import('./CodeView'));

const warningText = (warning: CodeWarning) => ({
  framing: t('Host、长度和连接控制字段由所选库自动生成。'),
  duplicates: t('此库会合并同名请求头：Cookie 使用分号，其余使用逗号；需要保留重复字段请选择支持的库。'),
  browser: t('浏览器受 CORS 和受限请求头约束；需要完整控制请求可选择 Node.js 或 Python。'),
  execution: t('导出当前 HTTP 请求，不执行脚本；TLS、字段顺序和代理由所选库配置。'),
})[warning];

function errorText(error: unknown) {
  const text = String(error);
  if (/body unsupported|HEAD request with a body|request body for GET or HEAD/.test(text)) return t('此库不支持当前方法携带正文，请选择 Python http.client、Go 或 Node.js Native。');
  if (text.includes('Invalid HTTP URL')) return t('请先填写有效的 HTTP / HTTPS 请求地址。');
  if (text.includes('Invalid HTTP header')) return t('请求头名称或值无效，请检查空名称和换行字符。');
  if (text.includes('Invalid HTTP method')) return t('请求方法无效。');
  if (text.includes('Invalid Base64 body')) return t('请求正文不是有效的 Base64。');
  return t('代码生成失败：{v0}', { v0: text });
}

export default function ExportCodeDialog({ request, onClose }: { request: RequestDraft; onClose: () => void }) {
  const id = useId(), dialog = useRef<HTMLDialogElement>(null);
  const [targetId, setTargetId] = useState(() => {
    try { const saved = localStorage.getItem('http-capture.code-target'); if (codeTargets.some(item => item.id === saved)) return saved!; } catch { /* Session selection. */ }
    return 'python-requests';
  });
  const target = codeTargets.find(item => item.id === targetId)!;
  const [message, setMessage] = useState(''), [failure, setFailure] = useState(''), [busy, setBusy] = useState(false);
  const languages = [...new Set(codeTargets.map(item => item.language))];
  const generated = useMemo(() => {
    try { return { result: generateRequestCode(request, targetId), error: '' }; }
    catch (error) { return { result: null, error: errorText(error) }; }
  }, [request, targetId]);
  useEffect(() => {
    const element = dialog.current!, previous = document.activeElement;
    element.showModal();
    return () => { if (element.open) element.close(); if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  }, []);
  function select(value: string) {
    setTargetId(value); setMessage(''); setFailure('');
    try { localStorage.setItem('http-capture.code-target', value); } catch { /* Session selection. */ }
  }
  async function copy() {
    if (!generated.result) return;
    setFailure('');
    try { await writeClipboard(generated.result.code); setMessage(t('代码已复制')); }
    catch { setFailure(t('复制失败，请选中代码后手动复制。')); }
  }
  async function download() {
    if (!generated.result || busy) return;
    setBusy(true); setFailure(''); setMessage('');
    try { setMessage(await saveCodeFile(generated.result.code, target.filename)); }
    catch (error) { setFailure(t('导出失败：{v0}', { v0: String(error) })); }
    finally { setBusy(false); }
  }
  return createPortal(<dialog className="code-export-dialog" ref={dialog} aria-labelledby={`${id}-title`}
    onCancel={event => { event.preventDefault(); if (!busy) onClose(); }}>
    <header className="code-export-heading"><FileCode2 size={22}/><h2 id={`${id}-title`}>{t('导出代码')}</h2>
      <button aria-label={t('关闭代码导出')} disabled={busy} onClick={onClose}><X size={18}/></button></header>
    <div className="code-export-selectors">
      <label>{t('语言')}<select aria-label={t('代码语言')} value={target.language} disabled={busy} autoFocus
        onChange={event => select(codeTargets.find(item => item.language === event.target.value)!.id)}>
        {languages.map(language => <option key={language}>{language}</option>)}
      </select></label>
      <label>{t('库 / 包')}<select aria-label={t('代码库或包')} value={targetId} disabled={busy} onChange={event => select(event.target.value)}>
        {codeTargets.filter(item => item.language === target.language).map(item => <option key={item.id} value={item.id}>{item.library}</option>)}
      </select></label>
      <code>{target.filename}</code>
    </div>
    <div className="code-export-notes">
      {target.setup && <div><span>{t('依赖安装')}</span><code>{target.setup}</code></div>}
      {generated.result?.warnings.map(warning => <p key={warning}>{warningText(warning)}</p>)}
    </div>
    {generated.error ? <p className="code-export-error" role="alert">{generated.error}</p>
      : <div className="code-export-content" aria-label={t('生成的请求代码')}><Suspense fallback={<pre>{generated.result?.code}</pre>}>
        <CodeView text={generated.result?.code ?? ''} language={target.parser === 'javascript' ? 'babel' : target.parser ?? 'text'} label={t('生成的请求代码')}/>
      </Suspense></div>}
    <footer className="code-export-footer">
      <span role={failure ? 'alert' : 'status'} className={failure ? 'code-export-error' : ''}>{failure || message}</span>
      <button disabled={busy} onClick={onClose}>{t('关闭')}</button>
      <button disabled={!generated.result || busy} onClick={()=>void download()}><Download size={15}/>{busy ? t('正在导出…') : t('下载代码')}</button>
      <button className="code-export-primary" disabled={!generated.result || busy} onClick={()=>void copy()}>{message === t('代码已复制') ? <Check size={15}/> : <Copy size={15}/>}{t('复制代码')}</button>
    </footer>
  </dialog>, document.body);
}
