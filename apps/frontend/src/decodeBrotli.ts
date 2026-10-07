import brotli from 'brotli-wasm';

export async function decodeBrotli(input: Uint8Array, limit = 8 * 1024 * 1024): Promise<Uint8Array<ArrayBuffer>> {
  const { DecompressStream, BrotliStreamResultCode } = await brotli;
  const stream = new DecompressStream();
  const chunks: Uint8Array[] = [];
  let offset = 0; let size = 0;
  try {
    while (true) {
      const result = stream.decompress(input.subarray(offset), Math.min(64 * 1024, limit - size + 1));
      let code: number; let consumed: number; let chunk: Uint8Array;
      try { code = result.code; consumed = result.input_offset; chunk = result.buf; }
      finally { result.free(); }
      offset += consumed; size += chunk.length;
      if (size > limit) throw new Error('解压后正文超过 8 MiB，请查看 Base64');
      chunks.push(chunk);
      if (code === BrotliStreamResultCode.ResultSuccess) {
        if (offset !== input.length) throw new Error('Brotli 正文包含多余数据');
        break;
      }
      if (code !== BrotliStreamResultCode.NeedsMoreOutput || (!consumed && !chunk.length)) throw new Error('Brotli 正文不完整或已损坏');
    }
    const output = new Uint8Array(size); let position = 0;
    for (const chunk of chunks) { output.set(chunk, position); position += chunk.length; }
    return output;
  } finally { stream.free(); }
}
