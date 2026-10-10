import type { Flow, Header } from './types';

const header = (headers: Header[], name: string) => headers.filter(h => h.name.toLowerCase() === name).map(h => h.value).join(', ');
const bytes = (value: string) => Uint8Array.from(atob(value), c => c.charCodeAt(0));
function base64(value: Uint8Array) {
  let binary = '';
  for (let i = 0; i < value.length; i += 32768) binary += String.fromCharCode(...value.subarray(i, i + 32768));
  return btoa(binary);
}
export async function createHar(flows: Flow[], decode: (body: string, encoding: string) => Promise<Uint8Array>) {
  const entries = [];
  for (const flow of flows) {
    const request = flow.request, response = flow.response;
    const requestBytes = bytes(request.bodyBase64);
    let postData;
    if (requestBytes.length) {
      const mimeType = header(request.headers, 'content-type') || 'application/octet-stream';
      try { postData = { mimeType, text: new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(requestBytes) }; }
      catch { postData = { mimeType, _bodyBase64: request.bodyBase64, comment: 'Binary request body is stored in the HTTP Capture _bodyBase64 extension.' }; }
    }
    let queryString: { name: string; value: string }[] = [];
    try { queryString = [...new URL(request.url).searchParams].map(([name, value]) => ({ name, value })); } catch { /* Imported URL may be incomplete. */ }
    let content: {size: number; mimeType: string; text?: string; encoding?: string; comment?: string; _rawBodyBase64?: string; _contentEncoding?: string} = { size: 0, mimeType: header(response?.headers ?? [], 'content-type') || 'application/octet-stream' };
    if (response) {
      const encoding = header(response.headers, 'content-encoding');
      try {
        const decoded = encoding ? await decode(response.bodyBase64, encoding) : bytes(response.bodyBase64);
        content = { ...content, size: decoded.length, text: base64(decoded), encoding: 'base64' };
      } catch (error) {
        // Never present compressed data as decoded HAR content. Preserve it in an explicit extension.
        content = { ...content, size: -1, _rawBodyBase64: response.bodyBase64, _contentEncoding: encoding, comment: `Body could not be decoded: ${String(error)}` };
      }
    }
    entries.push({ startedDateTime: new Date(flow.startedAt).toISOString(), time: flow.durationMs,
      request: { method: request.method, url: request.url, httpVersion: flow.clientProtocol ?? '', headers: request.headers, cookies: [], queryString, bodySize: requestBytes.length, headersSize: -1, ...(postData ? { postData } : {}) },
      response: { status: response?.status ?? 0, statusText: '', httpVersion: response?.version ?? '', headers: response?.headers ?? [], cookies: [], content, redirectURL: header(response?.headers ?? [], 'location'), headersSize: -1, bodySize: response ? bytes(response.bodyBase64).length : -1 },
      cache: {}, timings: { send: 0, wait: flow.durationMs, receive: 0, comment: 'Only total duration was recorded; phase timings are unavailable.' },
      ...(flow.error ? { _error: flow.error } : {}), comment: 'Recorded HTTP messages; TLS details and WebSocket frames require HTTP Capture session JSON.' });
  }
  return { log: { version: '1.2', creator: { name: 'HTTP Capture', version: '0.1.0' }, entries } };
}

export async function serializeHar(flows: Flow[]) {
  const har = await createHar(flows, (body, encoding) => new Promise((resolve, reject) => {
    const worker = new Worker(new URL('./response.worker.ts', import.meta.url), { type: 'module' });
    const timer = setTimeout(() => { worker.terminate(); reject(new Error('Decode timeout')); }, 30000);
    worker.onmessage = async event => {
      clearTimeout(timer); worker.terminate();
      try { if (!event.data.decodedBlob) throw new Error(event.data.error || 'Decode failed'); resolve(new Uint8Array(await event.data.decodedBlob.arrayBuffer())); }
      catch (error) { reject(error); }
    };
    worker.onerror = () => { clearTimeout(timer); worker.terminate(); reject(new Error('Decoder worker failed')); };
    worker.postMessage({ body, encoding, contentType: 'application/octet-stream', parser: 'bytes' });
  }));
  return JSON.stringify(har, null, 2);
}
