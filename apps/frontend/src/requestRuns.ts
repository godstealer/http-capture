import type { Flow, RequestDraft } from './types';
import type { RequestEditor } from './useRequestWorkspace';

export function applyRequestResult(editors: RequestEditor[], id: string, sent: string, flow: Flow, redirected = false): RequestEditor[] {
  return editors.map(editor => editor.id === id ? {
    ...editor, selected: flow.id,
    draft: !redirected && JSON.stringify(editor.draft) === sent && !editor.draft.headers.some(header => header.disabled) ? structuredClone(flow.request) : editor.draft,
  } : editor);
}

const redirectStatuses = new Set([301, 302, 303, 307, 308]);
const safeCrossOriginHeaders = new Set(['accept', 'accept-encoding', 'accept-language', 'user-agent']);

/** Build the next manual request without forwarding credentials to a new origin. */
export function redirectRequest(request: RequestDraft, flow: Flow): RequestDraft | null {
  const response = flow.response;
  if (flow.error || !response || !redirectStatuses.has(response.status)) return null;
  const location = response.headers.find(header => !header.disabled && header.name.toLowerCase() === 'location')?.value;
  if (!location) return null;
  const source = new URL(request.url);
  const target = new URL(location, source);
  if (!['http:', 'https:'].includes(target.protocol) || target.username || target.password) throw Error('重定向目标不是可安全发送的 HTTP URL');
  if (source.protocol === 'https:' && target.protocol !== 'https:') throw Error('已阻止 HTTPS 降级到 HTTP 的自动重定向');
  const changedMethod = response.status === 303 && request.method.toUpperCase() !== 'HEAD'
    || [301, 302].includes(response.status) && request.method.toUpperCase() === 'POST';
  const crossOrigin = source.origin !== target.origin;
  const headers = request.headers.filter(header => {
    const name = header.name.toLowerCase();
    if (['host', 'content-length', 'transfer-encoding'].includes(name)) return false;
    if (changedMethod && ['content-type', 'content-encoding', 'origin'].includes(name)) return false;
    if (crossOrigin && !safeCrossOriginHeaders.has(name)) return false;
    return true;
  });
  return { ...request, url: target.href, method: changedMethod ? 'GET' : request.method,
    bodyBase64: changedMethod ? '' : request.bodyBase64, headers, pseudoHeaders: [] };
}

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
interface Run { executionId?: string; cancelling: boolean }
/** Ephemeral executions belong to tabs, never to the currently selected tab. */
export class RequestRuns {
  private runs = new Map<string, Run>();
  constructor(private invoke: Invoke, private changed: () => void, private error: (error: string) => void) {}
  state(id: string) { return this.runs.get(id); }
  async cancel(id: string) {
    const run = this.runs.get(id);
    if (!run || run.cancelling) return;
    run.cancelling = true; this.changed();
    if (!run.executionId) return; // start() sends cancellation after reservation is acknowledged.
    try { await this.invoke('cancel_replay', { executionId: run.executionId }); }
    catch (error) { run.cancelling = false; this.error(`取消失败，可重试：${String(error)}`); this.changed(); }
  }
  async start(id: string, request: RequestDraft, parentId: string | null, completed: (flow: Flow, redirected: boolean) => void,
    followRedirects = true, observed?: (flow: Flow) => void) {
    if (this.runs.has(id)) return;
    const run: Run = { cancelling: false };
    this.runs.set(id, run); this.changed();
    let last: Flow | null = null;
    let redirected = false;
    let delivered = false;
    try {
      let current = request;
      let parent = parentId;
      const visited = new Set<string>();
      for (let hop = 0; hop <= 10; hop++) {
        const identity = `${(current.method ?? 'GET').toUpperCase()} ${current.url}`;
        if (visited.has(identity)) { this.error('已停止跟随重定向：检测到循环'); break; }
        visited.add(identity);
        run.executionId = await this.invoke<string>('prepare_replay');
        this.changed();
        if (run.cancelling) await this.invoke('cancel_replay', { executionId: run.executionId });
        const flow = await this.invoke<Flow>('replay_request', { request: current, parentId: parent, executionId: run.executionId });
        last = flow;
        observed?.(flow);
        if (run.cancelling || !followRedirects) break;
        let next: RequestDraft | null;
        try { next = redirectRequest(current, flow); }
        catch (error) { this.error(String(error)); break; }
        if (!next) break;
        if (hop === 10) { this.error('已停止跟随重定向：超过 10 次'); break; }
        current = next;
        parent = flow.id;
        redirected = true;
      }
      if (last) { delivered = true; completed(last, redirected); }
    } catch (error) {
      if (last && !delivered) completed(last, redirected);
      if (run.executionId) {
        try { await this.invoke('cancel_replay', { executionId: run.executionId }); }
        catch { this.error('连接中断，无法确认内核已取消请求；请检查重放历史和拦截列表。'); }
      }
      this.error(String(error));
    } finally { this.runs.delete(id); this.changed(); }
  }
}
