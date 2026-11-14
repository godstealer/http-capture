import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { t } from './i18n';
import { cssZoomFactor, menuPosition, UI_ZOOM_EVENT } from './uiZoom';
import './context-menu.css';

export type TabCloseAction = 'current' | 'others' | 'left' | 'right' | 'all';
export interface TabMenuTarget { id: string | null; x: number; y: number; anchor: HTMLElement }

export function tabCloseIds(ids: string[], id: string | null, action: TabCloseAction): string[] {
  if (action === 'all') return ids;
  const index = id === null ? -1 : ids.indexOf(id);
  if (index < 0) return [];
  return ids.filter((value, i) => action === 'current' ? value === id : action === 'others' ? value !== id : action === 'left' ? i < index : i > index);
}

export default function RequestTabMenu({ target, ids, onClose, onCloseTabs }: {
  target: TabMenuTarget; ids: string[]; onClose: () => void; onCloseTabs: (ids: string[]) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState(() => menuPosition(target, { width: 0, height: 0 },
    { width: window.innerWidth, height: window.innerHeight }, cssZoomFactor()));
  useLayoutEffect(() => {
    const rect = ref.current!.getBoundingClientRect();
    setPosition(menuPosition(target, rect, { width: window.innerWidth, height: window.innerHeight }, cssZoomFactor()));
    ref.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
  }, [target]);
  useEffect(() => {
    const outside = (event: PointerEvent) => { if (!ref.current?.contains(event.target as Node)) onClose(); };
    const dismiss = () => onClose();
    const scroll = (event: Event) => { if (!ref.current?.contains(event.target as Node)) onClose(); };
    window.addEventListener('pointerdown', outside);
    window.addEventListener('resize', dismiss);
    window.addEventListener('blur', dismiss);
    window.addEventListener('scroll', scroll, true);
    window.addEventListener(UI_ZOOM_EVENT, dismiss);
    return () => {
      window.removeEventListener('pointerdown', outside);
      window.removeEventListener('resize', dismiss);
      window.removeEventListener('blur', dismiss);
      window.removeEventListener('scroll', scroll, true);
      window.removeEventListener(UI_ZOOM_EVENT, dismiss);
    };
  }, [onClose]);
  const actions: [TabCloseAction, string][] = target.id === null ? [['all', '关闭全部请求标签']]
    : [['current', '关闭当前标签'], ['others', '关闭其他标签'], ['left', '关闭左侧标签'], ['right', '关闭右侧标签'], ['all', '关闭全部请求标签']];
  return createPortal(<div ref={ref} className="request-context-menu" role="menu" aria-label={t('标签操作')} style={position}
    onContextMenu={event => event.preventDefault()} onKeyDown={event => {
      const buttons = Array.from(ref.current!.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'));
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      if (event.key === 'Escape' || event.key === 'Tab') { event.preventDefault(); onClose(); if (target.anchor.isConnected) target.anchor.focus(); }
      if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
        event.preventDefault();
        buttons[event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1
          : (index + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length]?.focus();
      }
    }}>
    <div className="context-caption">{t('标签操作')}</div>
    {actions.map(([action, label]) => {
      const closing = tabCloseIds(ids, target.id, action);
      return <button key={action} role="menuitem" disabled={!closing.length} onClick={() => { onClose(); onCloseTabs(closing); }}>{t(label)}</button>;
    })}
  </div>, document.body);
}
