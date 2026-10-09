import { useState } from 'react';
import { t } from './i18n';
import { invoke } from './api';
import type { Flow } from './types';

export default function WebSocketFrames({ flow, onCopy }: { flow: Flow; onCopy: (value: string) => void }) {
  const [mode, setMode] = useState('auto');
  const [direction, setDirection] = useState('all');
  const [error, setError] = useState('');
  const session = flow.websocket;
  if (!session) return null;
  const names: Record<number, string> = { 0: 'Continuation', 1: 'Text', 2: 'Binary', 8: 'Close', 9: 'Ping', 10: 'Pong' };
  function payload(frame: NonNullable<Flow['websocket']>['frames'][number]) {
    if (mode === 'base64') return frame.payloadBase64;
    const bytes = Uint8Array.from(atob(frame.payloadBase64), c => c.charCodeAt(0));
    if (mode === 'auto' && !frame.compressed && frame.opcode === 1 && frame.fin) {
      try { return new TextDecoder('utf-8', { fatal: true }).decode(bytes); } catch { /* show bytes */ }
    }
    return Array.from(bytes.slice(0, 65536), (v, i) => `${i && i % 16 === 0 ? '\n' : i ? ' ' : ''}${v.toString(16).padStart(2, '0')}`).join('');
  }
  return <div className="formatted-response"><div className="format-toolbar">
    <strong>WebSocket · {session.state} · {session.frames.length}</strong>
    <select aria-label={t('消息方向')} value={direction} onChange={e => setDirection(e.target.value)}><option value="all">{t('全部')}</option><option value="client">{t('客户端 → 服务端')}</option><option value="server">{t('服务端 → 客户端')}</option></select>
    <select aria-label={t('消息格式')} value={mode} onChange={e => setMode(e.target.value)}><option value="auto">{t('自动识别')}</option><option value="hex">Hex</option><option value="base64">Base64</option></select>
    {['connecting', 'open'].includes(session.state) && <button onClick={() => void invoke('cancel_replay', { executionId: flow.id }).catch(e => setError(String(e)))}>{t('停止连接')}</button>}
  </div>{error && <div className="error-box">{error}</div>}<div className="format-note">{t('逐帧展示；压缩及分片载荷以 Hex/Base64 查看，Hex 最多显示前 64 KiB。')}</div>
  <div className="formatted-scroll">{session.frames.map((frame, i) => direction !== 'all' && frame.direction !== direction ? null : <details key={i} open>
    <summary>#{i + 1} · {frame.direction === 'client' ? '↑' : '↓'} {names[frame.opcode] ?? frame.opcode} · {frame.atMs} ms · FIN={String(frame.fin)} {frame.compressed ? '· compressed' : ''}</summary>
    <button className="text-button" onClick={() => onCopy(frame.payloadBase64)}>{t('复制')} Base64</button><pre>{payload(frame)}</pre>
  </details>)}</div></div>;
}
