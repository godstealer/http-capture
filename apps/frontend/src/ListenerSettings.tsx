import { t } from './i18n';
import { useEffect, useState } from 'react';
import { invoke } from './api';
interface InterfaceAddress { name: string; address: string; family: string; available: boolean; loopback: boolean }
export default function ListenerSettings({ host, port, running, onHost, onPort }: {
  host: string; port: number; running: boolean; onHost: (host: string) => void; onPort: (port: number) => void;
}) {
  const [interfaces, setInterfaces] = useState<InterfaceAddress[]>([]);
  const [refresh, setRefresh] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [manual, setManual] = useState(false);
  useEffect(() => {
    let alive = true;
    setLoading(true); setError('');
    void invoke<InterfaceAddress[]>('network_interfaces').then(items => {
      if (alive) setInterfaces(items);
    }).catch(e => { if (alive) setError(String(e)); }).finally(() => { if (alive) setLoading(false); });
    return () => { alive = false; };
  }, [refresh]);
  const selected = interfaces.find(item => item.address === host);
  const mode = manual ? 'manual' : host === '127.0.0.1' ? 'local' : host === '0.0.0.0' ? 'all' : selected ? `nic:${host}` : 'manual';
  return <div>
    <label className="setting-label">{t("监听范围")}<select aria-label={t("监听范围")} disabled={running} value={mode} onChange={e => {
      const value = e.target.value;
      setManual(value === 'manual');
      if (value !== 'manual') onHost(value === 'local' ? '127.0.0.1' : value === 'all' ? '0.0.0.0' : value.slice(4));
    }}>
      <option value="local">{t("仅本机（默认）")}</option>
      <option value="all">{t("所有 IPv4 网卡（局域网）")}</option>
      <optgroup label={t("本机网卡与地址")}>{interfaces.filter(item => item.address !== '127.0.0.1').map(item => <option key={`${item.name}:${item.address}`} value={`nic:${item.address}`} disabled={!item.available}>
        {item.name} · {item.family} · {item.address}{!item.available ? t("（未连接）") : item.loopback ? t("（回环）") : ''}
      </option>)}</optgroup>
      <option value="manual">{t("手动指定本机 IP")}</option>
    </select></label>
    <button className="text-button" disabled={loading} onClick={() => setRefresh(n => n + 1)}>{loading ? t("正在读取网卡…") : t("刷新网卡列表")}</button>
    {error && <p role="alert">{t("读取网卡失败：")}{error}{t("。仍可手动填写 IP。")}</p>}
    {!loading && !error && !interfaces.length && <p>{t("未发现网卡地址，可刷新或手动填写 IP。")}</p>}
    {mode === 'manual' && <label className="setting-label">{t("本机 IP")}<input aria-label={t("监听 IP")} disabled={running} placeholder={t("例如 192.168.1.10")} value={host} onChange={e => onHost(e.target.value)} /></label>}
    <label className="setting-label">{t("监听端口")}<input aria-label={t("监听端口")} type="number" min="1024" max="65535" disabled={running} value={port} onChange={e => onPort(Math.max(1024, Math.min(65535, Number(e.target.value) || 8080)))} /></label>
    <p className="muted">{t("按所选网卡的 IP 监听；IP 变化后请停止抓包、刷新并重新选择。IPv6 链路本地地址中的 % 后为本机网卡索引，其他设备连接时需使用其自己的接口范围。")}</p>
  </div>;
}
