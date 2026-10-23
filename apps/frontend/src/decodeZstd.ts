import { Decompress } from 'fzstd';

// fzstd allocates the frame's advertised history window before invoking its
// output callback. Walk framing metadata first so a tiny hostile body cannot
// request a multi-gigabyte allocation ahead of the decoded-byte limit.
function checkFrameWindows(input: Uint8Array, limit: number): void {
  let offset = 0;
  const invalid = () => { throw new Error('Zstd 正文不完整或已损坏'); };
  const read = (position: number, length: number): number => {
    if (position + length > input.length) invalid();
    let value = 0;
    for (let i = 0; i < length; i++) value += input[position + i] * 2 ** (8 * i);
    return value;
  };
  while (offset < input.length) {
    const magic = read(offset, 4);
    if (magic >= 0x184d2a50 && magic <= 0x184d2a5f) {
      const length = read(offset + 4, 4);
      if (offset + 8 + length > input.length) invalid();
      offset += 8 + length;
      continue;
    }
    if (magic !== 0xfd2fb528) invalid();
    const descriptor = read(offset + 4, 1);
    if (descriptor & 8) invalid();
    const singleSegment = (descriptor & 32) !== 0;
    const dictionaryBytes = [0, 1, 2, 4][descriptor & 3];
    const frameSizeFlag = descriptor >>> 6;
    const frameSizeBytes = frameSizeFlag ? 2 ** frameSizeFlag : Number(singleSegment);
    const headerEnd = offset + (singleSegment ? 5 : 6) + dictionaryBytes + frameSizeBytes;
    if (headerEnd > input.length) invalid();
    let windowSize = 0;
    if (!singleSegment) {
      const windowDescriptor = read(offset + 5, 1);
      const base = 2 ** (10 + (windowDescriptor >>> 3));
      windowSize = base + (base / 8) * (windowDescriptor & 7);
    } else {
      windowSize = read(headerEnd - frameSizeBytes, frameSizeBytes) + (frameSizeFlag === 1 ? 256 : 0);
    }
    if (windowSize > limit) throw new Error('Zstd 解压窗口超过 8 MiB，请查看 Base64');
    offset = headerEnd;
    let lastBlock = false;
    while (!lastBlock) {
      const block = read(offset, 3);
      lastBlock = (block & 1) !== 0;
      const type = (block >>> 1) & 3;
      if (type === 3) invalid();
      const payloadLength = type === 1 ? 1 : block >>> 3;
      if (offset + 3 + payloadLength > input.length) invalid();
      offset += 3 + payloadLength;
    }
    if (descriptor & 4) {
      if (offset + 4 > input.length) invalid();
      offset += 4;
    }
  }
}

export function decodeZstd(input: Uint8Array, limit = 8 * 1024 * 1024): Uint8Array<ArrayBuffer> {
  checkFrameWindows(input, limit);
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
