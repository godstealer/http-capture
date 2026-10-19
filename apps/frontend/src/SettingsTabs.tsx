import { useLayoutEffect, useRef, type ReactNode } from 'react';
import { t } from './i18n';

export type SettingsPageId = 'general' | 'upstream' | 'system' | 'decryption' | 'tun' | 'migration';
interface SettingsPage { id: SettingsPageId; label: string; icon: ReactNode; content: ReactNode }

export default function SettingsTabs({ pages, active, onChange }: {
  pages: SettingsPage[]; active: SettingsPageId; onChange: (id: SettingsPageId) => void;
}) {
  const body = useRef<HTMLDivElement>(null);
  const buttons = useRef(new Map<SettingsPageId, HTMLButtonElement>());
  const scrollPositions = useRef(new Map<SettingsPageId, number>());
  useLayoutEffect(() => {
    if (body.current) body.current.scrollTop = scrollPositions.current.get(active) ?? 0;
  }, [active]);
  function select(id: SettingsPageId) {
    if (body.current) scrollPositions.current.set(active, body.current.scrollTop);
    onChange(id);
  }

  return <>
    <div className="settings-tabs" role="tablist" aria-label={t('代理设置分类')} onKeyDown={event => {
      const index = pages.findIndex(page => page.id === active);
      const next = event.key === 'ArrowRight' ? (index + 1) % pages.length
        : event.key === 'ArrowLeft' ? (index + pages.length - 1) % pages.length
        : event.key === 'Home' ? 0 : event.key === 'End' ? pages.length - 1 : -1;
      if (next < 0) return;
      event.preventDefault();
      select(pages[next].id);
      buttons.current.get(pages[next].id)?.focus();
    }}>
      {pages.map(page => <button key={page.id} type="button" role="tab" className="settings-tab"
        id={`settings-tab-${page.id}`} aria-controls={`settings-panel-${page.id}`}
        aria-selected={active === page.id} tabIndex={active === page.id ? 0 : -1} autoFocus={active === page.id}
        ref={element => { if (element) buttons.current.set(page.id, element); else buttons.current.delete(page.id); }}
        onClick={() => select(page.id)}>{page.icon}<span>{page.label}</span></button>)}
    </div>
    <div className="settings-dialog-body settings-tab-body" ref={body}
      onScroll={event => scrollPositions.current.set(active, event.currentTarget.scrollTop)}>
      {pages.map(page => <section key={page.id} className="settings-tab-panel" role="tabpanel"
        id={`settings-panel-${page.id}`} aria-labelledby={`settings-tab-${page.id}`}
        hidden={active !== page.id} tabIndex={0}>{page.content}</section>)}
    </div>
  </>;
}
