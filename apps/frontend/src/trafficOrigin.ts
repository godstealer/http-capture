import { t } from './i18n';
import type { Flow } from './types';

export type TrafficOriginView = {
  kind: 'manual' | 'unrecorded' | 'identified' | 'remote' | 'unidentified';
  application: string;
  clientAddress: string;
  entry: string;
  pid: string;
  path: string;
  reason: string;
  imported: boolean;
  provenance: string;
};
type OriginFlow = Pick<Flow, 'source' | 'trafficOrigin' | 'state'>;

function record(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown> : undefined;
}
function text(value: unknown, limit = 4096): string | undefined {
  if (typeof value !== 'string') return undefined;
  const result = value.replace(/[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/g, '\ufffd').trim();
  return result ? result.length > limit ? result.slice(0, limit) + '…' : result : undefined;
}

export function trafficEntryLabel(entry: unknown): string {
  if (entry === 'explicit-proxy') return t('显式代理');
  if (entry === 'tun') return 'TUN';
  if (entry === 'quic-reverse') return t('QUIC 反向代理');
  const value = text(entry, 128);
  return value ? t('未知入口（{value}）', { value }) : t('未记录');
}

/** Process metadata is descriptive data, never markup, a URL or evidence derived
 * from User-Agent. Imported metadata is labelled as an unverified file record.
 */
export function describeTrafficOrigin(flow: OriginFlow): TrafficOriginView {
  const missing = t('未记录');
  const base: TrafficOriginView = {
    kind: 'unrecorded', application: missing, clientAddress: missing, entry: missing,
    pid: '—', path: missing, reason: t('此记录未记录流量来源。'),
    imported: flow.state === 'imported', provenance: '',
  };
  // Replays are new locally initiated operations, even if a malformed/imported
  // record accidentally carries the captured parent's origin.
  if (flow.source === 'replay') return {
    ...base, kind: 'manual', application: t('本应用手动发送'), entry: t('手动发送'),
    reason: t('手动发送没有外部客户端，不继承原请求的进程信息。'), imported: false,
  };
  if (base.imported) base.provenance = t('来源信息来自导入文件，未在本机验证。');
  const origin = record(flow.trafficOrigin);
  if (!origin) return {
    ...base, application: base.imported ? t('导入记录 · 未记录') : missing,
  };
  const result: TrafficOriginView = {
    ...base, kind: 'unidentified', application: t('未识别应用'),
    clientAddress: text(origin.clientAddress, 256) ?? missing,
    entry: trafficEntryLabel(origin.entry),
    reason: t('来源进程状态未识别。'),
  };
  if (origin.processStatus === 'identified') {
    const process = record(origin.process);
    const pid = process?.pid;
    if (typeof pid === 'number' && Number.isSafeInteger(pid) && pid > 0 && pid <= 0xffffffff) {
      return {
        ...result, kind: 'identified', pid: String(pid),
        application: text(process?.name, 256) ?? t('进程 {pid}', { pid }),
        path: text(process?.path) ?? missing,
        reason: base.imported ? t('文件记录的进程信息') : t('已识别来源进程。'),
      };
    }
    result.reason = t('进程信息无效或不完整。');
  } else if (origin.processStatus === 'remote') {
    result.kind = 'remote';
    result.application = t('远程客户端');
    result.reason = t('客户端不在本机，无法识别远程应用。');
  } else if (origin.processStatus === 'unknown') {
    result.reason = t('未找到客户端连接对应的本机进程。');
  } else if (origin.processStatus === 'unsupported') {
    result.reason = t('当前平台不支持识别来源应用。');
  } else if (origin.processStatus === 'transport-unsupported') {
    result.reason = t('当前入口暂不支持识别来源应用。');
  } else if (origin.processStatus === 'tun-unavailable') {
    // A redirected socket can belong to the TUN helper. Never display it as the
    // original process unless the backend explicitly identifies that process.
    result.reason = t('TUN 未提供原始进程信息，不能将辅助进程视为来源应用。');
  }
  return result;
}

export function trafficOriginSearchText(flow: OriginFlow): string {
  const origin = describeTrafficOrigin(flow);
  return [
    origin.application, origin.clientAddress, origin.entry, origin.pid, origin.path,
    origin.reason, origin.provenance,
  ].join(' ').toLowerCase();
}
export function trafficOriginTooltip(origin: TrafficOriginView): string {
  return [
    origin.application, origin.pid !== '—' ? 'PID ' + origin.pid : '',
    origin.clientAddress, origin.entry, origin.path !== t('未记录') ? origin.path : '',
    origin.reason, origin.provenance,
  ].filter(Boolean).join('\n');
}
