import type * as pb from 'protobufjs';
import type {FlowMetadata,Header} from './types';
import {decodeMessageChunks} from './grpc';
import {RECORDED_BODY_PAGE_BYTES,recordedBodyRequest,validateRecordedBodyPage,type RecordedBodyRead} from './recordedBodyModel';

/** Read only the versioned response slot; never hydrate the other body slots. */
export async function decodeRecordedGrpcMessages(flow:FlowMetadata,method:pb.Method|undefined,headers:Header[],partial:boolean,
  read:(request:RecordedBodyRead)=>Promise<unknown>,options:{signal?:AbortSignal;isCurrent?:()=>boolean}={}){
  recordedBodyRequest(flow,'response',0);
  const total=flow.bodyReferences.response!.byteLength;
  const current=()=>{if(options.signal?.aborted||options.isCurrent&&!options.isCurrent())throw new DOMException('gRPC body read cancelled','AbortError')};
  async function* pages(){
    for(let page=0;page<Math.max(1,Math.ceil(total/RECORDED_BODY_PAGE_BYTES));page++){
      current();const request=recordedBodyRequest(flow,'response',page);
      const raw=await read(request);current();
      yield validateRecordedBodyPage(raw,request,total).bytes;
    }
  }
  const messages=await decodeMessageChunks(pages(),method,headers,false,partial);
  current();return messages;
}
