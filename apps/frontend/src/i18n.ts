import { useSyncExternalStore } from 'react';
import english from './locales/en.json';
export type LanguagePreference = 'system' | 'zh-CN' | 'en';
const storageKey = 'http-capture.language';
const listeners = new Set<() => void>();
function readPreference(): LanguagePreference {
  try { const value = localStorage.getItem(storageKey); return value === 'zh-CN' || value === 'en' ? value : 'system'; }
  catch { return 'system'; }
}
let preference = readPreference();
export function resolveLanguage(value: LanguagePreference, system = typeof navigator === 'undefined' ? 'en' : navigator.language): 'zh-CN' | 'en' {
  return value === 'system' ? (/^zh(?:-|$)/i.test(system) ? 'zh-CN' : 'en') : value;
}
let language = resolveLanguage(preference);
function update() {
  language = resolveLanguage(preference);
  if (typeof document !== 'undefined') document.documentElement.lang = language;
  listeners.forEach(listener => listener());
}
export function setLanguage(value: LanguagePreference) {
  preference = value;
  try { localStorage.setItem(storageKey, value); } catch { /* Session choice still works. */ }
  update();
}
if (typeof window !== 'undefined') {
window.addEventListener('languagechange', update);
window.addEventListener('storage', e => { if (e.key === storageKey || e.key === null) { preference = readPreference(); update(); } });
}
if (typeof document !== 'undefined') document.documentElement.lang = language;
export function useLanguage() {
  useSyncExternalStore(callback => { listeners.add(callback); return () => { listeners.delete(callback); }; }, () => `${preference}:${language}`, () => `${preference}:${language}`);
  return { preference, language, setLanguage };
}
export function t(text: string, values?: Record<string, string | number>): string {
  const translated = language === 'en' ? (Object.prototype.hasOwnProperty.call(english, text) ? (english as Record<string, string>)[text] : text) : text;
  return values ? translated.replace(/\{(\w+)\}/g, (match, name) => String(values[name] ?? match)) : translated;
}

export function currentLanguage() { return language; }
