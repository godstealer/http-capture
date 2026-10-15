import { useEffect, useId, useRef, useState } from 'react';
import { Ban, Plus, RefreshCw, Save, Trash2 } from 'lucide-react';
import { invoke } from './api';
import { t } from './i18n';
import './request-blocking.css';

export interface BlockRule {
  id: string;
  enabled: boolean;
  matchType: 'urlContains' | 'urlRegex' | 'host';
  pattern: string;
  method: string;
}
export interface BlockConfig { enabled: boolean; scope: 'capture' | 'all'; rules: BlockRule[] }

const emptyConfig = (): BlockConfig => ({ enabled: false, scope: 'capture', rules: [] });
const methodToken = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]*$/;

export default function RequestBlocking({ connected, onSaved }: { connected: boolean; onSaved?: () => Promise<void> }) {
  const headingId = useId();
  const methodsId = useId();
  const [saved, setSaved] = useState<BlockConfig | null>(null);
  const [draft, setDraft] = useState<BlockConfig>(emptyConfig);
  const [loading, setLoading] = useState(connected);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [validationAttempted, setValidationAttempted] = useState(false);
  const [reload, setReload] = useState(0);
  const edited = useRef(false);
  const patternInputs = useRef(new Map<string, HTMLInputElement>());
  const methodInputs = useRef(new Map<string, HTMLInputElement>());
  const dirty = saved !== null && JSON.stringify(draft) !== JSON.stringify(saved);
  const locked = !connected || loading || saving || saved === null;
  const enabledRules = draft.rules.filter(rule => rule.enabled).length;

  useEffect(() => {
    if (!connected) { setLoading(false); return; }
    let live = true;
    setLoading(true);
    setError('');
    invoke<BlockConfig>('blocking').then(config => {
      if (!live) return;
      setSaved(config);
      if (!edited.current) setDraft(structuredClone(config));
    }).catch(error => {
      if (live) setError(t('读取阻止规则失败：{v0}', { v0: String(error) }));
    }).finally(() => { if (live) setLoading(false); });
    return () => { live = false; };
  }, [connected, reload]);

  function update(config: BlockConfig) {
    edited.current = saved === null || JSON.stringify(config) !== JSON.stringify(saved);
    setDraft(config);
    setError('');
    setNotice('');
  }
  function updateRule(id: string, patch: Partial<BlockRule>) {
    update({ ...draft, rules: draft.rules.map(rule => rule.id === id ? { ...rule, ...patch } : rule) });
  }
  function cancel() {
    if (saved) setDraft(structuredClone(saved));
    edited.current = false;
    setValidationAttempted(false);
    setError('');
    setNotice('');
  }
  function addRule() {
    update({ ...draft, rules: [...draft.rules, { id: crypto.randomUUID(), enabled: true, matchType: 'urlContains', pattern: '', method: '' }] });
  }
  async function save() {
    setError('');
    setNotice('');
    setValidationAttempted(true);
    const emptyIndex = draft.rules.findIndex(rule => !rule.pattern.trim());
    if (emptyIndex >= 0) {
      setError(t('第 {v0} 条规则需要填写匹配内容。', { v0: emptyIndex + 1 }));
      patternInputs.current.get(draft.rules[emptyIndex].id)?.focus();
      return;
    }
    const invalidMethod = draft.rules.findIndex(rule => !methodToken.test(rule.method));
    if (invalidMethod >= 0) {
      setError(t('第 {v0} 条规则的方法不能包含空格或特殊分隔符。', { v0: invalidMethod + 1 }));
      methodInputs.current.get(draft.rules[invalidMethod].id)?.focus();
      return;
    }
    setSaving(true);
    try {
      const config = structuredClone(draft);
      await invoke('configure_blocking', { config });
      setSaved(config);
      edited.current = false;
      setValidationAttempted(false);
      setNotice(t('阻止规则已保存，下一条请求生效。'));
      try { await onSaved?.(); }
      catch (error) { setError(t('规则已保存，但状态刷新失败：{v0}', { v0: String(error) })); }
    } catch (error) {
      // The server validates the regex dialect. Keep the edited rules visible
      // and leave the last applied configuration unchanged when it rejects one.
      setError(String(error));
    } finally { setSaving(false); }
  }

  return <section className="request-blocking-page" aria-labelledby={headingId}>
    <header className="blocking-heading">
      <div className="blocking-heading-copy">
        <div className="blocking-title"><Ban size={22} /><h1 id={headingId}>{t('阻止请求')}</h1>{saved !== null && <span className={'blocking-status' + (saved.enabled ? ' is-enabled' : '')}>{saved.enabled ? t('已开启') : t('已关闭')}</span>}</div>
        <p>{t('匹配任一启用规则的请求会在本地返回 403，不发送到目标网站。')}</p>
      </div>
      <button type="button" className="blocking-reload" disabled={!connected || loading || saving || dirty} onClick={() => setReload(value => value + 1)}><RefreshCw size={15} />{t('重新加载')}</button>
    </header>
    {!connected && <p className="blocking-message" role="status">{t('连接抓包内核后可设置阻止规则。')}</p>}
    {loading && <p className="blocking-message" role="status">{t('正在读取阻止规则…')}</p>}
    {error && <p className="blocking-message blocking-error" role="alert">{error}</p>}
    {notice && <p className="blocking-message blocking-notice" role="status">{notice}</p>}
    <form className="blocking-form" onSubmit={event => { event.preventDefault(); if (!locked && dirty) void save(); }}>
      <fieldset className="blocking-editor" disabled={locked}>
        <div className="blocking-settings">
          <label className="blocking-switch"><input type="checkbox" checked={draft.enabled} onChange={event => update({ ...draft, enabled: event.target.checked })} /><span>{t('启用请求阻止')}</span></label>
          <label className="blocking-scope">{t('作用范围')}<select value={draft.scope} onChange={event => update({ ...draft, scope: event.target.value as BlockConfig['scope'] })}>
            <option value="capture">{t('仅抓包流量')}</option><option value="all">{t('抓包和手动请求')}</option>
          </select></label>
        </div>
        <p className="blocking-help">{t('保存后对后续请求生效；不会中断正在处理的请求或关闭已建立的连接。')}</p>
        <p className="blocking-help">{t('HTTPS 直通隧道仅支持不限定方法的域名阻止规则。')}</p>
        <div className="blocking-rules-heading"><h2>{t('阻止规则')}</h2><span>{t('已启用 {v0} / {v1}', { v0: enabledRules, v1: draft.rules.length })}</span></div>
        <div className="blocking-rule-list">
          {!draft.rules.length && <div className="blocking-empty"><Ban size={26} /><strong>{t('尚未添加阻止规则')}</strong><p>{t('没有启用的匹配规则时，所有请求都会通过。')}</p></div>}
          {draft.rules.map((rule, index) => <article className={'blocking-rule' + (rule.enabled ? '' : ' is-disabled')} key={rule.id}>
            <div className="blocking-rule-heading">
              <label><input type="checkbox" checked={rule.enabled} onChange={event => updateRule(rule.id, { enabled: event.target.checked })} />{t('启用规则 {v0}', { v0: index + 1 })}</label>
              <button type="button" className="blocking-remove" aria-label={t('删除阻止规则 {v0}', { v0: index + 1 })} title={t('删除规则')} onClick={() => update({ ...draft, rules: draft.rules.filter(item => item.id !== rule.id) })}><Trash2 size={15} /></button>
            </div>
            <div className="blocking-rule-fields">
              <label>{t('匹配方式')}<select aria-label={t('阻止规则匹配方式 {v0}', { v0: index + 1 })} value={rule.matchType} onChange={event => updateRule(rule.id, { matchType: event.target.value as BlockRule['matchType'] })}>
                <option value="urlContains">{t('URL 包含')}</option><option value="urlRegex">{t('URL 正则表达式')}</option><option value="host">{t('域名精确匹配')}</option>
              </select></label>
              <label className="blocking-pattern">{t('匹配内容')}<input ref={input => { if (input) patternInputs.current.set(rule.id, input); else patternInputs.current.delete(rule.id); }} aria-label={t('阻止规则匹配内容 {v0}', { v0: index + 1 })} aria-invalid={validationAttempted && !rule.pattern.trim() ? true : undefined} value={rule.pattern} spellCheck={false} placeholder={rule.matchType === 'host' ? 'example.com' : rule.matchType === 'urlRegex' ? '^https://example\\.com/.*' : 'https://example.com/path'} onChange={event => updateRule(rule.id, { pattern: event.target.value })} /></label>
              <label>{t('方法')}<input ref={input => { if (input) methodInputs.current.set(rule.id, input); else methodInputs.current.delete(rule.id); }} aria-label={t('阻止规则方法 {v0}', { v0: index + 1 })} aria-invalid={validationAttempted && !methodToken.test(rule.method) ? true : undefined} list={methodsId} value={rule.method} maxLength={32} spellCheck={false} placeholder={t('任意方法')} onChange={event => updateRule(rule.id, { method: event.target.value.toUpperCase() })} /></label>
            </div>
            {rule.matchType === 'host' && <p className="blocking-rule-hint">{t('仅精确匹配主机名，不包含协议、端口或 *. 通配符。')}</p>}
            {rule.matchType === 'urlRegex' && <p className="blocking-rule-hint">{t('正则匹配完整 URL 的任意位置；使用 ^ 和 $ 限定边界，不需要 /…/ 分隔符。')}</p>}
          </article>)}
        </div>
        <datalist id={methodsId}>{['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS', 'CONNECT'].map(method => <option key={method} value={method} />)}</datalist>
        <button type="button" className="blocking-add" onClick={addRule}><Plus size={15} />{t('添加规则')}</button>
        <div className="blocking-actions">
          {saved !== null && <span>{dirty ? t('有未应用的修改') : t('已与保存的配置同步')}</span>}
          <button type="button" disabled={!dirty} onClick={cancel}>{t('取消修改')}</button>
          <button type="submit" className="blocking-save" disabled={!dirty}><Save size={15} />{saving ? t('正在保存…') : t('保存规则')}</button>
        </div>
      </fieldset>
    </form>
  </section>;
}
