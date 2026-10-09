import { invoke, isTauri } from '@tauri-apps/api/core';
import { t } from './i18n';
import { lazy, Suspense, useEffect, useState } from 'react';
import type { Header } from './types';
import { parseSse } from './sse';
const CodeView = lazy(() => import('./CodeView'));

interface Result { raw: string; formatted: string; language: string; error: string; binary?: boolean; kind?: string; mime?: string; blob?: Blob; size?: number; decodedBlob?: Blob }
export default function ResponseBody({ body, headers, onCopy }: { body: string; headers: Header[]; onCopy: (text: string) => void }) {
  const [mode, setMode] = useState('pretty');
  const [parser, setParser] = useState('auto');
  const [result, setResult] = useState<Result | null>(null);
  const [downloadMessage, setDownloadMessage] = useState('');
  async function download(original: boolean) {
    try {
    const blob = original ? new Blob([Uint8Array.from(atob(body), c => c.charCodeAt(0))]) : result?.decodedBlob ?? result?.blob;
    if (!blob) return;
    if (isTauri()) {
      const path = await invoke<string>('save_response_file', { bytes: Array.from(new Uint8Array(await blob.arrayBuffer())), original });
      setDownloadMessage(t('已保存：') + path); return;
    }
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a'); link.href = url; link.download = original ? 'response-original.bin' : 'response-decoded.bin'; link.click();
    setTimeout(() => URL.revokeObjectURL(url), 30000);
    setDownloadMessage(t('已提交浏览器下载'));
    } catch (error) { setDownloadMessage(String(error)); }
  }
  const downloads = <><button onClick={() => download(true)}>{t('下载原始字节')}</button><button disabled={!result?.blob && !result?.decodedBlob} onClick={() => download(false)}>{t('下载解压正文')}</button><span role="status">{downloadMessage}</span></>;
  const [mediaUrl, setMediaUrl] = useState('');
  useEffect(() => {
    if (!result?.blob) { setMediaUrl(''); return; }
    const url = URL.createObjectURL(result.blob); setMediaUrl(url);
    return () => URL.revokeObjectURL(url);
  }, [result]);
  const contentType = headers.find(h => h.name.toLowerCase() === 'content-type')?.value ?? '';
  const encoding = headers.find(h => h.name.toLowerCase() === 'content-encoding')?.value ?? '';
  const isSse = contentType.split(';')[0].trim().toLowerCase() === 'text/event-stream';
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
  if (result?.blob) {
    const media = ['image', 'audio', 'video'].includes(result.kind ?? '');
    const showPreview = media && !['hex', 'base64'].includes(mode);
    const value = mode === 'base64' ? body : result.raw;
    return <div className="formatted-response">
      <div className="format-toolbar">
        <button disabled={!media} aria-pressed={showPreview} onClick={() => setMode('preview')}>{t('预览')}</button>
        <button aria-pressed={!showPreview && mode !== 'base64'} onClick={() => setMode('hex')}>Hex</button>
        <button aria-pressed={mode === 'base64'} onClick={() => setMode('base64')}>Base64</button>
        <span>{result.mime} · {result.size} B</span>
        {downloads}
        <button onClick={() => onCopy(value)}>{t('复制当前内容')}</button>
      </div>
      <div className="format-note">{t('二进制响应：Hex 显示解压正文的前 64 KiB；Base64 与原始下载保留捕获字节。')}</div>
      <div className="formatted-scroll">
        {showPreview && mediaUrl ? <div className="response-media">
          {result.kind === 'image' ? <img src={mediaUrl} alt={t('响应图片')} /> : result.kind === 'video' ? <video src={mediaUrl} controls preload="metadata" /> : <audio src={mediaUrl} controls preload="metadata" />}
          <p>{t('预览需要浏览器支持该媒体编码；无法播放时请下载查看。')}</p>
        </div> : <pre>{value}</pre>}
      </div>
    </div>;
  }
  return <div className="formatted-response">
    <div className="format-toolbar"><div>{[['pretty', t("格式化")], ['raw', t("原文")], ['base64', 'Base64']].map(([key, label]) =>
      <button key={key} aria-pressed={mode === key} onClick={() => setMode(key)}>{t(label)}</button>)}</div>
      <button disabled={!canPreview} title={t("HTML 静态预览")} aria-pressed={mode === 'preview'} onClick={() => setMode('preview')}>{t("预览")}</button>
      <select aria-label={t("响应格式")} value={parser} onChange={e => setParser(e.target.value)}>
        <option value="auto">{t("自动识别")}</option><option value="json">JSON</option><option value="html">HTML</option>
        <option value="babel">JavaScript</option><option value="css">CSS</option><option value="text">{t("纯文本")}</option>
      </select><span>{result?.language === 'babel' ? 'JavaScript' : result?.language.toUpperCase()}</span>
      {downloads}<button disabled={!result && mode !== 'base64'} onClick={() => onCopy(text)}>{t("复制当前内容")}</button>
    </div>
    {result?.error && <div className="format-note" role="status">{t(result.error)}{result.binary ? t("（当前显示 Base64）") : ''}</div>}
    {isSse && mode === 'pretty' && result && !result.binary ? <div className="formatted-scroll"><div className="format-note">{t('SSE 事件 · 收到空行后显示完整事件；原文包含心跳和未完成事件。')}</div>{parseSse(result.raw).map((event, i) => <details key={i} open><summary>#{i + 1} · {event.event}{event.id ? ` · ID: ${event.id}` : ''}{event.retry ? ` · retry: ${event.retry} ms` : ''}</summary><pre>{event.data}</pre></details>)}</div> : mode === 'preview' && canPreview ? <div className="response-preview"><div className="format-note">{t("静态 HTML 预览 · 脚本与外部资源已禁用")}</div><iframe title={t("响应 HTML 预览")} sandbox="" referrerPolicy="no-referrer" srcDoc={preview} /></div> : <div className="formatted-scroll">{!result && mode !== 'base64' ? <p>{t("正在处理正文…")}</p> : text
      ? <Suspense fallback={<pre>{text}</pre>}><CodeView text={text} language={mode === 'base64' || result?.binary ? 'text' : result?.language ?? 'text'} /></Suspense> : <p>{t("（空正文）")}</p>}</div>
    }
  </div>;
}
