import type { RequestDraft } from './types';
import { toCurl } from './curl';
import type { CodeTarget, ExportRequest } from './codegen/types';
import { webTargets } from './codegen/web';
import { pythonGoTargets } from './codegen/python-go';
import { otherTargets } from './codegen/other';

export const codeTargets: CodeTarget[] = [
  ...pythonGoTargets,
  ...webTargets,
  ...otherTargets,
  { id: 'shell-curl', language: 'Shell', library: 'cURL (Bash / Git Bash)', filename: 'request.sh', preservesDuplicates: true,
    generate: request => {
      if (request.method === 'HEAD' && request.bodyBytes.length) throw new Error('HEAD body unsupported by this library');
      const code = toCurl({ ...request, tls: { preset: 'native' } });
      // -X HEAD only changes the verb; --head also selects response semantics.
      return request.method === 'HEAD' ? code.replace(/^curl /m, 'curl --head ') : code;
    } },
];

export type CodeWarning = 'framing' | 'duplicates' | 'browser' | 'execution';

export function generateRequestCode(draft: RequestDraft, targetId: string) {
  const target = codeTargets.find(item => item.id === targetId);
  if (!target) throw new Error('Unknown code target');
  let parsed: URL;
  try { parsed = new URL(draft.url); } catch { throw new Error('Invalid HTTP URL'); }
  if (!['http:', 'https:'].includes(parsed.protocol)) throw new Error('Invalid HTTP URL');
  const token = /^[!#$%&'*+.^_`|~0-9a-zA-Z-]+$/;
  if (!token.test(draft.method)) throw new Error('Invalid HTTP method');
  const enabled = draft.headers.filter(header => !header.disabled && (header.name || header.value));
  for (const header of enabled) {
    if (!token.test(header.name) || /[\x00-\x08\x0a-\x1f\x7f]/.test(header.value)) throw new Error('Invalid HTTP header');
  }
  const tokens = enabled.filter(header => header.name.toLowerCase() === 'connection')
    .flatMap(header => header.value.toLowerCase().split(',').map(value => value.trim()));
  const omitted = new Set(['host', 'content-length', 'transfer-encoding', 'connection', 'proxy-connection',
    'proxy-authorization', 'keep-alive', 'te', 'trailer', 'upgrade', ...tokens]);
  const headers = enabled.filter(header => !omitted.has(header.name.toLowerCase())).map(({ name, value }) => ({ name, value }));
  let binary: string;
  try { binary = atob(draft.bodyBase64); } catch { throw new Error('Invalid Base64 body'); }
  const bodyBytes = Uint8Array.from(binary, char => char.charCodeAt(0));
  let bodyText: string | null = null;
  try { bodyText = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bodyBytes); } catch { /* Binary stays Base64. */ }
  // atob accepts whitespace and omitted padding, while target decoders differ.
  // Always emit canonical padded Base64 for the exact decoded byte sequence.
  const request: ExportRequest = { method: draft.method, url: draft.url, headers, bodyBase64: btoa(binary), bodyBytes, bodyText };
  const lower = headers.map(header => header.name.toLowerCase());
  const warnings: CodeWarning[] = ['execution'];
  if (headers.length !== enabled.length) warnings.push('framing');
  if (!target.preservesDuplicates && new Set(lower).size !== lower.length) warnings.push('duplicates');
  if (target.browser) warnings.push('browser');
  // Fetch follows the browser request model even in Node.js.
  if (bodyBytes.length && /^(GET|HEAD)$/i.test(request.method) && (target.browser || /fetch/i.test(target.id)))
    throw new Error('GET/HEAD body unsupported by this library');
  return { target, code: target.generate(request), warnings };
}
