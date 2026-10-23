import { useEffect, useMemo, useRef, useState } from 'react';
import type { FlowMetadata } from './types';
import { t } from './i18n';
import { parseSse } from './sse';
import { readRecordedSseGrowth } from './recordedSseModel';

/** A metadata-only flow stays on 64 KiB version-pinned reads. Only new bytes
 * cross to the worker; the captured compressed body is never assembled here. */
export default function RecordedSseLive({ flow, encoding, onCopy }: { flow: FlowMetadata; encoding: string; onCopy: (text: string) => void }) {
  const workerRef = useRef<Worker | null>(null);
  const cursorRef = useRef(0);
  const finishedRef = useRef(false);
  const [raw, setRaw] = useState('');
  const [error, setError] = useState('');
  const total = flow.bodyReferences.response?.byteLength ?? 0;
  const events = useMemo(() => parseSse(raw), [raw]);

  useEffect(() => {
    const worker = new Worker(new URL('./sseLive.worker.ts', import.meta.url), { type: 'module' });
    workerRef.current = worker;
    cursorRef.current = 0;
    finishedRef.current = false;
    setRaw('');
    setError('');
    worker.onmessage = event => {
      if (event.data.reset) { setRaw(''); setError(''); return; }
      if (typeof event.data.raw === 'string') setRaw(event.data.raw);
      if (event.data.error) setError(String(event.data.error));
    };
    worker.onerror = () => setError(t('正文处理失败，请查看 Base64'));
    return () => { worker.terminate(); if (workerRef.current === worker) workerRef.current = null; };
  }, [flow.id, encoding]);

  useEffect(() => {
    const worker = workerRef.current;
    if (!worker) return;
    let current = true;
    setError('');
    if (total < cursorRef.current) {
      cursorRef.current = 0;
      finishedRef.current = false;
      worker.postMessage({ chunk: new Uint8Array(), encoding, reset: true });
    }
    void readRecordedSseGrowth(flow, cursorRef.current, {
      isCurrent: () => current && workerRef.current === worker,
      onChunk: (chunk, nextOffset) => {
        worker.postMessage({ chunk, encoding });
        cursorRef.current = nextOffset;
      },
    }).then(() => {
      if (current && flow.state === 'completed' && !finishedRef.current) {
        worker.postMessage({ encoding, finish: true });
        finishedRef.current = true;
      }
    }).catch(reason => { if (current && !(reason instanceof DOMException && reason.name === 'AbortError')) setError(String(reason)); });
    return () => { current = false; };
  }, [flow.id, flow.bodyRevision, flow.state, total, encoding]);

  return <div className="recorded-sse-live">
    <div className="format-toolbar"><strong>{t('SSE 实时事件')}</strong><span>{t('已读取 {v0} 字节压缩正文', { v0: cursorRef.current })}</span><button type="button" disabled={!raw} onClick={() => onCopy(raw)}>{t('复制解压原文')}</button></div>
    {error && <div className="format-note" role="alert">{error}</div>}
    <div className="format-note">{t('仅显示空行结束的事件；下方分页保留原始压缩字节。')}</div>
    <div className="formatted-scroll">
      {events.length ? events.map((event, index) => <details key={index} open><summary>#{index + 1} · {event.event}{event.id ? ` · ID: ${event.id}` : ''}{event.retry ? ` · retry: ${event.retry} ms` : ''}</summary><pre>{event.data}</pre></details>) : <p>{t('等待完整 SSE 事件…')}</p>}
    </div>
  </div>;
}
