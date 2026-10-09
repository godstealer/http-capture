export function responseType(bytes: Uint8Array, contentType: string): { kind: 'text' | 'image' | 'audio' | 'video' | 'binary'; mime: string } {
  const mime = contentType.split(';')[0].trim().toLowerCase();
  const starts = (...prefix: number[]) => prefix.every((v, i) => bytes[i] === v);
  const ascii = (a: number, b: number) => String.fromCharCode(...bytes.slice(a, b));
  let detected = '';
  if (starts(137, 80, 78, 71, 13, 10, 26, 10)) detected = 'image/png';
  else if (starts(255, 216, 255)) detected = 'image/jpeg';
  else if (['GIF87a', 'GIF89a'].includes(ascii(0, 6))) detected = 'image/gif';
  else if (ascii(0, 4) === 'RIFF' && ascii(8, 12) === 'WEBP') detected = 'image/webp';
  else if (ascii(0, 4) === 'RIFF' && ascii(8, 12) === 'WAVE') detected = 'audio/wav';
  else if (ascii(0, 3) === 'ID3') detected = 'audio/mpeg';
  else if (ascii(4, 8) === 'ftyp') {
    const brand = ascii(8, 12);
    if (['avif', 'avis'].includes(brand)) detected = 'image/avif';
    else if (['M4A ', 'M4B '].includes(brand)) detected = 'audio/mp4';
    else if (['isom', 'iso2', 'mp41', 'mp42', 'avc1', 'M4V '].includes(brand)) detected = 'video/mp4';
  }
  const effective = detected || mime;
  // SVG stays in the text viewer: it can contain active markup.
  if (effective.startsWith('image/') && effective !== 'image/svg+xml') return { kind: 'image', mime: effective };
  if (effective.startsWith('audio/')) return { kind: 'audio', mime: effective };
  if (effective.startsWith('video/')) return { kind: 'video', mime: effective };
  if (mime.startsWith('text/') || /json|javascript|ecmascript|xml/.test(mime)) return { kind: 'text', mime };
  if (!mime && !bytes.subarray(0, 4096).some(b => b === 0 || (b < 32 && ![9, 10, 13].includes(b)))) {
    try { new TextDecoder('utf-8', { fatal: true }).decode(bytes); return { kind: 'text', mime: 'text/plain' }; } catch { /* binary */ }
  }
  return { kind: 'binary', mime: mime || 'application/octet-stream' };
}
