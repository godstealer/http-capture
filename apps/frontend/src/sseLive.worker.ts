import { createSseDecoder, type DecoderStage } from './sseDecodeStages';
import { parseSseEncodings } from './sseEncoding';

// A single worker owns the decompressor for one live SSE response.
// The capture body remains the original compressed bytes; only this view is decoded.
const LIMIT = 8 * 1024 * 1024;
type Input = { encoding: string; body?: string; chunk?: Uint8Array; reset?: boolean; finish?: boolean };
let previous = '';
let previousBytes = 0;
let currentEncoding = '';
let decoder: DecoderStage | null = null;
let decodedBytes = 0;
let raw = '';
let textDecoder = new TextDecoder('utf-8');
let failed = false;
let pending: ReturnType<typeof setTimeout> | undefined;
let writes = Promise.resolve();

function publish() {
  if (pending) return;
  pending = setTimeout(() => {
    pending = undefined;
    self.postMessage({ raw, formatted: raw, language: 'text', error: '', binary: false });
  }, 20);
}

function emitDecoded(value: Uint8Array) {
  decodedBytes += value.length;
  if (decodedBytes > LIMIT) throw new Error('解压后正文超过 8 MiB，请查看 Base64');
  raw += textDecoder.decode(value, { stream: true });
  publish();
}

function finishText() {
  raw += textDecoder.decode();
  publish();
}

async function reset(encoding: string) {
  if (pending) clearTimeout(pending);
  pending = undefined;
  decoder?.abort();
  decoder = null;
  previous = '';
  previousBytes = 0;
  currentEncoding = encoding;
  decodedBytes = 0;
  raw = '';
  textDecoder = new TextDecoder('utf-8');
  failed = false;
  self.postMessage({ reset: true });
  const stages = parseSseEncodings(encoding);
  if (!stages) throw new Error('SSE 压缩格式不受支持');
  decoder = await createSseDecoder(stages, async bytes => { emitDecoded(bytes); });
}

async function append({ body, chunk, encoding, reset: restart, finish }: Input) {
  if (chunk) {
    if (restart || encoding !== currentEncoding) await reset(encoding);
    if (failed) return;
    previousBytes += chunk.length;
    if (previousBytes > LIMIT) throw new Error('正文超过 8 MiB，请查看 Base64 或下载原始字节');
    await decoder!.push(chunk);
    if (finish) { await decoder!.finish(); finishText(); }
    return;
  }
  if (finish) { await decoder?.finish(); finishText(); return; }
  if (body === undefined) throw new Error('SSE 压缩正文输入缺失');
  if (body.length > Math.ceil(LIMIT / 3) * 4 + 4) throw new Error('正文超过 8 MiB，请查看 Base64 或下载原始字节');
  const stableChars = Math.floor(previousBytes / 3) * 4;
  if (encoding !== currentEncoding || body.length < previous.length || body.slice(0, stableChars) !== previous.slice(0, stableChars)) await reset(encoding);
  if (failed) return;
  if (body === previous) return;
  const aligned = Math.floor(previousBytes / 3) * 4;
  const bytes = Uint8Array.from(atob(body.slice(aligned)), char => char.charCodeAt(0));
  const delta = bytes.subarray(previousBytes % 3);
  previous = body;
  previousBytes += delta.length;
  if (previousBytes > LIMIT) throw new Error('正文超过 8 MiB，请查看 Base64 或下载原始字节');
  if (delta.length) await decoder!.push(delta);
}

self.onmessage = (event: MessageEvent<Input>) => {
  writes = writes.then(() => append(event.data)).catch(error => {
    failed = true;
    self.postMessage({ raw, formatted: raw, language: 'text', error: String(error), binary: true });
  });
};
