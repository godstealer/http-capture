import type { Header } from './types';

const cookieName = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
const control = /[\x00-\x1f\x7f]/;

function isCookie(header: Header): boolean {
  return header.name.toLowerCase() === 'cookie';
}

function selectedCookie(headers: readonly Header[], index: number): Header | undefined {
  if (!Number.isInteger(index) || index < 0 || index >= headers.length) return undefined;
  const header = headers[index];
  return isCookie(header) ? header : undefined;
}

/** Split only unquoted, unescaped semicolons. Keep the value bytes after '=':
 * no decoding, comma splitting, deduplication or rewriting of quoted values.
 * Ambiguous/malformed input is left to the editor instead of partially changed.
 */
function cookiePairs(value: string): string[] | undefined {
  if (control.test(value)) return undefined;
  const pairs: string[] = [];
  let start = 0;
  let quoted = false;
  let escaped = false;
  const append = (end: number): boolean => {
    // Spaces after a delimiter separate pairs; trailing spaces belong to the
    // supplied value and are retained. Tabs are rejected as control characters.
    const pair = value.slice(start, end).replace(/^ +/, '');
    const equal = pair.indexOf('=');
    if (equal < 1 || !cookieName.test(pair.slice(0, equal))) return false;
    pairs.push(pair);
    return true;
  };
  for (let index = 0; index < value.length; index++) {
    const character = value[index];
    if (escaped) { escaped = false; continue; }
    if (character === '\\') { escaped = true; continue; }
    if (character === '"') { quoted = !quoted; continue; }
    if (character === ';' && !quoted) {
      if (!append(index)) return undefined;
      start = index + 1;
    }
  }
  if (quoted || escaped || !append(value.length)) return undefined;
  return pairs;
}

function mergeGroup(headers: readonly Header[], selected: Header): Header[] {
  return headers.filter(header => isCookie(header) && !!header.disabled === !!selected.disabled);
}

export function cookieHeaderActions(headers: readonly Header[], index: number): {
  canSplit: boolean; canMerge: boolean; mergeCount: number;
} {
  const selected = selectedCookie(headers, index);
  if (!selected) return { canSplit: false, canMerge: false, mergeCount: 0 };
  const group = mergeGroup(headers, selected);
  return {
    canSplit: (cookiePairs(selected.value)?.length ?? 0) > 1,
    canMerge: group.length > 1 && group.every(header => cookiePairs(header.value) !== undefined),
    mergeCount: group.length,
  };
}

/** Expand only the selected row, preserving its name, enabled state and place. */
export function splitCookieHeader(headers: readonly Header[], index: number): Header[] {
  const selected = selectedCookie(headers, index);
  const pairs = selected && cookiePairs(selected.value);
  if (!selected || !pairs || pairs.length < 2) return headers.slice();
  return [
    ...headers.slice(0, index),
    ...pairs.map(value => ({ ...selected, value })),
    ...headers.slice(index + 1),
  ];
}

/** Merge all Cookie rows with the selected row's enabled state at the first
 * matching row, keeping its spelling/state and all unrelated rows in order.
 */
export function mergeCookieHeaders(headers: readonly Header[], index: number): Header[] {
  const selected = selectedCookie(headers, index);
  if (!selected) return headers.slice();
  const group = mergeGroup(headers, selected);
  if (group.length < 2 || group.some(header => cookiePairs(header.value) === undefined)) return headers.slice();
  const first = group[0];
  const value = group.map(header => header.value).join('; ');
  const result: Header[] = [];
  let merged = false;
  for (const header of headers) {
    if (!isCookie(header) || !!header.disabled !== !!selected.disabled) result.push(header);
    else if (!merged) { result.push({ ...first, value }); merged = true; }
  }
  return result;
}
