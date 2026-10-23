import { useEffect, useState } from 'react';
import { invoke } from './api';
import { t } from './i18n';

interface ReverseQuicStatus {
  running: boolean;
  address: string | null;
  target: string | null;
  caPath: string;
}

export default function QuicReverseSettings() {
  const [status, setStatus] = useState<ReverseQuicStatus | null>(null);
  const [target, setTarget] = useState('https://example.com/');
  const [port, setPort] = useState('0');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    let active = true;
    void invoke<ReverseQuicStatus>('reverse_quic_status').then(value => {
      if (!active) return;
      setStatus(value);
      if (value.target) setTarget(value.target);
    }).catch(reason => { if (active) setError(String(reason)); });
    return () => { active = false; };
  }, []);

  async function change() {
    setError('');
    let parsedPort = 0;
    if (!status?.running) {
      try {
        const url = new URL(target);
        if (url.protocol !== 'https:' || !url.hostname || url.username || url.password || url.pathname !== '/' || url.search || url.hash)
          throw new Error(t('请输入 HTTPS 站点地址，不含路径、查询、片段或账号信息。'));
        if (!/^\d+$/.test(port) || Number(port) > 65535)
          throw new Error(t('UDP 端口须为 0–65535 的整数；0 表示自动分配。'));
        parsedPort = Number(port);
      } catch (reason) {
        setError(reason instanceof TypeError ? t('请输入有效的 HTTPS 站点地址。') : String(reason));
        return;
      }
    }
    setBusy(true);
    try {
      const value = status?.running
        ? await invoke<ReverseQuicStatus>('stop_reverse_quic')
        : await invoke<ReverseQuicStatus>('start_reverse_quic', { target, port: parsedPort });
      setStatus(value);
    } catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  }

  return <section className="tun-settings">
    <h3>{t('固定目标 HTTP/3 反向代理')}</h3>
    <p>{t('只监听本机 UDP，将发往所选 HTTPS 站点的 HTTP/3 请求捕获并转发。客户端需将该站点的 QUIC 流量定向到下方端口，并信任本地 CA；不会修改系统代理或路由。')}</p>
    <label>{t('目标站点 HTTPS 地址')}<input aria-label={t('目标站点 HTTPS 地址')} value={target} disabled={busy || !!status?.running} onChange={event => setTarget(event.target.value)} placeholder="https://example.com/" spellCheck={false}/></label>
    <label>{t('本机 UDP 端口（0 为自动分配）')}<input aria-label={t('本机 UDP 端口（0 为自动分配）')} value={port} disabled={busy || !!status?.running} onChange={event => setPort(event.target.value.replace(/\D/g, ''))} inputMode="numeric"/></label>
    {status?.running && <p role="status">{t('正在监听')} <code>{status.address}</code> → <code>{status.target}</code></p>}
    {status && <p>{t('本地 CA 证书')}：<code>{status.caPath}</code></p>}
    {error && <p role="alert">{error}</p>}
    <button className="button" disabled={busy || !status} onClick={() => void change()}>{busy ? t('处理中…') : status?.running ? t('停止 H3 反向代理') : t('启动 H3 反向代理')}</button>
  </section>;
}
