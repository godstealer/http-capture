// fzstd allocates a frame's advertised history window before producing output.
// Hold each frame header until its window is checked, including concatenated
// frames, while forwarding validated block bytes without retaining the frame.
const MAGIC = 0xfd2fb528;
type Phase = 'magic' | 'frame-header' | 'block-header' | 'block-data' | 'checksum' | 'skip-length' | 'skip-data';

export class ZstdStreamGuard {
  private phase: Phase = 'magic';
  private held: number[] = [];
  private remaining = 0;
  private lastBlock = false;
  private checksum = false;
  constructor(private readonly limit: number) {}

  private number(start: number, length: number): number {
    let value = 0;
    for (let i = 0; i < length; i++) value += this.held[start + i] * 2 ** (8 * i);
    return value;
  }

  push(input: Uint8Array, onSafe: (bytes: Uint8Array) => void) {
    let offset = 0;
    while (offset < input.length) {
      if (this.phase === 'block-data' || this.phase === 'skip-data' || this.phase === 'checksum') {
        const count = Math.min(this.remaining, input.length - offset);
        if (count) onSafe(input.subarray(offset, offset + count));
        offset += count;
        this.remaining -= count;
        if (this.remaining) continue;
        if (this.phase === 'skip-data' || this.phase === 'checksum') this.phase = 'magic';
        else if (this.lastBlock) {
          this.phase = this.checksum ? 'checksum' : 'magic';
          this.remaining = this.checksum ? 4 : 0;
        } else this.phase = 'block-header';
        continue;
      }
      this.held.push(input[offset++]);
      if (this.phase === 'magic' && this.held.length === 4) {
        const magic = this.number(0, 4) >>> 0;
        if (magic === MAGIC) this.phase = 'frame-header';
        else if (magic >= 0x184d2a50 && magic <= 0x184d2a5f) this.phase = 'skip-length';
        else throw new Error('Zstd 正文不完整或已损坏');
      }
      if (this.phase === 'skip-length' && this.held.length === 8) {
        this.remaining = this.number(4, 4);
        onSafe(Uint8Array.from(this.held)); this.held = [];
        this.phase = this.remaining ? 'skip-data' : 'magic';
      }
      if (this.phase === 'frame-header' && this.held.length >= 5) {
        const descriptor = this.held[4];
        if (descriptor & 8) throw new Error('Zstd 正文不完整或已损坏');
        const single = (descriptor & 32) !== 0;
        const dictionaryBytes = [0, 1, 2, 4][descriptor & 3];
        const frameSizeFlag = descriptor >>> 6;
        const frameSizeBytes = frameSizeFlag ? 2 ** frameSizeFlag : Number(single);
        const headerLength = (single ? 5 : 6) + dictionaryBytes + frameSizeBytes;
        if (this.held.length === headerLength) {
          let windowSize: number;
          if (single) windowSize = this.number(headerLength - frameSizeBytes, frameSizeBytes) + (frameSizeFlag === 1 ? 256 : 0);
          else {
            const windowDescriptor = this.held[5];
            const base = 2 ** (10 + (windowDescriptor >>> 3));
            windowSize = base + (base / 8) * (windowDescriptor & 7);
          }
          if (windowSize > this.limit) throw new Error('Zstd 解压窗口超过 8 MiB，请查看 Base64');
          this.checksum = (descriptor & 4) !== 0;
          onSafe(Uint8Array.from(this.held)); this.held = [];
          this.phase = 'block-header';
        }
      }
      if (this.phase === 'block-header' && this.held.length === 3) {
        const block = this.number(0, 3);
        this.lastBlock = (block & 1) !== 0;
        const kind = (block >>> 1) & 3;
        if (kind === 3) throw new Error('Zstd 正文不完整或已损坏');
        this.remaining = kind === 1 ? 1 : block >>> 3;
        onSafe(Uint8Array.from(this.held)); this.held = [];
        if (this.remaining) this.phase = 'block-data';
        else if (this.lastBlock) {
          this.phase = this.checksum ? 'checksum' : 'magic';
          this.remaining = this.checksum ? 4 : 0;
        } else this.phase = 'block-header';
      }
    }
  }

  finish() {
    if (this.phase !== 'magic' || this.held.length) throw new Error('Zstd 正文不完整或已损坏');
  }
}
