import {resolveEnvironment} from './library';
import type {RequestDraft} from './types';
export function grpcScope(draft:RequestDraft,environment:Record<string,string>){return {...environment,...draft.scripts?.variables}}
export function resolveGrpcJson(text:string,scope:Record<string,string>):string {
 const replace=(value:unknown):unknown=>{
  if(typeof value==='string')return value.replace(/\{\{\s*([^{}]+?)\s*\}\}/g,(_,name:string)=>{if(!Object.prototype.hasOwnProperty.call(scope,name))throw Error(`Undefined variable: ${name}`);return scope[name]});
  if(Array.isArray(value))return value.map(replace);
  if(value&&typeof value==='object')return Object.fromEntries(Object.entries(value).map(([k,v])=>[k,replace(v)]));
  return value;
 };
 return JSON.stringify(replace(JSON.parse(text)));
}
export function resolveGrpcEndpoint(draft:RequestDraft,environment:Record<string,string>):RequestDraft {
 // The existing body is framed protobuf. Never interpolate its bytes as text.
 const resolved=resolveEnvironment({...draft,bodyBase64:''},environment);
 return {...resolved,bodyBase64:draft.bodyBase64};
}
