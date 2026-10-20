import type {FlowRecord, Header} from './types';
/** Raw H1 fields take precedence over an edited or normalized RequestDraft. */
export function headerOrderSource(flow:FlowRecord):{headers:Header[];original:boolean} {
 if(flow.rawRequestHeadBase64){
  try{
   const lines=atob(flow.rawRequestHeadBase64).split('\r\n');
   if(!/^[^ ]+ .+ HTTP\/1\.[01]$/.test(lines[0])||!atob(flow.rawRequestHeadBase64).endsWith('\r\n\r\n'))throw Error('Incomplete HTTP/1 head');
   const headers:Header[]=[];
   for(const line of lines.slice(1)){
    if(!line)break;
    if(/^[ \t]/.test(line)&&headers.length){headers[headers.length-1].value+=' '+line.trim();continue}
    const colon=line.indexOf(':');if(colon<=0)throw Error('Invalid raw header');
    headers.push({name:line.slice(0,colon),value:line.slice(colon+1).trim()});
   }
   return {headers,original:true};
  }catch{/* Old/invalid raw data is not evidence of wire order. */}
 }
 const request=flow.originalRequest??flow.request;
 return {headers:[...(request.pseudoHeaders??[]),...request.headers],original:flow.source==='capture'&&flow.clientProtocol==='HTTP/2'&&flow.state!=='imported'&&!flow.notes.some(n=>n.includes('导入'))&&!!request.pseudoHeaders?.length};
}
