import {useSyncExternalStore} from 'react';
const storageKey = 'http-capture.body-editor.auto-format-paste.v1';
const listeners = new Set<() => void>();
function read() { try { return localStorage.getItem(storageKey) === 'true'; } catch { return false; } }
let enabled = read();
export function autoFormatPasteEnabled() { return enabled; }
export function reloadBodyEditorPreferences(){enabled=read();listeners.forEach(listener=>listener())}
export function setAutoFormatPaste(value: boolean) {
  enabled = value;
  try { localStorage.setItem(storageKey, String(value)); } catch { /* Retain this session's choice. */ }
  listeners.forEach(listener => listener());
}
if (typeof window !== 'undefined') window.addEventListener('storage', event => {
  if (event.key === storageKey || event.key === null) { enabled = read(); listeners.forEach(listener => listener()); }
});
export function useAutoFormatPaste() {
  return useSyncExternalStore(listener => { listeners.add(listener); return () => { listeners.delete(listener); }; }, autoFormatPasteEnabled, () => false);
}
