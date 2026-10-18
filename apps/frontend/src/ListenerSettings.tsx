import { t } from './i18n';
import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { invoke } from './api';
import { normalizeListenerHost, parseListenerPort } from './listenerPreferences';
interface InterfaceAddress { name: string; address: string; family: string; available: boolean; loopback: boolean }
export interface ListenerSettingsHandle { commit: () => Promise<boolean> }
const ListenerSettings = forwardRef<ListenerSettingsHandle, {
  host: string; port: number; running: boolean; onHost: (host: string) => void; onPort: (port: number) => void;
}>(function ListenerSettings({ host, port, running, onHost, onPort }, ref) {
  const [interfaces, setInterfaces] = useState<InterfaceAddress[]>([]);
  const [refresh, setRefresh] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [manual, setManual] = useState(false);
  const [hostText, setHostText] = useState(host);
  const [hostError, setHostError] = useState('');
  const [portText, setPortText] = useState(String(port));
  const [portError, setPortError] = useState('');
  const [checkingPort, setCheckingPort] = useState(false);
  const [portAvailable, setPortAvailable] = useState(false);
  const portCheck = useRef(0);
  const pending = useRef<{ host: string; port: number; promise: Promise<boolean> } | null>(null);
  const handle = useRef<ListenerSettingsHandle>({ commit: async () => false });
  useEffect(() => setPortText(String(port)), [port]);
  useEffect(() => { portCheck.current++; pending.current = null; setHostText(host); setHostError(''); setCheckingPort(false); setPortAvailable(false); setPortError(''); }, [host]);
  useEffect(() => { if (running) { portCheck.current++; pending.current = null; setCheckingPort(false); } }, [running]);
  useEffect(() => () => { portCheck.current++; }, []);
  async function commit(checkUnchanged = false): Promise<boolean> {
    if (running) return true;
    const candidateHost = normalizeListenerHost(manual || mode === 'manual' ? hostText : host);
    const value = parseListenerPort(portText);
    if (candidateHost === null) {
      setHostError(t('请输入有效的监听 IP；IPv6 链路本地地址需带 %网卡索引。')); return false;
    }
    setHostError('');
    if (value === null) { setPortError(t('端口必须在 1024–65535 之间')); return false; }
    if (pending.current?.host === candidateHost && pending.current.port === value) return pending.current.promise;
    if (!checkUnchanged && candidateHost === host && value === port && !portError) return true;
    const check = ++portCheck.current;
    setPortError(''); setPortAvailable(false); setCheckingPort(true);
    const promise = (async () => {
      try {
        await invoke('check_listener_port', { host: candidateHost, port: value });
        if (check !== portCheck.current) return false;
        setHostText(candidateHost); setPortText(String(value));
        if (candidateHost !== host) onHost(candidateHost);
        if (value !== port) onPort(value);
        setPortAvailable(true);
        return true;
      } catch (error) {
        if (check === portCheck.current) setPortError(String(error));
        return false;
      } finally {
        if (check === portCheck.current) { pending.current = null; setCheckingPort(false); }
      }
    })();
    pending.current = { host: candidateHost, port: value, promise };
    return promise;
  }
  handle.current.commit = () => commit();
  useImperativeHandle(ref, () => handle.current, []);
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
      portCheck.current++; pending.current = null; setCheckingPort(false); setPortAvailable(false); setPortError(''); setHostError('');
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
    {mode === 'manual' && <label className="setting-label">{t("本机 IP")}<input aria-label={t("监听 IP")} aria-invalid={!!hostError} aria-describedby={hostError ? 'listener-host-status' : undefined} disabled={running} placeholder={t("例如 192.168.1.10")} value={hostText} onChange={e => {
      portCheck.current++; pending.current = null; setCheckingPort(false); setPortAvailable(false); setPortError(''); setHostError(''); setHostText(e.target.value);
    }} onBlur={() => { if (!running) void commit(true); }} onKeyDown={e => { if (e.key === 'Enter') e.currentTarget.blur(); if (e.key === 'Escape') { portCheck.current++; pending.current = null; setHostText(host); setHostError(''); setCheckingPort(false); setPortAvailable(false); } }} /></label>}
    {hostError && <p id="listener-host-status" role="alert">{hostError}</p>}
    <label className="setting-label">{t("监听端口")}<input aria-label={t("监听端口")} aria-invalid={!!portError} aria-describedby="listener-port-status" type="text" inputMode="numeric" pattern="[0-9]*" maxLength={5} disabled={running} value={portText} onChange={e => {
      if (!/^\d*$/.test(e.target.value)) return;
      portCheck.current++; pending.current = null; setCheckingPort(false); setPortAvailable(false); setPortError(''); setPortText(e.target.value);
    }} onBlur={() => { if (!running) void commit(true); }} onKeyDown={e => { if (e.key === 'Enter') e.currentTarget.blur(); if (e.key === 'Escape') { portCheck.current++; pending.current = null; setPortText(String(port)); setPortError(''); setCheckingPort(false); setPortAvailable(false); } }} /></label>
    <div id="listener-port-status" role={portError ? 'alert' : 'status'}>
      {portError || (checkingPort ? t('正在检查端口…') : portAvailable ? t('端口可用；启动时将再次检查。') : t('仅支持 1024–65535，输入完成后自动检查占用。'))}
    </div>
    <p className="muted">{t("按所选网卡的 IP 监听；IP 变化后请停止抓包、刷新并重新选择。IPv6 链路本地地址中的 % 后为本机网卡索引，其他设备连接时需使用其自己的接口范围。")}</p>
  </div>;
});
export default ListenerSettings;
