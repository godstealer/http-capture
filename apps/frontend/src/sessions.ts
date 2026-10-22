import { t } from './i18n';
import { serializeHar } from './har';
import {chooseSave,saveText} from './saveFile';
import { encodeText, type Flow, type FlowRecord, type Header } from './types';
import { loadCompleteFlow } from './flowRecords';
import {isTauri,invoke} from '@tauri-apps/api/core';

async function nativeExportChunk(id:string,content:string,offset:number):Promise<number>{
  const bytes=new TextEncoder().encode(content);
  for(let start=0;start<bytes.length;start+=65536){
    const part=Array.from(bytes.subarray(start,start+65536));
    offset=await invoke<number>('append_session_export',{id,offset,bytes:part});
  }
  return offset;
}

// HAR headers are arrays: keep duplicate names and their recorded order.
export function readSession(text: string): Flow[] {
  const data = JSON.parse(text.replace(/^\uFEFF/, ""));
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
      notes: ['HAR 导入：头部顺序仅代表文件中的顺序，无法证明线上顺序；未提供 TLS 握手和原始报文字节。', rawResponse ? 'HAR 已恢复扩展字段中的原始正文及编码头。' : 'HAR 响应正文按解码内容读取，已移除响应的编码和长度头。', ...(content.text === undefined && !rawResponse ? ['HAR 未包含响应正文。'] : [])] };
  });
}
export async function saveSession(flows: FlowRecord[], format: 'json' | 'har' = 'json', progress:(message:string)=>void=()=>{}, rawHar = true): Promise<string> {
  progress('选择保存位置…');
  const name=`capture-${new Date().toISOString().replace(/[:.]/g, '-')}.${format}`;
  const handle=await chooseSave(name);if(handle===null)return t('已取消保存');
  if(isTauri()){
    const id=await invoke<string|null>('begin_session_export',{format});
    if(id===null)return t('已取消保存');
    let offset=0;
    try{
      offset=await nativeExportChunk(id,format==='json'?'{"format":"http-capture-session","version":1,"flows":[':'{"log":{"version":"1.2","creator":{"name":"HTTP Capture","version":"0.1.0"},"entries":[',offset);
      for(let index=0;index<flows.length;index++){
        progress('正在读取完整记录…');
        const full=await loadCompleteFlow(flows[index]);
        const record=format==='json'?full:JSON.parse(await serializeHar([full],rawHar)).log.entries[0];
        offset=await nativeExportChunk(id,(index?',':'')+JSON.stringify(record),offset);
      }
      offset=await nativeExportChunk(id,format==='json'?']}':']}}',offset);
      progress('正在保存文件…');
      const path=await invoke<string|null>('finish_session_export',{id,commit:true});
      if(!path)throw Error('Export file was not saved');
      return t('已保存：')+path;
    }catch(error){await invoke('finish_session_export',{id,commit:false}).catch(()=>{});throw error;}
  }
  if(handle){
    // File System Access writes each complete record before loading the next.
    // A failed versioned read aborts the draft, so a prefix is never reported
    // as a complete session export.
    const writer=await handle.createWritable();
    try{
      await writer.write(new Blob([format==='json'?'{"format":"http-capture-session","version":1,"flows":[':'{"log":{"version":"1.2","creator":{"name":"HTTP Capture","version":"0.1.0"},"entries":[']));
      for(let index=0;index<flows.length;index++){
        progress('正在读取完整记录…');
        const full=await loadCompleteFlow(flows[index]);
        const record=format==='json'?full:JSON.parse(await serializeHar([full],rawHar)).log.entries[0];
        await writer.write(new Blob([index?',':'',JSON.stringify(record)]));
      }
      progress('正在保存文件…');
      await writer.write(new Blob([format==='json'?']}':']}}']));
      await writer.close();
      return t('已保存：')+handle.name;
    }catch(error){await writer.abort().catch(()=>{});throw error;}
  }
  // A preview/metadata DTO is never a complete export. Resolve exact recorded
  // versions only after the save picker; one failed read aborts the export.
  const complete:Flow[]=[];
  for(const flow of flows){progress('正在读取完整记录…');complete.push(await loadCompleteFlow(flow));}
  progress('正在生成导出文件…');await new Promise(resolve=>setTimeout(resolve,0));
  const content = format === 'har' ? await serializeHar(complete, rawHar) : JSON.stringify({ format: 'http-capture-session', version: 1, flows:complete });
  progress('正在保存文件…');
  return saveText(content,name,format,handle);
}
