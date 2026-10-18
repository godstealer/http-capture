import type {RequestEditor} from './useRequestWorkspace';

export interface WorkspaceSnapshot {editors:RequestEditor[]; activeEditor:string}
export interface ReconciledWorkspace {
  workspace:WorkspaceSnapshot;
  copied:number;
  added:number;
  updated:number;
}

// Preserve array order (including duplicate/disabled headers), but object key
// order and an absent optional property versus undefined are not user edits.
function same(left:unknown,right:unknown):boolean {
  if(left===right)return true;
  if(!left||!right||typeof left!=='object'||typeof right!=='object')return false;
  if(Array.isArray(left)||Array.isArray(right))return Array.isArray(left)&&Array.isArray(right)&&left.length===right.length&&left.every((value,index)=>same(value,right[index]));
  const a=left as Record<string,unknown>,b=right as Record<string,unknown>;
  const keys=Object.keys(a).filter(key=>a[key]!==undefined),other=Object.keys(b).filter(key=>b[key]!==undefined);
  return keys.length===other.length&&keys.every(key=>Object.prototype.hasOwnProperty.call(b,key)&&same(a[key],b[key]));
}

/** Three-way recovery. The latest remote tabs always survive. Uncontested local
 * edits update their tab; a conflicting local draft becomes a new tab instead
 * of replacing the remote draft or silently losing local bytes. Local closures
 * do not delete remotely saved tabs during explicit recovery. */
export function reconcileWorkspace(
  base:WorkspaceSnapshot,local:WorkspaceSnapshot,remote:WorkspaceSnapshot,
  newId:()=>string=()=>crypto.randomUUID(),limit=100,
):ReconciledWorkspace {
  const baseById=new Map(base.editors.map(editor=>[editor.id,editor]));
  const remoteById=new Map(remote.editors.map(editor=>[editor.id,editor]));
  const editors=structuredClone(remote.editors),positions=new Map(editors.map((editor,index)=>[editor.id,index]));
  const used=new Set([...base.editors,...local.editors,...remote.editors].map(editor=>editor.id));
  const mapping=new Map<string,string>();
  let copied=0,added=0,updated=0;
  function copyId():string {
    for(let attempt=0;attempt<32;attempt++){const id=newId();if(id&&!used.has(id)){used.add(id);return id}}
    throw Error('无法生成新的工作区标签 ID，请重试恢复');
  }
  for(const editor of local.editors){
    const previous=baseById.get(editor.id),other=remoteById.get(editor.id);
    if(previous&&same(editor,previous))continue;
    if(other&&same(editor,other)){mapping.set(editor.id,editor.id);continue}
    if(other&&previous&&same(other,previous)){
      editors[positions.get(editor.id)!]=structuredClone(editor);updated++;mapping.set(editor.id,editor.id);continue;
    }
    const next=structuredClone(editor);
    if(other||previous){next.id=copyId();copied++}else{added++}
    mapping.set(editor.id,next.id);editors.push(next);
  }
  if(editors.length>limit)throw Error('合并后超过 100 个请求标签；当前草稿已保留，请先备份并关闭部分标签后重试');
  const active=mapping.get(local.activeEditor)??local.activeEditor;
  return {workspace:{editors,activeEditor:editors.some(editor=>editor.id===active)?active:editors.some(editor=>editor.id===remote.activeEditor)?remote.activeEditor:editors[0]?.id??''},copied,added,updated};
}

export function isWorkspaceConflict(error:unknown):boolean {
  return /工作区已在其他窗口修改|workspace[^\n]*(?:another window|changed|conflict)|revision[^\n]*(?:conflict|mismatch)/i.test(String(error));
}
