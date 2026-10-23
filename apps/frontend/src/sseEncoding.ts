export type SseEncoding = 'gzip' | 'deflate' | 'br' | 'zstd';

/** Content-Encoding lists encodings in application order; decoding reverses it. */
export function parseSseEncodings(value: string): SseEncoding[] | null {
  const parts = value.toLowerCase().split(',').map(part => part.trim());
  if (parts.length > 4 || parts.some(part => !part)) return null;
  const stages: SseEncoding[] = [];
  for (const part of parts.reverse()) {
    if (part === 'identity') continue;
    if (part !== 'gzip' && part !== 'deflate' && part !== 'br' && part !== 'zstd') return null;
    stages.push(part);
  }
  return stages.length ? stages : null;
}
