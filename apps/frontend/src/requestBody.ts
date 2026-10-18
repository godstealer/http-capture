import type { Header, RequestDraft } from './types';

export type RequestBodyMode = 'raw' | 'form' | 'multipart' | 'file' | 'base64';
export type RawBodyParser = 'text' | 'json' | 'html' | 'babel' | 'css';
export type RequestBodyPart = { name: string; value: string; file?: File };
export type EncodedRequestBody = { bodyBase64: string; contentType: string };
export type RequestBodyUpdateOptions = { preserveContentEncoding?: boolean };

export function requestContentType(headers: Header[]): string {
  return headers.find(header => !header.disabled && header.name.toLowerCase() === 'content-type')?.value ?? '';
}
export function contentTypeName(value: string): string {
  return value.split(';', 1)[0].trim().toLowerCase();
}
export function bodyBytes(encoded: string) {
  return Uint8Array.from(atob(encoded), character => character.charCodeAt(0));
}
export function bytesBase64(bytes: Uint8Array): string {
  const chunks: string[] = [];
  for (let offset = 0; offset < bytes.length; offset += 32768) {
    chunks.push(String.fromCharCode(...bytes.subarray(offset, offset + 32768)));
  }
  return btoa(chunks.join(''));
}
export function utf8Body(encoded: string): string | null {
  try { return new TextDecoder('utf-8', { fatal: true }).decode(bodyBytes(encoded)); }
  catch { return null; }
}
export function textBody(text: string): string {
  return bytesBase64(new TextEncoder().encode(text));
}
export function rawBodyParser(mime: string): RawBodyParser {
  const type = contentTypeName(mime);
  if (type === 'application/json' || type.endsWith('+json')) return 'json';
  if (type === 'text/html' || type === 'application/xhtml+xml') return 'html';
  if (['text/javascript', 'application/javascript', 'application/x-javascript', 'text/jsx'].includes(type)) return 'babel';
  if (type === 'text/css') return 'css';
  return 'text';
}
export function inferRequestBodyMode(bodyBase64: string, mime: string): RequestBodyMode {
  const type = contentTypeName(mime);
  if (type === 'application/x-www-form-urlencoded') return 'form';
  if (type === 'multipart/form-data') return 'multipart';
  if (bodyBase64 && ((type && !type.startsWith('text/') && rawBodyParser(mime) === 'text'
      && !type.endsWith('+xml') && type !== 'application/xml') || utf8Body(bodyBase64) === null)) return 'file';
  return 'raw';
}

export function utf8ContentType(current: string): string {
  // A quoted parameter can itself contain a semicolon or the word charset.
  // Only actual charset parameters describe the bytes written by TextEncoder.
  const parts: string[] = [];
  let start = 0, quoted = false, escaped = false;
  for (let index = 0; index < current.length; index++) {
    const character = current[index];
    if (escaped) { escaped = false; continue; }
    if (quoted && character === '\\') { escaped = true; continue; }
    if (character === '"') quoted = !quoted;
    else if (character === ';' && !quoted) { parts.push(current.slice(start, index)); start = index + 1; }
  }
  parts.push(current.slice(start));
  let charset = false;
  return parts.filter((part, index) => {
    if (!index || !/^\s*charset\s*=/i.test(part)) return true;
    if (charset) return false;
    charset = true;
    return true;
  }).map((part, index) => {
    if (!index) return part;
    const parameter = /^(\s*charset\s*=\s*)(.*?)(\s*)$/i.exec(part);
    if (!parameter || /^(?:utf-8|"utf-8")$/i.test(parameter[2])) return part;
    return `${parameter[1]}utf-8${parameter[3]}`;
  }).join(';');
}

export function rawContentType(parser: RawBodyParser, current: string): string {
  const type = contentTypeName(current);
  const matches = parser === 'text'
    ? type.startsWith('text/') && rawBodyParser(current) === 'text'
    : rawBodyParser(current) === parser;
  if (matches) return utf8ContentType(current);
  return {
    text: 'text/plain; charset=utf-8', json: 'application/json',
    html: 'text/html; charset=utf-8', babel: 'application/javascript; charset=utf-8', css: 'text/css; charset=utf-8',
  }[parser];
}

/** Patch only body/framing fields on the latest draft, retaining disabled fields
 * and all unrelated request changes. One active Content-Type stays in place.
 */
export function withRequestBody(draft: RequestDraft, bodyBase64: string, mime?: string,
    options: RequestBodyUpdateOptions = {}): RequestDraft {
  if (mime !== undefined && /[\r\n\0]/.test(mime)) throw new Error('Invalid Content-Type');
  let replaced = false;
  const headers: Header[] = [];
  for (const header of draft.headers) {
    const name = header.name.toLowerCase();
    if (!header.disabled && (['content-length', 'transfer-encoding'].includes(name)
        || (name === 'content-encoding' && !options.preserveContentEncoding))) continue;
    if (mime !== undefined && !header.disabled && name === 'content-type') {
      if (!replaced) headers.push({ ...header, value: mime });
      replaced = true;
    } else headers.push(header);
  }
  if (mime !== undefined && !replaced) headers.push({ name: 'Content-Type', value: mime });
  return { ...draft, headers, bodyBase64 };
}

export async function readFormBody(bodyBase64: string, mime: string, mode: 'form' | 'multipart'): Promise<RequestBodyPart[]> {
  if (!bodyBase64) return [];
  const expected = mode === 'form' ? 'application/x-www-form-urlencoded' : 'multipart/form-data';
  if (contentTypeName(mime) !== expected) throw new Error('Body is not this form type');
  if (mode === 'form') {
    const text = utf8Body(bodyBase64);
    if (text === null) throw new Error('Form body is not valid UTF-8');
    // URLSearchParams silently replaces invalid percent-encoded UTF-8 bytes.
    // Reject those fields before exposing an editable form that would lose data.
    for (const field of text.split('&')) {
      const equals = field.indexOf('=');
      for (const component of equals < 0 ? [field] : [field.slice(0, equals), field.slice(equals + 1)]) {
        try { decodeURIComponent(component.replace(/\+/g, ' ').replace(/%(?![0-9a-f]{2})/gi, '%25')); }
        catch { throw new Error('Form body is not valid UTF-8'); }
      }
    }
    return [...new URLSearchParams(text)].map(([name, value]) => ({ name, value }));
  }
  const response = new Response(bodyBytes(bodyBase64), { headers: { 'Content-Type': mime } });
  const form = await response.formData();
  return [...form].map(([name, value]) => typeof value === 'string'
    ? { name, value } : { name, value: '', file: value });
}
export async function encodeFormBody(parts: RequestBodyPart[], mode: 'form' | 'multipart'): Promise<EncodedRequestBody> {
  if (mode === 'form') {
    if (parts.some(part => part.file)) throw new Error('URL-encoded forms cannot contain files');
    return {
      bodyBase64: textBody(new URLSearchParams(parts.map(part => [part.name, part.value])).toString()),
      contentType: 'application/x-www-form-urlencoded',
    };
  }
  const form = new FormData();
  for (const part of parts) {
    if (part.file) form.append(part.name, part.file, part.file.name);
    else form.append(part.name, part.value);
  }
  const request = new Request('http://localhost/', { method: 'POST', body: form });
  return { bodyBase64: bytesBase64(new Uint8Array(await request.arrayBuffer())), contentType: request.headers.get('content-type')! };
}
export async function fileRequestBody(file: Blob): Promise<EncodedRequestBody> {
  return { bodyBase64: bytesBase64(new Uint8Array(await file.arrayBuffer())), contentType: file.type || 'application/octet-stream' };
}

/** Async body work may merge into newer headers/URL, but never a newer body,
 * Content-Type, editor mode or operation. UI creates a fresh token on changes.
 */
export function requestBodyWorkMatches(before: { body: string; mime: string; token: unknown },
    after: { body: string; mime: string; token: unknown }): boolean {
  return before.body === after.body && before.mime === after.mime && Object.is(before.token, after.token);
}
