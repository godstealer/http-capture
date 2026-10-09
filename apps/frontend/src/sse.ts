export interface SseEvent { event: string; id: string; data: string; retry?: string }
/** Parse a complete snapshot, dispatching only blank-line-terminated events. */
export function parseSse(text: string): SseEvent[] {
  const events: SseEvent[] = [];
  let id = '', event = '', data: string[] = [], retry: string | undefined;
  const lines = text.replace(/^\uFEFF/, '').split(/\r\n|\r|\n/);
  lines.pop(); // An unterminated line must not dispatch an event.
  for (const line of lines) {
    if (!line) {
      if (data.length) events.push({ event: event || 'message', id, data: data.join('\n'), retry });
      event = ''; data = []; continue;
    }
    if (line.startsWith(':')) continue;
    const colon = line.indexOf(':');
    const name = colon < 0 ? line : line.slice(0, colon);
    const value = colon < 0 ? '' : line.slice(colon + 1).replace(/^ /, '');
    if (name === 'data') data.push(value);
    else if (name === 'event') event = value;
    else if (name === 'id' && !value.includes('\0')) id = value;
    else if (name === 'retry' && /^\d+$/.test(value)) retry = value;
  }
  return events;
}
