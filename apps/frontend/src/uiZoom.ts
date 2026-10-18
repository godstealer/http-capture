export const UI_ZOOM_LEVELS = [75, 90, 100, 110, 125, 150] as const;
export const UI_ZOOM_STORAGE_KEY = 'http-capture.zoom';
export const UI_ZOOM_EVENT = 'http-capture:zoom';

export function readUiZoom(storage?: Pick<Storage, 'getItem'>): number {
  try {
    const value = Number((storage ?? localStorage).getItem(UI_ZOOM_STORAGE_KEY));
    return UI_ZOOM_LEVELS.some(level => level === value) ? value : 100;
  } catch { return 100; }
}

/** Native zoom requests must finish in order when the user changes the picker quickly. */
export function createZoomQueue(apply: (scale: number) => Promise<void>) {
  let pending = Promise.resolve();
  return (scale: number): Promise<void> => {
    const result = pending.then(() => apply(scale));
    pending = result.catch(() => {});
    return result;
  };
}

/** Only browser preview uses CSS zoom. Native WebView zoom already normalizes DOM coordinates. */
export function cssZoomFactor(): number {
  const value = Number(document.body.dataset.uiCssZoom);
  return Number.isFinite(value) && value > 0 ? value : 1;
}

export function layoutDelta(screenDelta: number, cssZoom = 1): number {
  return screenDelta / cssZoom;
}

export function menuPosition(point: { x: number; y: number }, size: { width: number; height: number },
  viewport: { width: number; height: number }, cssZoom = 1) {
  const margin = 8;
  const left = Math.max(margin, Math.min(point.x, viewport.width - size.width - margin));
  const top = Math.max(margin, Math.min(point.y, viewport.height - size.height - margin));
  return { left: left / cssZoom, top: top / cssZoom,
    maxWidth: Math.max(0, viewport.width - margin * 2) / cssZoom,
    maxHeight: Math.max(0, viewport.height - margin * 2) / cssZoom };
}
