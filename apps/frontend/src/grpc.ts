import * as pb from 'protobufjs';
import type {Header, RequestDraft} from './types';
export function schema(sources:string[]) {
 const root=new pb.Root();
 for(const text of sources){if(text.trim().startsWith('{'))root.addJSON(JSON.parse(text).nested);else pb.parse(text,root,{keepCase:true});}
 root.resolveAll();
 const methods:pb.Method[]=[];
 const visit=(node:pb.NamespaceBase)=>{for(const child of node.nestedArray){if(child instanceof pb.Service)methods.push(...child.methodsArray);if(child instanceof pb.Namespace)visit(child)}};
 visit(root);return {root,methods};
}
export const methodPath=(m:pb.Method)=>'/'+m.parent!.fullName.replace(/^\./,'')+'/'+m.name;
export const bytes=(s:string)=>Uint8Array.from(atob(s),c=>c.charCodeAt(0));
export function base64(data:Uint8Array){let s='';for(let i=0;i<data.length;i+=32768)s+=String.fromCharCode(...data.subarray(i,i+32768));return btoa(s)}
export function frame(data:Uint8Array){const framed=new Uint8Array(data.length+5);new DataView(framed.buffer).setUint32(1,data.length);framed.set(data,5);return framed}
export function frames(data:Uint8Array,partial=false){const result:{compressed:boolean;data:Uint8Array}[]=[];let offset=0;while(offset<data.length){if(data.length-offset<5){if(partial)break;throw Error('Incomplete gRPC message header');}const flag=data[offset];if(flag>1)throw Error('Invalid gRPC compression flag (gRPC-Web is not supported)');const size=new DataView(data.buffer,data.byteOffset+offset+1,4).getUint32(0);offset+=5;if(size>data.length-offset){if(partial)break;throw Error('Incomplete gRPC message body');}result.push({compressed:flag===1,data:data.subarray(offset,offset+size)});offset+=size}return result}
export function encodeMessage(method:pb.Method,text:string){method.resolve();const type=method.resolvedRequestType!;const value=JSON.parse(text);const message=type.fromObject(value);const error=type.verify(message);if(error)throw Error(error);return frame(type.encode(message).finish())}
export function prepareGrpc(draft:RequestDraft,method:pb.Method,text:string):RequestDraft {
 const url=new URL(draft.url);url.pathname=methodPath(method);url.search='';url.hash='';
 const headers=draft.headers.filter(h=>!['content-type','content-length','transfer-encoding','te','grpc-accept-encoding','grpc-encoding'].includes(h.name.toLowerCase()));
 headers.push({name:'content-type',value:'application/grpc+proto'},{name:'te',value:'trailers'},{name:'grpc-accept-encoding',value:'identity,gzip'});
 return {...draft,method:'POST',engine:'h2',pseudoHeaders:[],url:url.toString(),headers,bodyBase64:base64(encodeMessage(method,text))};
}
const MAX_GRPC_DECODED_BYTES=8*1024*1024;
async function decodeGzipMessage(data:Uint8Array,limit:number){
 const stream=new Blob([new Uint8Array(data).buffer]).stream().pipeThrough(new DecompressionStream('gzip'));
 const reader=stream.getReader();const chunks:Uint8Array[]=[];let size=0;
 try {
  while(true){const {done,value}=await reader.read();if(done)break;
   size+=value.length;
   if(size>limit){try{await reader.cancel()}catch{}throw Error('gRPC 解压后正文超过 8 MiB，请查看 Base64 或导出原始字节');}
   chunks.push(value);
  }
 }finally{reader.releaseLock();}
 const result=new Uint8Array(size);let offset=0;
 for(const chunk of chunks){result.set(chunk,offset);offset+=chunk.length;}
 return result;
}
export async function decodeMessages(body:string,method:pb.Method|undefined,headers:Header[],request=false,partial=false){
 const result=[];method?.resolve();const type=request?method?.resolvedRequestType:method?.resolvedResponseType;let decodedBytes=0;
 for(const message of frames(bytes(body),partial)){let data=message.data;
  if(message.compressed){const encoding=headers.find(h=>h.name.toLowerCase()==='grpc-encoding')?.value;if(encoding!=='gzip')throw Error('Unsupported gRPC message encoding: '+encoding);data=await decodeGzipMessage(data,MAX_GRPC_DECODED_BYTES-decodedBytes);}
  if(data.length>MAX_GRPC_DECODED_BYTES-decodedBytes)throw Error('gRPC 解码正文超过 8 MiB，请查看 Base64 或导出原始字节');
  decodedBytes+=data.length;
  result.push(type?type.toObject(type.decode(data),{longs:String,enums:String,bytes:String,defaults:true}):rawFields(data));
 }return result;
}
/** Schema-free view: field numbers and wire values, never invented field names/types. */
export function rawFields(data:Uint8Array){const r=pb.Reader.create(data),fields:unknown[]=[];while(r.pos<r.len){const tag=r.uint32(),field=tag>>>3,wire=tag&7;if(!field)throw Error('Invalid protobuf field 0');let value:unknown;switch(wire){case 0:value=r.uint64().toString();break;case 1:value=r.fixed64().toString();break;case 2:{const b=r.bytes();let text:string|undefined;try{text=new TextDecoder('utf-8',{fatal:true}).decode(b)}catch{}value={base64:base64(b),...(text===undefined?{}:{text})};break;}case 5:value=r.fixed32();break;default:throw Error('Unsupported protobuf wire type '+wire)}fields.push({field,wire,value})}return fields}
export function grpcStatus(headers:Header[],trailers:Header[]=[]){const all=[...headers,...trailers];const codes=all.filter(h=>h.name.toLowerCase()==='grpc-status');const value=codes.at(-1)?.value;const names=['OK','CANCELLED','UNKNOWN','INVALID_ARGUMENT','DEADLINE_EXCEEDED','NOT_FOUND','ALREADY_EXISTS','PERMISSION_DENIED','RESOURCE_EXHAUSTED','FAILED_PRECONDITION','ABORTED','OUT_OF_RANGE','UNIMPLEMENTED','INTERNAL','UNAVAILABLE','DATA_LOSS','UNAUTHENTICATED'];const code=value===undefined?undefined:Number(value);let message=all.filter(h=>h.name.toLowerCase()==='grpc-message').at(-1)?.value??'';try{message=decodeURIComponent(message)}catch{}return {code,name:code!==undefined&&Number.isInteger(code)&&code>=0&&code<names.length?names[code]:'Missing or invalid grpc-status',message}}

export const isGrpcContentType=(value:string)=>/^application\/grpc(?:\+[a-z0-9._-]+)?$/i.test(value.split(";")[0].trim());
