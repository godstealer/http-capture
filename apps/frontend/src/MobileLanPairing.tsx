import { isTauri } from '@tauri-apps/api/core';
import { QRCodeSVG } from 'qrcode.react';
import { useEffect, useRef, useState } from 'react';
import { invoke } from './api';
import { t } from './i18n';
import './mobile-lan-pairing.css';

type InterfaceAddress = { name: string; address: string; family: string; available: boolean; loopback: boolean };
type GatewayStatus = { running: boolean; address: string | null; paired: boolean; fingerprint: string | null };
const emptyStatus: GatewayStatus = { running: false, address: null, paired: false, fingerprint: null };

function privateIPv4(value: string) {
  const octets = value.split('.').map(Number);
  return octets.length === 4 && octets.every(part => Number.isInteger(part) && part >= 0 && part <= 255)
    && (octets[0] === 10 || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31)
      || (octets[0] === 192 && octets[1] === 168));
}

export default function MobileLanPairing({ active }: { active: boolean }) {
  const [interfaces, setInterfaces] = useState<InterfaceAddress[]>([]);
  const [host, setHost] = useState('');
  const [status, setStatus] = useState<GatewayStatus>(emptyStatus);
  const [uri, setUri] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [now, setNow] = useState(Date.now());
  const activeRef = useRef(active);
  activeRef.current = active;
  const uriRef = useRef(uri);
  uriRef.current = uri;
  const expiry = uri ? Number(new URL(uri).searchParams.get('exp')) * 1000 : 0;
  const remaining = Math.max(0, Math.ceil((expiry - now) / 1000));

  useEffect(() => {
    if (!active || !isTauri()) return;
    let cancelled = false;
    void invoke<InterfaceAddress[]>('network_interfaces').then(list => {
      if (cancelled) return;
      const available = list.filter(item => item.available && !item.loopback && item.family === 'IPv4' && privateIPv4(item.address));
      setInterfaces(available);
      setHost(current => available.some(item => item.address === current) ? current : available[0]?.address ?? '');
    }).catch(cause => { if (!cancelled) setError(String(cause)); });
    const refresh = () => void invoke<GatewayStatus>('mobile_gateway_status').then(next => {
      if (cancelled) return;
      setStatus(next);
      if (next.paired) setUri('');
    }).catch(cause => { if (!cancelled) setError(String(cause)); });
    refresh();
    const poll = window.setInterval(refresh, 2000);
    const clock = window.setInterval(() => setNow(Date.now()), 1000);
    return () => {
      cancelled = true;
      window.clearInterval(poll);
      window.clearInterval(clock);
      if (uriRef.current) void invoke('mobile_gateway_close_pairing', { uri: uriRef.current }).catch(() => undefined);
      setUri('');
    };
  }, [active]);

  async function pair() {
    if (!host || busy) return;
    setBusy(true); setError(''); setUri('');
    try {
      const code = await invoke<string>('mobile_gateway_pair', { host });
      if (!activeRef.current) { await invoke('mobile_gateway_close_pairing', { uri: code }); return; }
      setUri(code);
      setStatus(await invoke<GatewayStatus>('mobile_gateway_status'));
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }

  async function command(name: 'mobile_gateway_disconnect' | 'mobile_gateway_stop') {
    setBusy(true); setError('');
    try {
      await invoke(name);
      setStatus(await invoke<GatewayStatus>('mobile_gateway_status'));
      setUri('');
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }

  return <section className="mobile-lan-pairing settings-section">
    <div className="settings-section-heading"><h3>{t('Android 局域网协同抓包')}</h3></div>
    <p className="settings-section-description">{t('手机连接同一 Wi-Fi 后，扫码建立加密连接。无抓包 CA 时，HTTPS 仅显示连接元数据。')}</p>
    {!isTauri() ? <p>{t('请在桌面应用中使用此功能。')}</p> : <>
      <label className="setting-label">{t('PC 局域网地址')}
        <select value={host} onChange={event => setHost(event.target.value)} disabled={busy || status.paired}>
          {interfaces.length === 0 && <option value="">{t('未找到可用的私有 IPv4 网卡')}</option>}
          {interfaces.map(item => <option key={`${item.name}:${item.address}`} value={item.address}>{item.name} · {item.address}</option>)}
        </select>
      </label>
      <div className="mobile-lan-actions">
        <button className="button primary" type="button" disabled={!host || busy || status.paired} onClick={() => void pair()}>
          {busy ? t('处理中…') : uri ? t('刷新配对码') : t('显示配对二维码')}
        </button>
        {status.paired && <button className="button" type="button" disabled={busy} onClick={() => void command('mobile_gateway_disconnect')}>{t('断开设备')}</button>}
        {status.running && <button className="button" type="button" disabled={busy} onClick={() => void command('mobile_gateway_stop')}>{t('关闭接收端')}</button>}
      </div>
      <p className="mobile-lan-state" aria-live="polite">{status.paired ? t('Android 已配对') : status.running ? t('等待 Android 配对') : t('接收端未启动')}{status.address ? ` · ${status.address}` : ''}</p>
      {status.fingerprint && <p className="mobile-lan-fingerprint">{t('PC 身份指纹')}<code>{status.fingerprint}</code></p>}
      {uri && remaining > 0 && !status.paired && <div className="mobile-lan-code">
        <div className="mobile-lan-qr"><QRCodeSVG value={uri} size={240} level="M" /></div>
        <div><strong>{t('用 Android 版扫描')}</strong><p>{t('二维码仅用于一次配对，剩余 {v0} 秒。', { v0: remaining })}</p>
          <p>{t('请核对手机显示的 PC 地址及身份指纹，确认后再连接。')}</p></div>
      </div>}
      {uri && remaining === 0 && <p role="status">{t('配对码已过期，请刷新。')}</p>}
      {error && <p role="alert" className="mobile-lan-error">{error}</p>}
      <p className="muted">{t('此入口独立于普通抓包代理，不会自动安装 CA、修改防火墙或系统代理。')}</p>
    </>}
  </section>;
}
