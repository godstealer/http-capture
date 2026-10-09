import { Decompress } from 'fzstd';

export function decodeZstd(input: Uint8Array, limit = 8 * 1024 * 1024): Uint8Array<ArrayBuffer> {
  const chunks: Uint8Array[] = [];
  let size = 0;
  const decoder = new Decompress(chunk => {
    size += chunk.length;
    if (size > limit) throw new Error('解压后正文超过 8 MiB，请查看 Base64');
    chunks.push(chunk.slice());
  });
  decoder.push(input, true);
  const output = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { output.set(chunk, offset); offset += chunk.length; }
  return output;
}
