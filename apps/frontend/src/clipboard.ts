import { isTauri, invoke } from '@tauri-apps/api/core';
export function readClipboard():Promise<string>{return isTauri()?invoke<string>('read_clipboard'):navigator.clipboard.readText();}
export function writeClipboard(text:string):Promise<void>{return isTauri()?invoke<void>('write_clipboard',{text}):navigator.clipboard.writeText(text);}
