import { t } from './i18n';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { toCurl } from './curl';
import type { Flow } from './types';
import { cssZoomFactor, menuPosition, UI_ZOOM_EVENT } from './uiZoom';
import { headerLines } from './messageText';

export interface MenuTarget { flow: Flow; x: number; y: number; anchor: HTMLElement }
export default function RequestMenu({ target, onClose, onEdit, onReplay, onCopy, replayDisabled, onSave, onDelete, onInterceptResponse, onBlock, selectionCount = 1 }: {
  onBlock?: (flow: Flow, kind: 'url' | 'host') => void;
  onInterceptResponse?: (flow: Flow) => void;
  selectionCount?: number; onSave?: (flow: Flow) => void; onDelete?: (flow: Flow) => void;
  target: MenuTarget; onClose: () => void; onEdit: (flow: Flow) => void;
  onReplay: (flow: Flow) => void; onCopy: (text: string) => void; replayDisabled: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState(() => menuPosition(target, {width: 0, height: 0},
    {width: window.innerWidth, height: window.innerHeight}, cssZoomFactor()));
  useLayoutEffect(() => {
    const rect = ref.current!.getBoundingClientRect();
    setPosition(menuPosition(target, rect, {width: window.innerWidth, height: window.innerHeight}, cssZoomFactor()));
    ref.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
  }, [target]);
  useEffect(() => {
    const outside = (event: PointerEvent) => { if (!ref.current?.contains(event.target as Node)) onClose(); };
    const dismiss = () => onClose();
    window.addEventListener('pointerdown', outside);
    window.addEventListener('resize', dismiss);
    window.addEventListener('blur', dismiss);
    window.addEventListener(UI_ZOOM_EVENT, dismiss);
    const scroll = (event: Event) => { if (!ref.current?.contains(event.target as Node)) onClose(); };
    window.addEventListener('scroll', scroll, true);
    return () => {
      window.removeEventListener('pointerdown', outside); window.removeEventListener('resize', dismiss);
      window.removeEventListener('blur', dismiss); window.removeEventListener('scroll', scroll, true);
      window.removeEventListener(UI_ZOOM_EVENT, dismiss);
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
    {onInterceptResponse && <button role="menuitem" disabled={!!target.flow.websocket} onClick={() => run(() => onInterceptResponse(target.flow))}>{t('拦截替换响应')}<span>{t('下次请求生效')}</span></button>}
    {onBlock && <><button role="menuitem" disabled={request.method === 'CONNECT'} title={request.method === 'CONNECT' ? t('HTTPS 直通请求请使用域名阻止') : undefined} onClick={() => run(() => onBlock(target.flow, 'url'))}>{t('阻止此 URL')}<span>{t('下次请求生效')}</span></button><button role="menuitem" onClick={() => run(() => onBlock(target.flow, 'host'))}>{t('阻止此域名')}<span>{t('下次请求生效')}</span></button></>}
    <div role="separator" />
    <button role="menuitem" title="Bash / Git Bash" onClick={() => run(() => onCopy(toCurl(request)))}>{t("复制 cURL")}<span>Ctrl+Shift+C</span></button>
    <button role="menuitem" onClick={() => run(() => onCopy(request.url))}>{t("复制 URL")}<span>Ctrl+C</span></button>
    <button role="menuitem" onClick={() => run(() => onCopy(headerLines(request.headers)))}>{t("复制请求头")}</button>
    <button role="menuitem" disabled={!target.flow.rawRequestHeadBase64} onClick={() => run(() => onCopy(atob(target.flow.rawRequestHeadBase64!)))}>{t("复制原始请求头")}</button>
    <button role="menuitem" onClick={() => run(() => onCopy(JSON.stringify(request, null, 2)))}>{t("复制请求 JSON")}</button>
  </div>, document.body);
}
