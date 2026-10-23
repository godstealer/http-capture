export const INLINE_RESPONSE_BYTES = 8 * 1024 * 1024;

/** Base64 contains ASCII only, so its length gives a decoded-size ceiling
 * without first allocating the decoded body. Malformed input is left for atob
 * to reject when it is within the processing budget. */
export function exceedsInlineResponseBudget(base64: string): boolean {
  const padding = base64.endsWith('==') ? 2 : base64.endsWith('=') ? 1 : 0;
  return Math.floor(base64.length / 4) * 3 - padding > INLINE_RESPONSE_BYTES;
}
