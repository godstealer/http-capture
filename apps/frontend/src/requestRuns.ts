import type { Flow, RequestDraft } from './types';
import type { RequestEditor } from './useRequestWorkspace';

export function applyRequestResult(editors: RequestEditor[], id: string, sent: string, flow: Flow): RequestEditor[] {
  return editors.map(editor => editor.id === id ? {
    ...editor, selected: flow.id,
    draft: JSON.stringify(editor.draft) === sent ? structuredClone(flow.request) : editor.draft,
  } : editor);
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
  async start(id: string, request: RequestDraft, parentId: string | null, completed: (flow: Flow) => void) {
    if (this.runs.has(id)) return;
    const run: Run = { cancelling: false };
    this.runs.set(id, run); this.changed();
    try {
      run.executionId = await this.invoke<string>('prepare_replay');
      this.changed();
      if (run.cancelling) await this.invoke('cancel_replay', { executionId: run.executionId });
      const flow = await this.invoke<Flow>('replay_request', { request, parentId, executionId: run.executionId });
      completed(flow);
    } catch (error) {
      if (run.executionId) {
        try { await this.invoke('cancel_replay', { executionId: run.executionId }); }
        catch { this.error('连接中断，无法确认内核已取消请求；请检查重放历史和拦截列表。'); }
      }
      this.error(String(error));
    } finally { this.runs.delete(id); this.changed(); }
  }
}
