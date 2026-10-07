import { t } from './i18n';
import { Children, useRef, useState, type ReactNode, type CSSProperties } from 'react';

/** Ratios survive restarts; each pane scrolls independently of its splitter. */
export default function SplitPane({ children, storageKey, label, direction = 'row', initial = 50,
  min = 15, max = 85, className = '', hidden = false, collapsedSecond = false }: {
  children: ReactNode; storageKey: string; label: string; direction?: 'row' | 'column';
  initial?: number; min?: number; max?: number; className?: string; hidden?: boolean;
  collapsedSecond?: boolean;
}) {
  const clamp = (value: number) => Math.min(max, Math.max(min, value));
  const key = `http-capture.layout.${storageKey}`;
  const [ratio, setRatio] = useState(() => {
    try {
      const stored = localStorage.getItem(key);
      const value = stored === null ? initial : Number(stored);
      return Number.isFinite(value) ? clamp(value) : initial;
    } catch { return initial; }
  });
  const [dragging, setDragging] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const drag = useRef<{ coordinate: number; ratio: number; size: number } | null>(null);
  const update = (value: number) => {
    const next = clamp(value);
    setRatio(next);
    try { localStorage.setItem(key, String(next)); } catch { /* Layout still works without storage. */ }
  };
  const panes = Children.toArray(children);
  return <div ref={root} hidden={hidden} className={`split-pane split-${direction} ${collapsedSecond ? 'second-collapsed' : ''} ${dragging ? 'is-dragging' : ''} ${className}`}
    style={{ '--split-ratio': ratio, '--split-rest': 100 - ratio } as CSSProperties}>
    <div className="split-content">{panes[0]}</div>
    {panes.length > 1 && <><div hidden={collapsedSecond} className="split-handle" role="separator" tabIndex={0}
      aria-label={label} aria-orientation={direction === 'row' ? 'vertical' : 'horizontal'}
      aria-valuemin={min} aria-valuemax={max} aria-valuenow={Math.round(ratio)}
      aria-valuetext={t("前一个面板占 {v0}%", { v0: Math.round(ratio) })}
      title={t("{v0} · 拖动调整，双击恢复默认，方向键微调", { v0: label })}
      onDoubleClick={() => update(initial)}
      onKeyDown={event => {
        const less = direction === 'row' ? 'ArrowLeft' : 'ArrowUp';
        const more = direction === 'row' ? 'ArrowRight' : 'ArrowDown';
        if (![less, more, 'Home', 'End', 'Enter'].includes(event.key)) return;
        event.preventDefault();
        update(event.key === 'Home' ? min : event.key === 'End' ? max : event.key === 'Enter' ? initial
          : ratio + (event.key === less ? -1 : 1) * (event.shiftKey ? 10 : 2));
      }}
      onPointerDown={event => {
        if (event.button !== 0) return;
        const rect = root.current!.getBoundingClientRect();
        const size = (direction === 'row' ? rect.width : rect.height) - 8;
        if (size <= 0) return;
        event.preventDefault();
        event.currentTarget.focus();
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = { coordinate: direction === 'row' ? event.clientX : event.clientY, ratio, size };
        setDragging(true);
      }}
      onPointerMove={event => {
        if (!drag.current) return;
        const coordinate = direction === 'row' ? event.clientX : event.clientY;
        update(drag.current.ratio + (coordinate - drag.current.coordinate) / drag.current.size * 100);
      }}
      onPointerUp={event => { drag.current = null; setDragging(false); event.currentTarget.releasePointerCapture(event.pointerId); }}
      onPointerCancel={() => { drag.current = null; setDragging(false); }}
      onLostPointerCapture={() => { drag.current = null; setDragging(false); }}
    /><div className="split-content">{panes[1]}</div></>}
  </div>;
}
