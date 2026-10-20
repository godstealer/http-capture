import type {Flow, Header} from './types';
import {bytesBase64, rawBodyParser, textBody, utf8Body, utf8ContentType} from './requestBody';

export type RewriteAction = 'replace' | 'modify';
export interface Modification {
  target:'body'|'header';operation:'replace'|'set'|'remove';name:string;find:string;value:string;
  regex:boolean;caseSensitive:boolean;all:boolean;
}
export interface Replacement {status:number;headers:Header[];bodyBase64:string}
export interface RewriteRule {
  id:string;name:string;enabled:boolean;matchType:'urlContains'|'urlRegex'|'host';pattern:string;method:string;
  action:RewriteAction;replacement:Replacement|null;modifications:Modification[];
}
export interface RewriteConfig {enabled:boolean;scope:'all'|'capture'|'replay';rules:RewriteRule[]}
export interface RewriteSnapshot {config:RewriteConfig;version:string;hits:number[]}
export interface RewriteSeed {key:string;flow:Flow;action:RewriteAction}
export const MAX_REWRITE_BODY_BYTES=8*1024*1024;
export const MAX_REWRITE_RULES=32;
export class RewriteValidationError extends Error {}
const bytes=(value:string)=>new TextEncoder().encode(value).byteLength;
const token=/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;
const invalidHeaderValue=/[\u0000-\u0008\u000a-\u001f\u007f]/;
const managedHeaders=new Set(['content-length','transfer-encoding','connection']);
const invalidatedHeaders=new Set(['content-length','transfer-encoding','etag','content-md5','digest','content-digest','repr-digest','last-modified','content-range','accept-ranges']);
export const emptyRewriteConfig=():RewriteConfig=>({enabled:false,scope:'all',rules:[]});
export function newModification():Modification {
  return {target:'body',operation:'replace',name:'',find:'',value:'',regex:false,caseSensitive:true,all:true};
}
export function newRewriteRule(action:RewriteAction,id=crypto.randomUUID()):RewriteRule {
  return {id,name:action==='replace'?'替换响应':'修改响应',enabled:false,matchType:'urlContains',pattern:'',method:'',action,
    replacement:action==='replace'?{status:200,headers:[{name:'Content-Type',value:'text/plain; charset=utf-8'}],bodyBase64:''}:null,
    modifications:action==='modify'?[newModification()]:[]};
}
function requireValue(condition:unknown,message:string):asserts condition {if(!condition)throw new RewriteValidationError(message)}
export function rewriteBodyLength(encoded:string):number {
  requireValue(encoded.length<=Math.ceil(MAX_REWRITE_BODY_BYTES/3)*4,'替换正文不能超过 8 MiB。');
  requireValue(encoded.length%4===0&&/^[A-Za-z0-9+/]*(?:={1,2})?$/.test(encoded),'替换正文不是有效的 Base64。');
  let binary:string;
  try{binary=atob(encoded)}catch{throw new RewriteValidationError('替换正文不是有效的 Base64。')}
  requireValue(btoa(binary)===encoded,'替换正文不是有效的 Base64。');
  requireValue(binary.length<=MAX_REWRITE_BODY_BYTES,'替换正文不能超过 8 MiB。');
  return binary.length;
}
export function replacementEncoding(rule:RewriteRule):'text'|'base64' {
  const replacement=rule.replacement;
  return !replacement||(!replacement.headers.some(h=>!h.disabled&&h.name.toLowerCase()==='content-encoding'&&h.value.trim().toLowerCase()!=='identity')&&utf8Body(replacement.bodyBase64)!==null)?'text':'base64';
}
export function replacementParser(rule:RewriteRule) {
  const mime=rule.replacement?.headers.find(h=>!h.disabled&&h.name.toLowerCase()==='content-type')?.value??'';
  return rawBodyParser(mime);
}
export function seedRewriteRule(flow:Flow,action:RewriteAction,id=crypto.randomUUID()):RewriteRule {
  requireValue(!!flow.response&&!flow.websocket,'此记录没有可用的普通 HTTP 响应。');
  const rule=newRewriteRule(action,id),url=flow.request.url.split('#')[0];
  rule.name=(action==='replace'?'替换响应':'修改响应')+' · '+flow.request.method;
  rule.matchType='urlRegex';rule.pattern='^'+url.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')+'$';rule.method=flow.request.method;
  if(action==='replace'){
    rewriteBodyLength(flow.response.bodyBase64);
    rule.replacement={status:flow.response.status,headers:structuredClone(flow.response.headers),bodyBase64:flow.response.bodyBase64};
  }
  return rule;
}
export function copyRewriteRule(rule:RewriteRule,id=crypto.randomUUID()):RewriteRule {
  return {...structuredClone(rule),id,name:Array.from(rule.name+' · 副本').slice(0,128).join(''),enabled:false};
}
export function moveRewriteRule<T>(items:readonly T[],index:number,direction:-1|1):T[] {
  const next=[...items],target=index+direction;
  if(!Number.isInteger(index)||index<0||index>=next.length||target<0||target>=next.length)return next;
  [next[index],next[target]]=[next[target],next[index]];return next;
}
function bodyHeaders(headers:Header[],text:boolean):Header[] {
  return headers.filter(header=>header.disabled||(!invalidatedHeaders.has(header.name.toLowerCase())&&(!text||header.name.toLowerCase()!=='content-encoding')))
    .map(header=>text&&!header.disabled&&header.name.toLowerCase()==='content-type'?{...header,value:utf8ContentType(header.value)}:{...header});
}
export function updateReplacementBody(rule:RewriteRule,value:string,encoding:'text'|'base64'):RewriteRule {
  requireValue(rule.action==='replace'&&!!rule.replacement,'请先选择整份响应替换规则。');
  requireValue(encoding==='base64'?value.length<=Math.ceil(MAX_REWRITE_BODY_BYTES/3)*4:bytes(value)<=MAX_REWRITE_BODY_BYTES,'替换正文不能超过 8 MiB。');
  const bodyBase64=encoding==='text'?textBody(value):value;
  if(bodyBase64===rule.replacement.bodyBase64)return rule;
  return {...rule,replacement:{...rule.replacement,bodyBase64,headers:bodyHeaders(rule.replacement.headers,encoding==='text')}};
}
export function replacementFromFile(rule:RewriteRule,data:Uint8Array,mime:string):RewriteRule {
  requireValue(data.byteLength<=MAX_REWRITE_BODY_BYTES,'替换正文不能超过 8 MiB。');
  requireValue(rule.action==='replace'&&!!rule.replacement,'请先选择整份响应替换规则。');
  const bodyBase64=bytesBase64(data),contentType=mime||'application/octet-stream';
  const enabled=rule.replacement.headers.filter(header=>!header.disabled),types=enabled.filter(header=>header.name.toLowerCase()==='content-type');
  if(bodyBase64===rule.replacement.bodyBase64&&types.length===1&&types[0].value===contentType&&!enabled.some(header=>header.name.toLowerCase()==='content-encoding'))return rule;
  const headers=bodyHeaders(rule.replacement.headers,true).filter(h=>h.disabled||h.name.toLowerCase()!=='content-type');
  return {...rule,replacement:{...rule.replacement,headers:[...headers,{name:'Content-Type',value:contentType}],bodyBase64}};
}
/** Validate only the shared structural contract here. Rust compiles its own
 * regular-expression syntax when saving; JavaScript RegExp is not a substitute. */
export function validatedRewriteConfig(value:RewriteConfig):RewriteConfig {
  requireValue(typeof value.enabled==='boolean'&&['all','capture','replay'].includes(value.scope),'自动响应规则配置无效。');
  requireValue(Array.isArray(value.rules)&&value.rules.length<=MAX_REWRITE_RULES,'最多允许 32 条自动响应规则。');
  const result=structuredClone(value),ids=new Set<string>();
  for(const rule of result.rules){
    requireValue(typeof rule.id==='string'&&rule.id.length<=128&&token.test(rule.id)&&!ids.has(rule.id),'自动响应规则标识无效或重复。');ids.add(rule.id);
    requireValue(typeof rule.name==='string'&&Array.from(rule.name).length<=128&&typeof rule.enabled==='boolean','规则名称不能超过 128 个字符。');
    requireValue(['urlContains','urlRegex','host'].includes(rule.matchType)&&typeof rule.pattern==='string'&&!!rule.pattern.trim()&&bytes(rule.pattern)<=2048,'请填写匹配内容，且不能超过 2048 字节。');
    requireValue(typeof rule.method==='string'&&(rule.method===''||(rule.method.length<=32&&token.test(rule.method))),'请求方法必须为空或有效的方法名称。');
    requireValue(rule.action==='replace'||rule.action==='modify','自动响应操作类型无效。');
    requireValue(Array.isArray(rule.modifications)&&rule.modifications.length<=32,'每条规则最多允许 32 项修改。');
    if(rule.action==='replace'){
      const replacement=rule.replacement;
      requireValue(!!replacement&&Number.isInteger(replacement.status)&&replacement.status>=200&&replacement.status<=599,'替换响应状态码必须为 200–599 的整数。');
      requireValue(Array.isArray(replacement.headers),'替换响应头格式无效。');
      replacement.headers=replacement.headers.filter(h=>h.name!==''||h.value!=='');
      requireValue(replacement.headers.length<=256,'替换响应头最多允许 256 项。');
      let headerBytes=0;
      for(const header of replacement.headers){
        requireValue(typeof header.name==='string'&&token.test(header.name)&&typeof header.value==='string'&&!invalidHeaderValue.test(header.value)&&(header.disabled===undefined||typeof header.disabled==='boolean'),'响应头名称或值无效，请检查换行和控制字符。');
        headerBytes+=bytes(header.name)+bytes(header.value)+4;
      }
      requireValue(headerBytes<=64*1024,'替换响应头不能超过 64 KiB。');
      requireValue(typeof replacement.bodyBase64==='string','替换正文不是有效的 Base64。');rewriteBodyLength(replacement.bodyBase64);
    }else{
      requireValue(rule.replacement===null&&rule.modifications.length>0,'修改响应规则至少需要一项修改。');
      for(const modification of rule.modifications){
        requireValue(['body','header'].includes(modification.target)&&['replace','set','remove'].includes(modification.operation),'响应修改操作无效。');
        requireValue(['name','find','value'].every(key=>typeof modification[key as 'name'|'find'|'value']==='string')&&['regex','caseSensitive','all'].every(key=>typeof modification[key as 'regex'|'caseSensitive'|'all']==='boolean'),'响应修改操作无效。');
        requireValue(bytes(modification.find)<=1024*1024&&bytes(modification.value)<=1024*1024,'查找或替换内容不能超过 1 MiB。');
        requireValue(modification.target!=='body'||modification.operation==='replace','正文只支持文本或正则替换。');
        if(modification.target==='header'){
          requireValue(token.test(modification.name)&&!managedHeaders.has(modification.name.toLowerCase()),'请填写有效的响应头名称；长度、传输编码和连接头由内核管理。');
          requireValue(!invalidHeaderValue.test(modification.value),'响应头名称或值无效，请检查换行和控制字符。');
        }
        if(modification.operation==='replace')requireValue(!!modification.find,'请填写要查找的文本或正则表达式。');
      }
    }
  }
  requireValue(bytes(JSON.stringify(result))<=16*1024*1024,'全部自动响应规则不能超过 16 MiB。');
  return result;
}
