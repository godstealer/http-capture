import { invoke } from './api';
import type { FlowMetadata } from './types';
import { RECORDED_BODY_COMPLETE_BYTES, RECORDED_BODY_PAGE_BYTES, recordedBodyRequest, validateRecordedBodyPage } from './recordedBodyModel';

/** Read only bytes added to this version of a recorded SSE response. The last
 * page may be reread when the body grows, but already emitted bytes are not
 * sent to the decoder again. A failed/stale page never advances the cursor. */
export async function readRecordedSseGrowth(
  flow: FlowMetadata,
  from: number,
  options: { isCurrent: () => boolean; onChunk: (chunk: Uint8Array, nextOffset: number) => void },
  call: typeof invoke = invoke,
): Promise<number> {
  const total = flow.bodyReferences.response?.byteLength;
  if (typeof total !== 'number' || !Number.isSafeInteger(total) || total < 0 || total > RECORDED_BODY_COMPLETE_BYTES || !Number.isSafeInteger(from) || from < 0 || from > total) {
    throw new Error('SSE 正文范围不可用');
  }
  let offset = from;
  while (offset < total) {
    if (!options.isCurrent()) throw new DOMException('SSE read cancelled', 'AbortError');
    const pageIndex = Math.floor(offset / RECORDED_BODY_PAGE_BYTES);
    const request = recordedBodyRequest(flow, 'response', pageIndex);
    const raw = await call<unknown>('read_flow_body', { ...request });
    if (!options.isCurrent()) throw new DOMException('SSE read cancelled', 'AbortError');
    const page = validateRecordedBodyPage(raw, request, total);
    const chunk = page.bytes.subarray(offset - request.offset);
    if (!chunk.length) throw new Error('SSE 正文片段未前进');
    offset += chunk.length;
    options.onChunk(chunk, offset);
  }
  return offset;
}
