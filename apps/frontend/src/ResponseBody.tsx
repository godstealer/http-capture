import { t } from './i18n';
import { lazy, Suspense, useEffect, useState } from 'react';
import type { Header } from './types';
const CodeView = lazy(() => import('./CodeView'));

interface Result { raw: string; formatted: string; language: string; error: string; binary?: boolean }
export default function ResponseBody({ body, headers, onCopy }: { body: string; headers: Header[]; onCopy: (text: string) => void }) {
  const [mode, setMode] = useState('pretty');
  const [parser, setParser] = useState('auto');
  const [result, setResult] = useState<Result | null>(null);
  const contentType = headers.find(h => h.name.toLowerCase() === 'content-type')?.value ?? '';
  const encoding = headers.find(h => h.name.toLowerCase() === 'content-encoding')?.value ?? '';
  const canPreview = result?.language === 'html' && !result.binary;
  // Empty sandbox blocks scripts, forms, popups and access to the app origin.
  // Place CSP before untrusted markup so resources cannot phone home.
  const preview = `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; base-uri 'none'; form-action 'none'"><meta charset="utf-8">${result?.raw ?? ''}`;
  useEffect(() => {
    setResult(null);
    const worker = new Worker(new URL('./response.worker.ts', import.meta.url), { type: 'module' });
    const timer = setTimeout(() => { worker.terminate(); setResult({ raw: '', formatted: '', language: 'text', error: t("处理超时，请查看 Base64"), binary: true }); }, 8000);
    worker.onmessage = event => { clearTimeout(timer); setResult(event.data); };
    worker.onerror = () => { clearTimeout(timer); setResult({ raw: '', formatted: '', language: 'text', error: t("正文处理失败，请查看 Base64"), binary: true }); };
    worker.postMessage({ body, contentType, encoding, parser });
    return () => { clearTimeout(timer); worker.terminate(); };
  }, [body, contentType, encoding, parser]);
  const text = mode === 'base64' || result?.binary ? body : mode === 'raw' ? result?.raw ?? '' : result?.formatted ?? '';
  return <div className="formatted-response">
    <div className="format-toolbar"><div>{[['pretty', t("格式化")], ['raw', t("原文")], ['base64', 'Base64']].map(([key, label]) =>
      <button key={key} aria-pressed={mode === key} onClick={() => setMode(key)}>{t(label)}</button>)}</div>
      <button disabled={!canPreview} title={t("HTML 静态预览")} aria-pressed={mode === 'preview'} onClick={() => setMode('preview')}>{t("预览")}</button>
      <select aria-label={t("响应格式")} value={parser} onChange={e => setParser(e.target.value)}>
        <option value="auto">{t("自动识别")}</option><option value="json">JSON</option><option value="html">HTML</option>
        <option value="babel">JavaScript</option><option value="css">CSS</option><option value="text">{t("纯文本")}</option>
      </select><span>{result?.language === 'babel' ? 'JavaScript' : result?.language.toUpperCase()}</span>
      <button disabled={!result && mode !== 'base64'} onClick={() => onCopy(text)}>{t("复制当前内容")}</button>
    </div>
    {result?.error && <div className="format-note" role="status">{t(result.error)}{result.binary ? t("（当前显示 Base64）") : ''}</div>}
    {mode === 'preview' && canPreview ? <div className="response-preview"><div className="format-note">{t("静态 HTML 预览 · 脚本与外部资源已禁用")}</div><iframe title={t("响应 HTML 预览")} sandbox="" referrerPolicy="no-referrer" srcDoc={preview} /></div> : <div className="formatted-scroll">{!result && mode !== 'base64' ? <p>{t("正在处理正文…")}</p> : text
      ? <Suspense fallback={<pre>{text}</pre>}><CodeView text={text} language={mode === 'base64' || result?.binary ? 'text' : result?.language ?? 'text'} /></Suspense> : <p>{t("（空正文）")}</p>}</div>
    }
  </div>;
}
