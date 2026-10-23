import brotli from 'brotli-wasm';
import { Decompress as ZstdDecompress } from 'fzstd';
import { ZstdStreamGuard } from './zstdStreamGuard';
import type { SseEncoding } from './sseEncoding';

const LIMIT = 8 * 1024 * 1024;
type Sink = (bytes: Uint8Array) => Promise<void>;
export interface DecoderStage {
  push(bytes: Uint8Array): Promise<void>;
  finish(): Promise<void>;
  abort(): void;
}

export async function createSseDecoder(stages: SseEncoding[], output: Sink): Promise<DecoderStage> {
  let next: DecoderStage | null = null;
  for (const encoding of [...stages].reverse()) {
    const downstream = next;
    const sink = downstream ? (bytes: Uint8Array) => downstream.push(bytes) : output;
    let produced = 0;
    const emit = async (bytes: Uint8Array) => {
      produced += bytes.length;
      if (produced > LIMIT) throw new Error('解压后正文超过 8 MiB，请查看 Base64');
      if (bytes.length) await sink(bytes);
    };
    if (encoding === 'br') {
      const module = await brotli;
      const stream = new module.DecompressStream();
      let done = false;
      next = {
        async push(bytes) {
          if (done) { if (bytes.length) throw new Error('Brotli 正文包含多余数据'); return; }
          let offset = 0;
          while (true) {
            const result = stream.decompress(bytes.subarray(offset), Math.min(64 * 1024, LIMIT - produced + 1));
            let code: number, consumed: number, decoded: Uint8Array;
            try { code = result.code; consumed = result.input_offset; decoded = Uint8Array.from(result.buf); }
            finally { result.free(); }
            offset += consumed;
            await emit(decoded);
            if (code === module.BrotliStreamResultCode.ResultSuccess) {
              if (offset !== bytes.length) throw new Error('Brotli 正文包含多余数据');
              done = true;
              return;
            }
            if (code === module.BrotliStreamResultCode.NeedsMoreInput) {
              if (offset !== bytes.length) throw new Error('Brotli 正文不完整或已损坏');
              return;
            }
            if (code !== module.BrotliStreamResultCode.NeedsMoreOutput || (!consumed && !decoded.length))
              throw new Error('Brotli 正文不完整或已损坏');
          }
        },
        async finish() { if (!done) throw new Error('Brotli 正文不完整或已损坏'); await downstream?.finish(); },
        abort() { stream.free(); downstream?.abort(); },
      };
    } else if (encoding === 'zstd') {
      const guard = new ZstdStreamGuard(LIMIT);
      const chunks: Uint8Array[] = [];
      let pendingBytes = 0;
      const stream = new ZstdDecompress(chunk => {
        if (produced + pendingBytes + chunk.length > LIMIT) throw new Error('解压后正文超过 8 MiB，请查看 Base64');
        if (chunk.length) { chunks.push(Uint8Array.from(chunk)); pendingBytes += chunk.length; }
      });
      const drain = async () => {
        for (const chunk of chunks.splice(0)) { pendingBytes -= chunk.length; await emit(chunk); }
      };
      next = {
        async push(bytes) { guard.push(bytes, safe => stream.push(safe, false)); await drain(); },
        async finish() { guard.finish(); stream.push(new Uint8Array(), true); await drain(); await downstream?.finish(); },
        abort() { downstream?.abort(); },
      };
    } else {
      const stream = new DecompressionStream(encoding);
      const writer = stream.writable.getWriter();
      const reader = stream.readable.getReader();
      let pumpError: unknown;
      const pump = (async () => {
        while (true) {
          const { done, value } = await reader.read();
          if (done) return;
          await emit(value);
        }
      })().catch(error => {
        pumpError = error;
        // Release a writer blocked by downstream backpressure after a decode error.
        void writer.abort(error).catch(() => {});
        void reader.cancel(error).catch(() => {});
      });
      next = {
        async push(bytes) { if (pumpError) throw pumpError; if (bytes.length) await writer.write(Uint8Array.from(bytes)); if (pumpError) throw pumpError; },
        async finish() { await writer.close(); await pump; if (pumpError) throw pumpError; await downstream?.finish(); },
        abort() { void writer.abort().catch(() => {}); void reader.cancel().catch(() => {}); downstream?.abort(); },
      };
    }
  }
  if (!next) throw new Error('SSE 压缩格式不受支持');
  return next;
}
