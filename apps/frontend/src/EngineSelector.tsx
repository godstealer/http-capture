import { t } from './i18n';
import type { EngineInfo, RequestDraft } from './types';

export default function EngineSelector({ draft, engines, onChange }: {
  draft: RequestDraft; engines: EngineInfo[]; onChange: (draft: RequestDraft) => void;
}) {
  const selected = draft.engine ?? 'auto';
  const engine = engines.find(e => e.id === selected);
  return <div className="engine-selector"><label>{t("发送引擎")}<select aria-label={t("发送引擎")} value={selected} onChange={event => {
    const next = engines.find(e => e.id === event.target.value)!;
    // Retain compatible TLS settings when comparing engines with the same profile.
    onChange({ ...draft, engine: next.id, tls: next.profiles.includes(draft.tls.preset)
      ? draft.tls : { preset: next.profiles[0] ?? 'native' } });
  }}>
    {!engine && <option value={selected}>{selected} {t("· 未知或尚未连接")}</option>}
    {engines.map(e => <option key={e.id} value={e.id} disabled={!e.available}>{e.id === 'auto' ? t("Auto · 优先 h2，兼容 h1") : e.id}{!e.available ? ` · ${e.reason}` : ''}</option>)}
  </select></label><span>{engine?.available ? selected === 'httpcloak' ? t('httpcloak · Windows 浏览器预设 · HTTPS 使用 HTTP/2 · UA 保持不变') : selected === 'h2' ? t("HTTP/2 · 默认 TLS · 按字段列表保序 · 不自动降级") : selected === 'h3' ? t("HTTP/3 实验性引擎 · 默认 TLS · 原始字段顺序暂不保证 · 不自动降级") : t("仅作用于本次请求；TLS 预设单独配置") : engine?.reason ?? t("等待内核提供引擎列表")}</span></div>;
}
