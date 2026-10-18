export interface ListenerPreferences { host: string; port: number }

const key = 'http-capture.listener.v1';
const defaults: ListenerPreferences = { host: '127.0.0.1', port: 8080 };

export function parseListenerPort(text: string): number | null {
  if (!/^\d{1,5}$/.test(text)) return null;
  const port = Number(text);
  return port >= 1024 && port <= 65535 ? port : null;
}

// Match the core's IP-literal policy, including numeric scopes for IPv6
// link-local addresses. URL validates IPv6 syntax without accepting hostnames.
export function normalizeListenerHost(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  let host = value.trim();
  if (host.startsWith('[') && host.endsWith(']')) host = host.slice(1, -1);
  if (!host.includes(':')) {
    if (!/^(?:0|[1-9]\d{0,2})(?:\.(?:0|[1-9]\d{0,2})){3}$/.test(host)) return null;
    const octets = host.split('.').map(Number);
    if (octets.some(part => part > 255) || (octets[0] >= 224 && octets[0] <= 239)) return null;
    return host;
  }
  const match = /^([\da-fA-F:.]+)(?:%(\d{1,10}))?$/.exec(host);
  if (!match) return null;
  const scope = match[2] === undefined ? null : Number(match[2]);
  if (scope !== null && scope > 0xffffffff) return null;
  try {
    const normalized = new URL(`http://[${match[1]}]/`).hostname.slice(1, -1);
    const prefix = Number.parseInt(normalized.split(':')[0] || '0', 16);
    if ((prefix & 0xff00) === 0xff00) return null;
    if ((prefix & 0xffc0) === 0xfe80 && !scope) return null;
    return normalized + (scope === null ? '' : `%${scope}`);
  } catch { return null; }
}

export function readListenerPreferences(): ListenerPreferences {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(key) ?? 'null');
    if (stored && typeof stored === 'object' && !Array.isArray(stored)) {
      const candidate = stored as Partial<ListenerPreferences>;
      return {
        host: normalizeListenerHost(candidate.host) ?? defaults.host,
        port: typeof candidate.port === 'number' && Number.isInteger(candidate.port)
          ? parseListenerPort(String(candidate.port)) ?? defaults.port : defaults.port,
      };
    }
  } catch { /* Missing storage or invalid preferences use defaults. */ }
  return { ...defaults };
}

export function saveListenerPreferences(preferences: ListenerPreferences): boolean {
  const host = normalizeListenerHost(preferences.host);
  const port = Number.isInteger(preferences.port) ? parseListenerPort(String(preferences.port)) : null;
  if (host === null || port === null) return false;
  try {
    localStorage.setItem(key, JSON.stringify({ host, port }));
    return true;
  } catch { return false; /* The valid preference remains active for this session. */ }
}
