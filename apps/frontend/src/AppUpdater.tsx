/// <reference types="vite/client" />
import { useEffect, useId, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { Download, RefreshCw, X } from 'lucide-react';
import { invoke } from './api';
import { t } from './i18n';
import { UpdateCheckGate, downloadPercent, mergeUpdateProgress, performUpdateInstallation, shouldOfferUpdate,
  type AppUpdate, type AppUpdateStatus, type UpdateProgress } from './appUpdateState';
import './app-updater.css';

const skippedKey = 'http-capture.updater.skippedVersion';
export interface AppUpdaterDriver {
  desktop: boolean;
  production: boolean;
  status: () => Promise<AppUpdateStatus>;
  check: () => Promise<AppUpdate | null>;
  download: () => Promise<void>;
  install: () => Promise<void>;
  listen: (receive: (event: unknown) => void) => Promise<UnlistenFn>;
}
const nativeDriver: AppUpdaterDriver = {
  desktop: isTauri(),
  production: import.meta.env.PROD,
  status: () => invoke<AppUpdateStatus>('app_update_status'),
  check: () => invoke<AppUpdate | null>('check_app_update'),
  download: () => invoke<void>('download_app_update'),
  install: () => invoke<void>('install_app_update'),
  listen: receive => listen<UpdateProgress>('app-update-progress', event => receive(event.payload)),
};
interface DriverCache { checks: UpdateCheckGate; statusRequest?: Promise<AppUpdateStatus> }
const driverCaches = new WeakMap<AppUpdaterDriver, DriverCache>();
function cacheFor(driver: AppUpdaterDriver): DriverCache {
  let cache = driverCaches.get(driver);
  if (!cache) { cache = { checks: new UpdateCheckGate() }; driverCaches.set(driver, cache); }
  return cache;
}
function readStatus(driver: AppUpdaterDriver, cache: DriverCache) {
  if (!cache.statusRequest) {
    const request = driver.status();
    cache.statusRequest = request;
    void request.catch(() => { if (cache.statusRequest === request) cache.statusRequest = undefined; });
  }
  return cache.statusRequest;
}
function readSkippedVersion(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}
function size(value: number) { return `${(value / 1024 / 1024).toFixed(1)} MB`; }
type InstallState = 'idle' | 'preparing' | 'running' | 'submitted' | 'failed';

export default function AppUpdater({ beforeInstall, onBusyChange, driver }: { beforeInstall?: () => Promise<void>; onBusyChange?: (busy: boolean) => void; driver?: AppUpdaterDriver }) {
  const activeDriver = driver ?? nativeDriver;
  const desktop = activeDriver.desktop;
  const driverCache = cacheFor(activeDriver);
  const checks = driverCache.checks;
  const skipStorageKey = driver ? `${skippedKey}.preview` : skippedKey;
  const titleId = useId();
  const [status, setStatus] = useState<AppUpdateStatus | null>(null);
  const [update, setUpdate] = useState<AppUpdate | null>(null);
  const [open, setOpen] = useState(false);
  const [checking, setChecking] = useState(false);
  const [checked, setChecked] = useState(false);
  const [error, setError] = useState('');
  const [installState, setInstallState] = useState<InstallState>('idle');
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const mounted = useRef(false);
  const installPending = useRef(false);
  const cleanupProgress = useRef<UnlistenFn | undefined>();
  const beforeInstallRef = useRef(beforeInstall);
  beforeInstallRef.current = beforeInstall;
  const onBusyChangeRef = useRef(onBusyChange);
  onBusyChangeRef.current = onBusyChange;
  const skipped = useRef(readSkippedVersion(skipStorageKey));
  const notified = useRef<number | undefined>();
  const dialog = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const busy = installState === 'preparing' || installState === 'running' || installState === 'submitted';
  const percent = downloadPercent(progress);

  async function check(manual: boolean) {
    if (manual) { setOpen(true); setError(''); }
    if (!desktop || installPending.current) return;
    setChecking(true);
    try {
      const current = await readStatus(activeDriver, driverCache);
      if (!mounted.current) return;
      setStatus(current);
      if (!current.configured) { setUpdate(null); setChecked(false); return; }
      const result = await checks.check(activeDriver.check, manual, Date.now());
      if (!mounted.current || !result) return;
      setChecked(true);
      setError('');
      const offered = shouldOfferUpdate(result.update, skipped.current, manual);
      setUpdate(offered ? result.update : null);
      if (offered) {
        if (manual || notified.current !== result.checkedAt) {
          notified.current = result.checkedAt;
          setOpen(true);
        }
      }
    } catch (error) {
      if (mounted.current) {
        setUpdate(null); setChecked(false);
        if (manual) setError(t('检查更新失败：{v0}', { v0: String(error) }));
      }
    } finally { if (mounted.current) setChecking(false); }
  }
  const checkRef = useRef(check);
  checkRef.current = check;

  useEffect(() => {
    mounted.current = true;
    let timer: ReturnType<typeof setInterval> | undefined;
    const automatic = () => {
      if (document.visibilityState === 'visible' && checks.isDue(Date.now()) && !installPending.current) void checkRef.current(false);
    };
    if (desktop && activeDriver.production) {
      // Defer to a microtask so StrictMode's first disposable effect does not
      // start an extra check. The gate also shares any already pending request.
      let live = true;
      queueMicrotask(() => { if (live) automatic(); });
      timer = setInterval(automatic, 60000);
      document.addEventListener('visibilitychange', automatic);
      return () => {
        live = false; mounted.current = false;
        clearInterval(timer); document.removeEventListener('visibilitychange', automatic);
        cleanupProgress.current?.(); cleanupProgress.current = undefined;
        onBusyChangeRef.current?.(false);
      };
    }
    return () => { mounted.current = false; cleanupProgress.current?.(); cleanupProgress.current = undefined; onBusyChangeRef.current?.(false); };
  }, [desktop, activeDriver, checks]);

  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog.current?.focus();
    return () => { if (previous?.isConnected) previous.focus(); else trigger.current?.focus(); };
  }, [open]);

  function skip() {
    if (!update || busy) return;
    skipped.current = update.version;
    try { localStorage.setItem(skipStorageKey, update.version); } catch { /* Session skip still applies. */ }
    setUpdate(null); setOpen(false);
  }
  async function install() {
    if (!desktop || !update || installPending.current) return;
    installPending.current = true;
    onBusyChangeRef.current?.(true);
    setInstallState('running'); setProgress(null); setError('');
    let submitted = false;
    try {
      const unlisten = await activeDriver.listen(event => {
        if (mounted.current && installPending.current) setProgress(previous => mergeUpdateProgress(previous, event));
      });
      if (!mounted.current) { unlisten(); return; }
      cleanupProgress.current?.(); cleanupProgress.current = unlisten;
      submitted = await performUpdateInstallation({
        download: activeDriver.download,
        beforeInstall: async () => {
          if (mounted.current) setInstallState('preparing');
          await beforeInstallRef.current?.();
          if (mounted.current) setInstallState('running');
        },
        install: activeDriver.install,
        active: () => mounted.current,
      });
      // Returning from the command is not proof that the application restarted.
      if (submitted && mounted.current) setInstallState('submitted');
    } catch (error) {
      if (mounted.current) { setInstallState('failed'); setError(t('更新未完成：{v0}', { v0: String(error) })); }
    } finally {
      if (!submitted) {
        installPending.current = false;
        onBusyChangeRef.current?.(false);
        cleanupProgress.current?.(); cleanupProgress.current = undefined;
      }
    }
  }

  const progressText = installState === 'preparing' ? t('正在保存草稿并准备更新…')
    : installState === 'submitted' ? t('安装请求已提交，等待应用重启。')
    : progress?.phase === 'restarting' ? t('正在重启应用…')
    : progress?.phase === 'installing' ? t('正在安装更新…') : progress ? t('正在下载更新…') : t('正在准备下载更新…');

  return <>
    <button ref={trigger} type="button" className={'app-update-trigger' + (update ? ' has-update' : '')} disabled={busy} title={desktop ? t('检查更新') : t('应用内更新仅支持桌面版。')} onClick={() => void check(true)}>
      {update ? <Download size={15} /> : <RefreshCw size={15} />}<span>{update ? t('新版本') : t('检查更新')}</span>
    </button>
    {open && createPortal(<div className="app-update-backdrop" onMouseDown={event => { if (event.target === event.currentTarget && !busy) setOpen(false); }}>
      <div ref={dialog} className="app-update-dialog" role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1} onKeyDown={event => {
        if (event.key === 'Escape' && !busy) { event.preventDefault(); setOpen(false); }
        if (event.key === 'Tab') {
          const buttons = Array.from(dialog.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []);
          if (!buttons.length) { event.preventDefault(); return; }
          const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
          if (index < 0 || (!event.shiftKey && index === buttons.length - 1) || (event.shiftKey && index === 0)) {
            event.preventDefault(); buttons[event.shiftKey ? buttons.length - 1 : 0].focus();
          }
        }
      }}>
        <header><div className="app-update-symbol"><Download size={22} /></div><h2 id={titleId}>{update ? t('发现新版本 {v0}', { v0: update.version }) : t('应用更新')}</h2><button type="button" className="app-update-close" aria-label={t('关闭')} disabled={busy} onClick={() => setOpen(false)}><X size={18} /></button></header>
        {driver && <p className="app-update-demo">{t('更新界面演示')}</p>}
        {status && <p className="app-update-version">{t('当前版本：{v0}', { v0: status.currentVersion })}</p>}
        {!desktop ? <p>{t('应用内更新仅支持桌面版。')}</p>
          : checking ? <p role="status">{t('正在检查更新…')}</p>
          : status && !status.configured ? <p>{t('此构建尚未配置更新服务。')}{status.reason && <span className="app-update-reason">{status.reason}</span>}</p>
          : !update && checked && !error ? <p role="status">{t('当前已是最新版本。')}</p> : null}
        {update && <>
          <p>{t('新版本：{v0}', { v0: update.version })}{update.date && <span className="app-update-date">{Number.isNaN(Date.parse(update.date)) ? update.date : new Date(update.date).toLocaleDateString()}</span>}</p>
          <div className="app-update-notes"><h3>{t('更新说明')}</h3><pre>{update.notes?.trim() ? update.notes : t('此版本未提供更新说明。')}</pre></div>
          {!busy && <p className="app-update-restart-note">{t('下载完成后会保存请求草稿，再安装并重启。')}</p>}
        </>}
        {busy && <div className="app-update-progress" role="status" aria-live="polite"><strong>{progressText}</strong>
          {progress?.phase === 'downloading' && <><progress aria-label={t('更新下载进度')} max={100} {...(percent === undefined ? {} : { value: percent })} /><span>{size(progress.downloaded)}{progress.total !== undefined ? ` / ${size(progress.total)}` : ''}{percent !== undefined ? ` · ${percent}%` : ''}</span></>}
        </div>}
        {error && <p className="app-update-error" role="alert">{error}</p>}
        <footer><button type="button" disabled={busy} onClick={() => setOpen(false)}>{update ? t('稍后') : t('关闭')}</button>
          {update && <><button type="button" disabled={busy || checking} onClick={skip}>{t('跳过此版本')}</button><button type="button" className="app-update-install" disabled={busy || checking} onClick={() => void install()}><Download size={15} />{installState === 'failed' ? t('重试安装') : t('下载并安装')}</button></>}
          {!update && desktop && (!status || status.configured) && error && <button type="button" className="app-update-install" disabled={checking} onClick={() => void check(true)}>{t('重试检查')}</button>}
        </footer>
      </div>
    </div>, document.body)}
  </>;
}
