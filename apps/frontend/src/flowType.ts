import type { Flow } from './types';

/** Classify from response metadata; URL suffixes are not authoritative. */
export function flowType(flow: Flow): string {
  if (flow.websocket) return flow.request.url.startsWith('https:') || flow.request.url.startsWith('wss:') ? 'WSS' : 'WS';
  const mime = flow.response?.headers.find(h => h.name.toLowerCase() === 'content-type')?.value.split(';')[0].trim().toLowerCase();
  if (!mime) return '—';
  if (mime === 'text/event-stream') return 'SSE';
  if (mime.includes('protobuf') || mime === 'application/proto') return 'Protobuf';
  if (mime.startsWith('application/grpc')) return 'gRPC';
  if (mime === 'application/json' || mime.endsWith('+json')) return 'JSON';
  if (mime === 'text/html' || mime === 'application/xhtml+xml') return 'HTML';
  if (mime.includes('javascript') || mime.includes('ecmascript')) return 'JS';
  if (mime === 'text/css') return 'CSS';
  if (mime.startsWith('image/')) return '图片';
  if (mime.startsWith('video/')) return '视频';
  if (mime.startsWith('audio/')) return '音频';
  if (mime.startsWith('font/') || /(?:font|woff)/.test(mime)) return '字体';
  if (mime === 'application/xml' || mime === 'text/xml' || mime.endsWith('+xml')) return 'XML';
  if (mime === 'application/pdf') return 'PDF';
  if (mime === 'application/octet-stream') return '二进制';
  if (mime.startsWith('text/')) return '文本';
  return mime;
}
