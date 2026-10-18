import { useCallback, useEffect, useLayoutEffect, useRef, useState, type SetStateAction } from 'react';
import { invoke } from './api';
import type { RequestDraft } from './types';
import {isWorkspaceConflict,reconcileWorkspace,type WorkspaceSnapshot} from './workspaceRecovery';

export interface GrpcDraft { method: string; json: string }
export interface RequestEditor { grpcDraft?: GrpcDraft; savedRequestId?: string; id: string; draft: RequestDraft; selected: string | null; parentId: string | null }
interface Workspace { editors: RequestEditor[]; activeEditor: string; revision: number }
const empty = JSON.stringify({ editors: [], activeEditor: '' });

export function useRequestWorkspace() {
  const [editors, applyEditors] = useState<RequestEditor[]>([]);
  const [activeEditor, applyActiveEditor] = useState('');
  const [ready, setReady] = useState(false);
  const [message, setMessage] = useState('正在读取请求工作区…');
  const [error, setError] = useState(false);
  const [conflict,setConflict]=useState(false),[recovering,setRecovering]=useState(false);
  const persistence = useRef({ revision: 0, local:{editors:[],activeEditor:''} as WorkspaceSnapshot, latest: empty, saved: empty, running: false, recovering:false, conflict:false, ready: false });
  const alive = useRef(true);
  const saving = useRef<Promise<boolean> | null>(null);
  const recovery = useRef<Promise<boolean> | null>(null);
  // Update the authoritative local snapshot synchronously. A keystroke during
  // an awaited recovery must be included even before React commits its render.
  const setEditors=useCallback((action:SetStateAction<RequestEditor[]>)=>{
    const state=persistence.current;
    const next=typeof action==='function'?action(state.local.editors):action;
    state.local={...state.local,editors:next};state.latest=JSON.stringify(state.local);applyEditors(next);
  },[]);
  const setActiveEditor=useCallback((action:SetStateAction<string>)=>{
    const state=persistence.current;
    const next=typeof action==='function'?action(state.local.activeEditor):action;
    state.local={...state.local,activeEditor:next};state.latest=JSON.stringify(state.local);applyActiveEditor(next);
  },[]);
  useEffect(() => {
    alive.current = true;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    async function load() {
      try {
        const data = await invoke<Workspace>('request_workspace');
        if (disposed) return;
        const json = JSON.stringify({ editors: data.editors, activeEditor: data.activeEditor });
        Object.assign(persistence.current, { revision: data.revision, local:{editors:data.editors,activeEditor:data.activeEditor}, latest: json, saved: json, ready: true, conflict:false });
        applyEditors(data.editors); applyActiveEditor(data.activeEditor); setReady(true); setError(false); setConflict(false); setMessage('草稿已自动保存');
      } catch (e) {
        if (!disposed) { setError(true); setMessage(`工作区读取失败：${String(e)}`); timer = setTimeout(() => void load(), 5000); }
      }
    }
    void load();
    return () => { disposed = true; alive.current = false; clearTimeout(timer); };
  }, []);
  const flush = useCallback((): Promise<boolean> => {
    if(recovery.current)return recovery.current;
    if (saving.current) return saving.current;
    const operation = (async () => {
    const state = persistence.current;
    if (!state.ready) return true;
    if(state.conflict||state.recovering)return false;
    state.running = true;
    try {
      while (state.latest !== state.saved&&!state.recovering) {
        const json = state.latest;
        if (alive.current) { setMessage('正在保存草稿…'); setError(false); }
        const revision = await invoke<number>('save_request_workspace', { workspace: { ...JSON.parse(json), revision: state.revision } });
        state.revision = revision; state.saved = json;
      }
      if(state.recovering)return false;
      if (alive.current) { setMessage('草稿已自动保存'); setError(false); }
      return true;
    } catch (e) {
      state.conflict=isWorkspaceConflict(e);
      if(alive.current)setConflict(state.conflict);
      if (alive.current) { setMessage(`草稿未保存：${String(e)}`); setError(true); }
      return false;
    } finally { state.running = false; }
    })();
    saving.current = operation;
    void operation.finally(() => { saving.current = null; });
    return operation;
  }, []);
  const recover=useCallback(():Promise<boolean>=>{
    if(recovery.current)return recovery.current;
    const state=persistence.current;
    if(!state.ready)return Promise.resolve(false);
    state.recovering=true;setRecovering(true);setMessage('正在读取并合并其他窗口的草稿…');
    const operation=(async()=>{
      let rebased=false;
      try{
        // Let an already-issued save settle before choosing the three-way base.
        if(saving.current)await saving.current;
        const remote=await invoke<Workspace>('request_workspace');
        if(!alive.current)return false;
        const remoteSnapshot={editors:remote.editors,activeEditor:remote.activeEditor};
        const merged=reconcileWorkspace(JSON.parse(state.saved),state.local,remoteSnapshot);
        // Reading and merging never discard local changes. If the subsequent
        // write fails, this merged workspace stays visible and remains unsaved.
        state.revision=remote.revision;state.saved=JSON.stringify(remoteSnapshot);
        state.local=merged.workspace;state.latest=JSON.stringify(merged.workspace);state.conflict=false;
        rebased=true;
        applyEditors(merged.workspace.editors);applyActiveEditor(merged.workspace.activeEditor);setConflict(false);
        while(state.latest!==state.saved){
          const json=state.latest;
          const revision=await invoke<number>('save_request_workspace',{workspace:{...JSON.parse(json),revision:state.revision}});
          state.revision=revision;state.saved=json;
          // Public setters keep latest current while this RPC is pending, so
          // later typing is saved in the next CAS write rather than overwritten.
        }
        if(alive.current){setError(false);setMessage(merged.copied?'工作区已合并保存；冲突草稿已复制为新标签，其他窗口的标签均已保留':'工作区已合并保存，其他窗口的标签均已保留')}
        return true;
      }catch(e){
        state.conflict=isWorkspaceConflict(e)||(!rebased&&state.conflict);if(alive.current){setConflict(state.conflict);setError(true);setMessage(`工作区恢复未完成，当前草稿已保留：${String(e)}`)}
        return false;
      }finally{state.recovering=false;if(alive.current)setRecovering(false)}
    })();
    recovery.current=operation;void operation.finally(()=>{recovery.current=null});return operation;
  },[]);
  useLayoutEffect(() => {
    if (!ready) return;
    const state = persistence.current;
    if(state.recovering)return;
    if(state.conflict){setError(true);setMessage('工作区已在其他窗口修改，当前草稿已保留；请合并恢复后继续保存');return}
    if (state.latest === state.saved) {
      if (!state.running) { setMessage('草稿已自动保存'); setError(false); }
      return;
    }
    setMessage('草稿有未保存的修改');
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
  return { editors, setEditors, activeEditor, setActiveEditor, ready, message, error, conflict, recovering, recover, retry: flush };
}
