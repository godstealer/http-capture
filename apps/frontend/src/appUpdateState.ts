export const UPDATE_CHECK_INTERVAL = 6 * 60 * 60 * 1000;
export interface AppUpdateStatus { currentVersion: string; configured: boolean; reason?: string }
export interface AppUpdate { version: string; currentVersion: string; notes?: string; date?: string }
export interface UpdateCheckResult { update: AppUpdate | null; checkedAt: number }
export interface UpdateProgress { phase: 'downloading' | 'installing' | 'restarting'; downloaded: number; total?: number }

/** Share a pending check across StrictMode mounts and manual/automatic callers. */
export class UpdateCheckGate {
  private pending?: Promise<UpdateCheckResult>;
  private lastAttempt?: number;
  private result?: UpdateCheckResult;

  isDue(now: number): boolean {
    return this.lastAttempt === undefined || now < this.lastAttempt || now - this.lastAttempt >= UPDATE_CHECK_INTERVAL;
  }

  check(run: () => Promise<AppUpdate | null>, manual: boolean, now: number): Promise<UpdateCheckResult | undefined> {
    if (this.pending) return this.pending;
    if (!manual && !this.isDue(now)) return Promise.resolve(this.result);
    this.lastAttempt = now;
    const pending = Promise.resolve().then(run).then(update => {
      const result = { update, checkedAt: now };
      this.result = result;
      return result;
    }, error => {
      // The native check invalidates its pending offer before querying again.
      // Never replay an offer whose download token may no longer exist.
      this.result = undefined;
      throw error;
    });
    this.pending = pending;
    // Both handlers consume the cleanup branch, including rejected checks.
    void pending.then(() => { if (this.pending === pending) this.pending = undefined; },
      () => { if (this.pending === pending) this.pending = undefined; });
    return pending;
  }
}

export function shouldOfferUpdate(update: AppUpdate | null, skippedVersion: string | null, manual: boolean): boolean {
  return !!update && update.version !== update.currentVersion && (manual || update.version !== skippedVersion);
}

const phaseOrder = { downloading: 0, installing: 1, restarting: 2 };
const bytes = (value: unknown): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;

/** Ignore malformed/late events instead of regressing or displaying NaN percent. */
export function mergeUpdateProgress(previous: UpdateProgress | null, event: unknown): UpdateProgress | null {
  if (!event || typeof event !== 'object') return previous;
  const next = event as Partial<UpdateProgress>;
  if (!next.phase || !Object.prototype.hasOwnProperty.call(phaseOrder, next.phase) || !bytes(next.downloaded)) return previous;
  if (previous && phaseOrder[next.phase] < phaseOrder[previous.phase]) return previous;
  const downloaded = Math.max(previous?.downloaded ?? 0, next.downloaded);
  const receivedTotal = bytes(next.total) && next.total > 0 ? next.total : undefined;
  const knownTotal = receivedTotal ?? previous?.total;
  // A mismatched Content-Length is no longer a trustworthy progress denominator.
  const total = knownTotal !== undefined && knownTotal >= downloaded ? knownTotal : undefined;
  return { phase: next.phase, downloaded, ...(total === undefined ? {} : { total }) };
}

export function downloadPercent(progress: UpdateProgress | null): number | undefined {
  if (!progress?.total || !bytes(progress.downloaded) || !bytes(progress.total) || progress.downloaded > progress.total) return undefined;
  return Math.min(100, Math.max(0, Math.floor(progress.downloaded / progress.total * 100)));
}

/** Flush the latest drafts only after the verified download is ready to install. */
export async function performUpdateInstallation(actions: {
  download: () => Promise<void>;
  beforeInstall?: () => Promise<void>;
  install: () => Promise<void>;
  active: () => boolean;
}): Promise<boolean> {
  await actions.download();
  if (!actions.active()) return false;
  await actions.beforeInstall?.();
  if (!actions.active()) return false;
  await actions.install();
  return true;
}
