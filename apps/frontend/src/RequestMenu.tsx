import { t } from './i18n';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { toCurl } from './curl';
import type { Flow } from './types';

export interface MenuTarget { flow: Flow; x: number; y: number; anchor: HTMLElement }
export default function RequestMenu({ target, onClose, onEdit, onReplay, onCopy, replayDisabled, onSave, onDelete, selectionCount = 1 }: {
  selectionCount?: number; onSave?: (flow: Flow) => void; onDelete?: (flow: Flow) => void;
  target: MenuTarget; onClose: () => void; onEdit: (flow: Flow) => void;
  onReplay: (flow: Flow) => void; onCopy: (text: string) => void; replayDisabled: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: target.x, top: target.y });
  useLayoutEffect(() => {
    const rect = ref.current!.getBoundingClientRect();
    setPosition({ left: Math.max(8, Math.min(target.x, window.innerWidth - rect.width - 8)),
      top: Math.max(8, Math.min(target.y, window.innerHeight - rect.height - 8)) });
    ref.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
  }, [target]);
  useEffect(() => {
    const outside = (event: PointerEvent) => { if (!ref.current?.contains(event.target as Node)) onClose(); };
    const dismiss = () => onClose();
    window.addEventListener('pointerdown', outside);
    window.addEventListener('resize', dismiss);
    window.addEventListener('blur', dismiss);
    const scroll = (event: Event) => { if (!ref.current?.contains(event.target as Node)) onClose(); };
    window.addEventListener('scroll', scroll, true);
    return () => {
      window.removeEventListener('pointerdown', outside); window.removeEventListener('resize', dismiss);
      window.removeEventListener('blur', dismiss); window.removeEventListener('scroll', scroll, true);
    };
  }, [onClose]);
  const run = (action: () => void) => { onClose(); action(); };
  const request = target.flow.request;
  return createPortal(<div ref={ref} className="request-context-menu" role="menu" aria-label={t("请求操作")} style={position}
    onContextMenu={e => e.preventDefault()} onKeyDown={e => {
      const buttons = Array.from(ref.current!.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'));
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      if (e.key === 'Escape' || e.key === 'Tab') { e.preventDefault(); onClose(); target.anchor.focus(); }
      if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) {
        e.preventDefault(); buttons[e.key === 'Home' ? 0 : e.key === 'End' ? buttons.length - 1
          : (index + (e.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length]?.focus();
      }
    }}>
    <div className="context-caption">{request.method} {t("· 请求操作")}</div>
    {onSave && <button role="menuitem" onClick={() => run(() => onSave(target.flow))}>{t("保存选中（")}{selectionCount}）</button>}
    {onDelete && <button role="menuitem" onClick={() => run(() => onDelete(target.flow))}>{t("删除选中（")}{selectionCount}）</button>}
    <button role="menuitem" onClick={() => run(() => onEdit(target.flow))}>{t("编辑")}<span>Ctrl+Shift+Enter</span></button>
    <button role="menuitem" disabled={replayDisabled} onClick={() => run(() => onReplay(target.flow))}>{t("直接 Replay")}<span>{t("立即发送")}</span></button>
    <div role="separator" />
    <button role="menuitem" title="Bash / Git Bash" onClick={() => run(() => onCopy(toCurl(request)))}>{t("复制 cURL")}<span>Ctrl+Shift+C</span></button>
    <button role="menuitem" onClick={() => run(() => onCopy(request.url))}>{t("复制 URL")}<span>Ctrl+C</span></button>
    <button role="menuitem" onClick={() => run(() => onCopy(request.headers.map(h => `${h.name}: ${h.value}`).join('\r\n')))}>{t("复制请求头")}</button>
    <button role="menuitem" disabled={!target.flow.rawRequestHeadBase64} onClick={() => run(() => onCopy(atob(target.flow.rawRequestHeadBase64!)))}>{t("复制原始请求头")}</button>
    <button role="menuitem" onClick={() => run(() => onCopy(JSON.stringify(request, null, 2)))}>{t("复制请求 JSON")}</button>
  </div>, document.body);
}
