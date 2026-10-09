import { formatResponse, responseLanguage } from './formatResponse';
import { decodeZstd } from './decodeZstd';
import { decodeBrotli } from './decodeBrotli';
import { responseType } from './responseType';

self.onmessage = async (event: MessageEvent<{ body: string; contentType: string; encoding: string; parser: string }>) => {
  const { body, contentType, encoding, parser } = event.data;
  try {
    let bytes: Uint8Array<ArrayBuffer> = Uint8Array.from(atob(body), c => c.charCodeAt(0));
    for (const name of encoding.toLowerCase().split(',').map(s => s.trim()).filter(s => s && s !== 'identity').reverse()) {
      if (name === 'zstd') { bytes = decodeZstd(bytes); continue; }
      if (name === 'br') { bytes = await decodeBrotli(bytes); continue; }
      if (name !== 'gzip' && name !== 'deflate') throw new Error(`暂不支持解压 ${name}，请查看 Base64`);
      const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream(name));
      const reader = stream.getReader();
      const chunks: Uint8Array[] = []; let size = 0;
      while (true) {
        const { done, value } = await reader.read(); if (done) break;
        size += value.length;
        if (size > 8 * 1024 * 1024) { await reader.cancel(); throw new Error('解压后正文超过 8 MiB，请查看 Base64'); }
        chunks.push(value);
      }
      bytes = new Uint8Array(size); let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
    }
    const decodedBlob = new Blob([bytes], { type: contentType || 'application/octet-stream' });
    const charset = /charset\s*=\s*["']?([^\s;"']+)/i.exec(contentType)?.[1] ?? 'utf-8';
    const type = responseType(bytes, contentType);
    if (parser === 'auto' && type.kind !== 'text') {
      const hex = Array.from(bytes.subarray(0, 65536), (b, i) => `${i && i % 16 === 0 ? '\n' : i ? ' ' : ''}${b.toString(16).padStart(2, '0')}`).join('');
      self.postMessage({ raw: hex, formatted: hex, language: 'binary', binary: true, kind: type.kind, mime: type.mime, blob: new Blob([bytes], { type: type.mime }), size: bytes.length, error: '' });
      return;
    }
    const sse = contentType.split(';')[0].trim().toLowerCase() === 'text/event-stream';
    const raw = new TextDecoder(sse ? 'utf-8' : charset, { fatal: !sse }).decode(bytes, { stream: sse });
    const language = parser === 'auto' ? responseLanguage(contentType) : parser;
    try { self.postMessage({ decodedBlob, raw, formatted: await formatResponse(raw, language), language, error: '' }); }
    catch { self.postMessage({ decodedBlob, raw, formatted: raw, language, error: raw.length > 1024 * 1024
      ? '正文超过 1 MiB，已显示原文' : '无法按所选格式解析，已显示原文；可切换语言重试' }); }
  } catch (error) { self.postMessage({ raw: '', formatted: '', language: 'text', error: String(error), binary: true }); }
};
