import * as pb from 'protobufjs';
import * as descriptor from 'protobufjs/ext/descriptor';
import {invoke} from './api';
import {schema,prepareGrpc,base64,encodeMessage,decodeMessages,grpcStatus} from './grpc';
import type {Flow,RequestDraft} from './types';
// Wire-compatible subset of the public gRPC reflection protocol; field numbers are stable in v1/v1alpha.
export const reflectionProto=`syntax="proto3"; package grpc.reflection.v1;
message ServerReflectionRequest {string host=1;oneof message_request {string file_by_filename=3;string file_containing_symbol=4;string list_services=7;}}
message ServerReflectionResponse {string valid_host=1;ServerReflectionRequest original_request=2;oneof message_response {FileDescriptorResponse file_descriptor_response=4;ListServiceResponse list_services_response=6;ErrorResponse error_response=7;}}
message FileDescriptorResponse {repeated bytes file_descriptor_proto=1;}
message ListServiceResponse {repeated ServiceResponse service=1;}
message ServiceResponse {string name=1;}
message ErrorResponse {int32 error_code=1;string error_message=2;}
service ServerReflection {rpc ServerReflectionInfo(stream ServerReflectionRequest) returns(stream ServerReflectionResponse);}`;
export async function reflect(draft:RequestDraft,version:'v1'|'v1alpha'='v1'):Promise<string>{
 const method=schema([reflectionProto.replace(/grpc.reflection.v1/g,'grpc.reflection.'+version)]).methods[0];
 const request=prepareGrpc({...draft,scripts:undefined},method,JSON.stringify({list_services:''}));
 const id=await invoke<string>('grpc_start',{request,halfClose:false});let seen=0;
 async function next():Promise<any>{const deadline=Date.now()+15000;while(Date.now()<deadline){const flow=await invoke<Flow|null>('grpc_poll',{id});if(flow?.error)throw Error(flow.error);if(flow?.response){const result=await decodeMessages(flow.response.bodyBase64,method,flow.response.headers,false,true);if(result.length>seen){const value=result[seen++] as Record<string,any>;if(value.error_response)throw Error('Reflection: '+value.error_response.error_message);return value;}
 if(['completed','failed','cancelled'].includes(flow.state??'')){const status=grpcStatus(flow.response.headers,flow.response.trailers);throw Error('Reflection '+status.code+': '+status.name+' '+status.message)}}await new Promise(r=>setTimeout(r,100))}throw Error('Reflection timeout')}
 async function query(value:object){await invoke('grpc_send',{id,body:base64(encodeMessage(method,JSON.stringify(value))),end:false});return next()}
 try{
  const first=await next();const services=first.list_services_response?.service??[];const files=new Map<string,any>();
  const collect=(answer:any)=>{for(const encoded of answer.file_descriptor_response?.file_descriptor_proto??[]){const file=descriptor.FileDescriptorProto.decode(Uint8Array.from(atob(encoded),c=>c.charCodeAt(0)));if(file.name)files.set(file.name,file)}};
  for(const service of services)collect(await query({file_containing_symbol:service.name}));
  const requested=new Set<string>();
  while(true){const missing=[...files.values()].flatMap(f=>f.dependency??[]).find(name=>!files.has(name));if(!missing)break;if(requested.has(missing))throw Error('Missing reflected dependency: '+missing);requested.add(missing);collect(await query({file_by_filename:missing}));}
  if(!files.size)throw Error('Reflection returned no descriptors');
  return JSON.stringify(pb.Root.fromDescriptor({file:[...files.values()]},{keepCase:true}).toJSON());
 }catch(error){if(version==='v1'&&String(error).includes('Reflection 12:'))return reflect(draft,'v1alpha');throw error}
 finally{await invoke('cancel_replay',{executionId:id}).catch(()=>{})}
}
