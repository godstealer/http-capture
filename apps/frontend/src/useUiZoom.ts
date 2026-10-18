import { useEffect } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { createZoomQueue, UI_ZOOM_EVENT, UI_ZOOM_STORAGE_KEY } from './uiZoom';
import './ui-zoom.css';

const applyNativeZoom = createZoomQueue(scale => getCurrentWebview().setZoom(scale));

export function useUiZoom(zoom: number) {
  useEffect(() => {
    const scale = zoom / 100;
    try { localStorage.setItem(UI_ZOOM_STORAGE_KEY, String(zoom)); } catch { /* Session zoom still works. */ }
    // Root-element CSS zoom breaks native caret/selection hit testing in some WebViews.
    document.documentElement.style.removeProperty('zoom');
    if (isTauri()) {
      document.body.style.removeProperty('zoom');
      document.body.style.removeProperty('--ui-viewport-height');
      document.body.style.removeProperty('--ui-viewport-width');
      delete document.body.dataset.uiCssZoom;
      void applyNativeZoom(scale).catch(error => console.error('Unable to set WebView zoom', error));
    } else {
      document.body.style.setProperty('zoom', String(scale));
      document.body.style.setProperty('--ui-viewport-height', `${100 / scale}vh`);
      document.body.style.setProperty('--ui-viewport-width', `${100 / scale}vw`);
      document.body.dataset.uiCssZoom = String(scale);
    }
    window.dispatchEvent(new Event(UI_ZOOM_EVENT));
  }, [zoom]);
}
