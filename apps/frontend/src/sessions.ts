import { serializeHar } from './har';
import { isTauri, invoke } from '@tauri-apps/api/core';
import { encodeText, type Flow, type Header } from './types';

// HAR headers are arrays: keep duplicate names and their recorded order.
export function readSession(text: string): Flow[] {
  const data = JSON.parse(text);
  if (data.format === 'http-capture-session' && data.version === 1 && Array.isArray(data.flows)) return data.flows;
  if (!data.log || !Array.isArray(data.log.entries)) throw new Error('请选择 HAR 1.2 或本工具保存的会话 JSON 文件');
  const headers = (value: unknown): Header[] => {
    if (!Array.isArray(value) || value.some(h => typeof h?.name !== 'string' || typeof h?.value !== 'string')) throw new Error('HAR 请求头格式无效');
    return value.map(h => ({ name: h.name, value: h.value }));
  };
  return data.log.entries.map((e: any, i: number): Flow => {
    const r = e.request, s = e.response;
    if (!r || typeof r.url !== 'string' || typeof r.method !== 'string' || !s) throw new Error(`HAR 第 ${i + 1} 条请求不完整`);
    const startedAt = Date.parse(e.startedDateTime);
    if (!Number.isFinite(startedAt) || startedAt < 0 || typeof e.time !== 'number' || !Number.isFinite(e.time)) throw new Error(`HAR 第 ${i + 1} 条时间无效`);
    let body = r.postData?.text;
    if (body === undefined && r.postData?.params?.length) {
      if (!r.postData.mimeType?.startsWith('application/x-www-form-urlencoded')) throw new Error('HAR 的 multipart 参数缺少原始正文，无法完整导入');
      body = new URLSearchParams(r.postData.params.map((p: any) => [p.name, p.value ?? ''])).toString();
    }
    const content = s.content ?? {};
    if (content.encoding && content.encoding !== 'base64') throw new Error('不支持的 HAR 正文编码：' + content.encoding);
    const rawResponse = typeof content._rawBodyBase64 === 'string';
    const requestBody = typeof r.postData?._bodyBase64 === 'string' ? r.postData._bodyBase64 : encodeText(body ?? '');
    atob(requestBody);
    const responseBody = rawResponse ? content._rawBodyBase64 : content.encoding === 'base64' ? content.text ?? '' : encodeText(content.text ?? '');
    atob(responseBody);
    return { id: '', parentId: null, source: 'capture', startedAt, durationMs: Math.max(0, Math.round(e.time)),
      request: { method: r.method, url: r.url, headers: headers(r.headers), bodyBase64: requestBody, tls: { preset: 'native' }, engine: 'auto' },
      response: s.status > 0 ? { status: s.status, version: s.httpVersion || '未知',
        // HAR content is decoded entity content, not compressed wire bytes.
        headers: headers(s.headers).filter(h => rawResponse || !['content-encoding', 'transfer-encoding', 'content-length'].includes(h.name.toLowerCase())), bodyBase64: responseBody } : null,
      clientProtocol: r.httpVersion || null, error: typeof e._error === 'string' ? e._error : s.status > 0 ? null : 'HAR 未记录有效响应',
      notes: ['HAR 导入：头部顺序仅代表文件中的顺序，无法证明线上顺序；未提供 TLS 握手和原始报文字节。', rawResponse ? 'HAR 解压失败：已恢复扩展字段中的原始正文及编码头。' : 'HAR 响应正文按解码内容读取，已移除响应的编码和长度头。', ...(content.text === undefined && !rawResponse ? ['HAR 未包含响应正文。'] : [])] };
  });
}
export async function saveSession(flows: Flow[], format: 'json' | 'har' = 'json'): Promise<string> {
  const content = format === 'har' ? await serializeHar(flows) : JSON.stringify({ format: 'http-capture-session', version: 1, flows }, null, 2);
  if (isTauri()) return '已保存：' + await invoke<string>('save_session_file', { content, format });
  const blob = new Blob([content], { type: 'application/json' });
  const url = URL.createObjectURL(blob), a = document.createElement('a');
  a.href = url; a.download = `capture-${new Date().toISOString().replace(/[:.]/g, '-')}.${format}`; a.click();
  setTimeout(() => URL.revokeObjectURL(url), 30000);
  return '已提交浏览器下载';
}
