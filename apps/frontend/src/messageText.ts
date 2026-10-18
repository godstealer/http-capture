import type { Header } from './types';

/** Copy the displayed rows without normalizing names, order, duplicate fields or pseudo headers. */
export function headerLines(headers: readonly Header[]): string {
  return headers.map(header => `${header.name}: ${header.value}`).join('\r\n');
}

/** Raw heads use a one-byte character mapping, including non-UTF-8 field values. */
export function rawHeaderText(raw?: string | null): string | null {
  if (!raw) return null;
  try { return atob(raw); } catch { return null; }
}
