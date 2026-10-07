import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from './api';
import type { RequestDraft } from './types';

export interface RequestEditor { id: string; draft: RequestDraft; selected: string | null; parentId: string | null }
interface Workspace { editors: RequestEditor[]; activeEditor: string; revision: number }
const empty = JSON.stringify({ editors: [], activeEditor: '' });

export function useRequestWorkspace() {
  const [editors, setEditors] = useState<RequestEditor[]>([]);
  const [activeEditor, setActiveEditor] = useState('');
  const [ready, setReady] = useState(false);
  const [message, setMessage] = useState('正在读取请求工作区…');
  const [error, setError] = useState(false);
  const persistence = useRef({ revision: 0, latest: empty, saved: empty, running: false, ready: false });
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    async function load() {
      try {
        const data = await invoke<Workspace>('request_workspace');
        if (disposed) return;
        const json = JSON.stringify({ editors: data.editors, activeEditor: data.activeEditor });
        Object.assign(persistence.current, { revision: data.revision, latest: json, saved: json, ready: true });
        setEditors(data.editors); setActiveEditor(data.activeEditor); setReady(true); setError(false); setMessage('请求已保存');
      } catch (e) {
        if (!disposed) { setError(true); setMessage(`工作区读取失败：${String(e)}`); timer = setTimeout(() => void load(), 5000); }
      }
    }
    void load();
    return () => { disposed = true; alive.current = false; clearTimeout(timer); };
  }, []);
  const flush = useCallback(async () => {
    const state = persistence.current;
    if (!state.ready || state.running) return;
    state.running = true;
    try {
      while (state.latest !== state.saved) {
        const json = state.latest;
        if (alive.current) { setMessage('正在保存请求…'); setError(false); }
        const revision = await invoke<number>('save_request_workspace', { workspace: { ...JSON.parse(json), revision: state.revision } });
        state.revision = revision; state.saved = json;
      }
      if (alive.current) { setMessage('请求已保存'); setError(false); }
    } catch (e) {
      if (alive.current) { setMessage(`请求未保存：${String(e)}`); setError(true); }
    } finally { state.running = false; }
  }, []);
  useEffect(() => {
    if (!ready) return;
    const state = persistence.current;
    state.latest = JSON.stringify({ editors, activeEditor });
    if (state.latest === state.saved) return;
    setMessage('请求有未保存的修改');
    const timer = setTimeout(() => void flush(), 400);
    return () => clearTimeout(timer);
  }, [editors, activeEditor, ready, flush]);
  useEffect(() => {
    const leave = (event: BeforeUnloadEvent) => {
      if (persistence.current.latest !== persistence.current.saved) { event.preventDefault(); event.returnValue = ''; }
    };
    window.addEventListener('beforeunload', leave);
    return () => window.removeEventListener('beforeunload', leave);
  }, []);
  return { editors, setEditors, activeEditor, setActiveEditor, ready, message, error, retry: flush };
}
