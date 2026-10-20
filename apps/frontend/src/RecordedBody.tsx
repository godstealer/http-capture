import {useEffect,useMemo,useRef,useState} from 'react';
import {downloadFlowBody,invoke} from './api';
import type {BodyVariant,FlowMetadata,Header} from './types';
import {t} from './i18n';
import ResponseBody from './ResponseBody';
import './recorded-body.css';
import {RECORDED_BODY_PAGE_BYTES,RECORDED_BODY_COMPLETE_BYTES,readCompleteRecordedBody,recordedBodyEncoded,recordedBodyHex,recordedBodyIdentity,recordedBodyRequest,validateRecordedBodyPage,type RecordedBodyPage} from './recordedBodyModel';

export interface RecordedBodyProps {
  flow:FlowMetadata;variant:BodyVariant;headers:Header[];onCopy:(text:string)=>void;
  active?:boolean;connected?:boolean;mode?:'body'|'base64'|'raw';onReloadMetadata?:()=>void;
}
export default function RecordedBody({flow,variant,headers,onCopy,active=true,connected=true,mode='body',onReloadMetadata}:RecordedBodyProps) {
  const key=recordedBodyIdentity(flow,variant),total=flow.bodyReferences[variant]?.byteLength;
  const [position,setPosition]=useState({key,index:0}),[reload,setReload]=useState(0),[loading,setLoading]=useState(false);
  const [data,setData]=useState<{key:string;offset:number;page:RecordedBodyPage}|null>(null);
  const [failure,setFailure]=useState<{key:string;message:string}|null>(null),[view,setView]=useState<'text'|'hex'|'base64'>('text');
  const [download,setDownload]=useState<{key:string;busy:boolean;message:string}|null>(null);
  const current=useRef({key,active,connected});current.current={key,active,connected};
  const lifetime=useRef(0),downloadAbort=useRef<AbortController|null>(null);
  const index=position.key===key?position.index:0,offset=index*RECORDED_BODY_PAGE_BYTES;
  const pageKey=JSON.stringify([key,index]);
  const [jump,setJump]=useState({key:pageKey,value:'1',invalid:false});
  const jumpDraft=jump.key===pageKey?jump:{key:pageKey,value:String(index+1),invalid:false};
  const [complete,setComplete]=useState<{key:string;state:'loading'|'ready'|'error';body?:string;message?:string}|null>(null);
  const completeAbort=useRef<AbortController|null>(null),pageNow=useRef({key:pageKey,active,connected});pageNow.current={key:pageKey,active,connected};
  const completeBody=complete?.key===pageKey&&complete.state==='ready'?complete.body:undefined;
  const page=data?.key===key&&data.offset===offset?data.page:null;
  const encoded=recordedBodyEncoded(headers),effectiveView=mode==='base64'?'base64':view==='text'&&(page?.text===null||encoded)?'hex':view;
  const text=useMemo(()=>!page?'':effectiveView==='base64'?page.bodyBase64:effectiveView==='hex'?recordedBodyHex(page.bytes,page.offset):page.text??'',[page,effectiveView]);
  const pages=typeof total==='number'&&Number.isSafeInteger(total)&&total>=0?Math.max(1,Math.ceil(total/RECORDED_BODY_PAGE_BYTES)):0;
  useEffect(()=>{
    lifetime.current++;setDownload(null);
    return()=>{lifetime.current++;downloadAbort.current?.abort();downloadAbort.current=null};
  },[key,active,connected]);
  useEffect(()=>{
    setJump({key:pageKey,value:String(index+1),invalid:false});setComplete(null);
    return()=>{completeAbort.current?.abort();completeAbort.current=null};
  },[pageKey,active,connected]);
  useEffect(()=>{
    if(!active||!connected){setLoading(false);setData(null);setFailure(null);return;}
    let live=true;setLoading(true);setFailure(null);setData(null);
    let request;
    try {request=recordedBodyRequest(flow,variant,index)}catch(error){setFailure({key,message:String(error)});setLoading(false);return;}
    invoke<unknown>('read_flow_body',{...request}).then(raw=>{
      if(!live)return;
      const page=validateRecordedBodyPage(raw,request,total!);
      setData({key,offset:request.offset,page});
    }).catch(error=>{if(live)setFailure({key,message:String(error)})}).finally(()=>{if(live)setLoading(false)});
    return()=>{live=false};
  },[key,index,reload,active,connected,total]);
  function jumpToPage() {
    if(loading||!active||!connected)return;
    const value=jumpDraft.value.trim(),number=Number(value);
    if(!/^\d+$/.test(value)||!Number.isSafeInteger(number)||number<1||number>pages){setJump({key:pageKey,value:jumpDraft.value,invalid:true});return;}
    setJump({key:pageKey,value:String(number),invalid:false});
    if(number-1===index)setReload(value=>value+1);else setPosition({key,index:number-1});
  }
  async function processComplete() {
    if(!active||!connected||loading||completeAbort.current||completeBody!==undefined)return;
    const controller=new AbortController();completeAbort.current=controller;setComplete({key:pageKey,state:'loading'});
    const isCurrent=()=>!controller.signal.aborted&&pageNow.current.key===pageKey&&pageNow.current.active&&pageNow.current.connected;
    try {
      const body=await readCompleteRecordedBody(flow,variant,{signal:controller.signal,isCurrent});
      if(isCurrent())setComplete({key:pageKey,state:'ready',body});
    }catch(error){if(isCurrent())setComplete({key:pageKey,state:'error',message:String(error)})}
    finally {if(completeAbort.current===controller)completeAbort.current=null;}
  }
  async function save() {
    if(!active||!connected||downloadAbort.current||typeof total!=='number')return;
    const controller=new AbortController(),generation=lifetime.current;downloadAbort.current=controller;
    setDownload({key,busy:true,message:''});
    try {
      const result=await downloadFlowBody({id:flow.id,variant,expectedVersion:flow.bodyRevision},{signal:controller.signal,expectedBytes:total});
      if(generation!==lifetime.current||current.current.key!==key||!current.current.active||!current.current.connected)return;
      const message=result.kind==='cancelled'?t('已取消保存'):result.kind==='saved'?t('已保存：')+result.name
        :t('已提交浏览器下载；此浏览器使用完整 Blob 缓冲，不保证低内存。');
      setDownload({key,busy:false,message});
    }catch(error){if(generation===lifetime.current&&current.current.key===key&&current.current.active&&current.current.connected)setDownload({key,busy:false,message:String(error)})}
    finally {if(downloadAbort.current===controller)downloadAbort.current=null;}
  }
  if(!active)return null;
  if(!connected)return <p role="status">{t('内核已断开，连接后读取历史正文。')}</p>;
  return <div className="formatted-response recorded-body">
    <div className="format-toolbar"><span>{typeof total==='number'?t('完整正文 {v0} 字节',{v0:total}):t('此记录没有该正文')}</span>
      <button type="button" disabled={typeof total!=='number'||total>256*1024*1024||download?.key===key&&download.busy} onClick={()=>void save()}>{t('下载完整原始实体')}</button>
      <button type="button" disabled={loading||!pages} onClick={()=>setReload(value=>value+1)}>{t('重读当前片段')}</button>
      {onReloadMetadata&&<button type="button" onClick={onReloadMetadata}>{t('重新读取记录信息')}</button>}
      {download?.key===key&&download.message&&<span role="status">{download.message}</span>}
    </div>
    {typeof total==='number'&&total>256*1024*1024&&<p className="format-note">{t('完整下载当前限制为 256 MiB，分段查看仍可使用。')}</p>}
    {mode==='body'&&typeof total==='number'&&total>RECORDED_BODY_PAGE_BYTES&&<div className="format-toolbar">
      {total<=RECORDED_BODY_COMPLETE_BYTES?<button type="button" disabled={loading||complete?.key===pageKey&&(complete.state==='loading'||complete.state==='ready')} onClick={()=>void processComplete()}>{t('加载完整正文用于格式化或预览')}</button>:<span>{t('完整格式化或预览的处理上限为 8 MiB；抓包不受此限，请使用分页或完整下载。')}</span>}
      {complete?.key===pageKey&&complete.state==='loading'&&<span role="status">{t('正在完整读取所选正文…')}</span>}
      {complete?.key===pageKey&&complete.state==='error'&&<span role="alert">{t('完整正文读取失败，未使用部分内容。')} {complete.message}</span>}
    </div>}
    {loading&&<p role="status">{t('正在读取正文片段…')}</p>}
    {failure?.key===key&&<p role="alert">{t('正文读取失败；记录可能已更新，请重新读取记录信息后重试。')} {failure.message}</p>}
    {page?.totalBytes===0&&<p role="status">{t('（空正文）')}</p>}
    {page&&page.totalBytes>0&&<>
      <div className="format-note">{completeBody!==undefined?t('已完整读取所选实体，以下处理不包含其他正文槽位。'):page.complete?t('当前片段包含完整实体正文。'):t('仅显示原始实体的当前片段；不会解压、格式化或预览截断内容。')}
        {!page.complete&&completeBody===undefined&&t('复制仅包含当前片段，完整内容请使用下载。')}
      </div>
      {mode==='body'&&(page.complete||completeBody!==undefined)?<ResponseBody key={pageKey} body={completeBody??page.bodyBase64} headers={headers} onCopy={onCopy}/>:<>
        <div className="format-toolbar">{mode!=='base64'&&<>
          <button type="button" disabled={page.text===null||encoded} aria-pressed={effectiveView==='text'} onClick={()=>setView('text')}>{t('原始文本片段')}</button>
          <button type="button" aria-pressed={effectiveView==='hex'} onClick={()=>setView('hex')}>Hex</button>
          <button type="button" aria-pressed={effectiveView==='base64'} onClick={()=>setView('base64')}>Base64</button>
        </>}<button type="button" onClick={()=>onCopy(text)}>{t('复制当前片段')}</button></div>
        {(page.text===null||encoded)&&<div className="format-note">{encoded?t('片段保留内容编码，仅显示原始字节。'):t('此片段不是完整 UTF-8 文本，可能为二进制或跨字符边界；请使用 Hex 或 Base64。')}</div>}
        <div className="formatted-scroll"><pre>{text||t('（空正文）')}</pre></div>
      </>}
    </>}
    {pages>0&&<div className="format-toolbar" aria-label={t('正文分页')}>
      <button type="button" disabled={loading||index===0} onClick={()=>setPosition({key,index:index-1})}>{t('上一片段')}</button>
      <span>{t('第 {v0} / {v1} 页 · 字节偏移 {v2}',{v0:index+1,v1:pages,v2:offset})}</span>
      <button type="button" disabled={loading||index+1>=pages} onClick={()=>setPosition({key,index:index+1})}>{t('下一片段')}</button>
      {pages>1&&<form className="recorded-body-jump" noValidate onSubmit={event=>{event.preventDefault();jumpToPage()}}>
        <input type="number" min={1} max={pages} step={1} aria-label={t('跳转到正文页码')} aria-invalid={jumpDraft.invalid} value={jumpDraft.value} onChange={event=>setJump({key:pageKey,value:event.target.value,invalid:false})}/>
        <button type="submit" disabled={loading}>{t('跳转')}</button>
        {jumpDraft.invalid&&<span role="alert">{t('请输入 1 到 {v0} 的整数页码。',{v0:pages})}</span>}
      </form>}
    </div>}
  </div>;
}
