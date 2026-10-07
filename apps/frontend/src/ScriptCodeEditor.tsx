import { t, useLanguage } from './i18n';
import { json } from '@codemirror/lang-json';
import { parseVariables } from './scriptVariables';
import type { ScriptModules } from './types';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Compartment, EditorState } from '@codemirror/state';
import { EditorView, lineNumbers, keymap, highlightActiveLine, highlightActiveLineGutter, drawSelection } from '@codemirror/view';
import { foldGutter, foldKeymap, syntaxHighlighting, defaultHighlightStyle, HighlightStyle, bracketMatching, indentOnInput, syntaxTree } from '@codemirror/language';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { javascript, localCompletionSource } from '@codemirror/lang-javascript';
import { autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap, snippetCompletion, startCompletion, type CompletionContext, type Completion } from '@codemirror/autocomplete';
const highlight = HighlightStyle.define(defaultHighlightStyle.specs.map(spec=>({...spec,...(spec.color?{color:`var(--syntax-${String(spec.color).slice(1)}, ${spec.color})`}:{})})));
const functions = [
 ['sha256','text','(text: string) → hex','UTF-8 字符串的 SHA-256，64 位小写十六进制。'],
 ['md5','text','(text: string) → hex','UTF-8 字符串的 MD5，32 位小写十六进制。'],
 ['hmacSha256','key, text','(key: string, text: string) → hex','使用 UTF-8 密钥计算 HMAC-SHA256。参数顺序：密钥、消息。'],
 ['hmacMd5','key, text','(key: string, text: string) → hex','使用 UTF-8 密钥计算 HMAC-MD5。参数顺序：密钥、消息。'],
 ['encodeText','text','(text: string) → base64','将 UTF-8 文本编码成 Base64。'],
 ['decodeText','base64','(base64: string) → text','将 Base64 解码成 UTF-8；不会自动解压。'],
 ['assert','condition, message','(condition, message?) → void','条件为 false 时终止请求并显示错误。'],
];
const moduleHelp:Record<string,[string,string]>={
 'encoding.convert':['(input, {from, to})','编码格式：utf8、base64、base64url、hex。保留二进制字节。'],
 'crypto.hash':['(algorithm, input, options?)','算法：MD5、SHA-1、SHA-256、SHA-512；options 支持 inputEncoding 和 output。'],
 'crypto.hmac':['(algorithm, {key, message, keyEncoding?, inputEncoding?, output?})','默认 UTF-8 输入和密钥，hex 输出。'],
 'crypto.encrypt':['({mode, key, keyEncoding, nonce, nonceEncoding, data, inputEncoding, output, aad?, aadEncoding?})','AES-GCM：16/32 字节密钥、12 字节 nonce；输出为密文拼接 16 字节认证标签。'],
 'crypto.decrypt':['({mode, key, keyEncoding, nonce, nonceEncoding, data, inputEncoding, output, aad?, aadEncoding?})','AES-GCM：输入为密文拼接认证标签，参数需与加密一致。'],
 'crypto.randomBytes':['(length, output = "hex")','安全随机数；1–65536 字节，output 可用 hex、base64、base64url。'],
 'utils.timestamp':['(unit = "seconds")','返回数值时间戳。可选 seconds 或 milliseconds。'],
 'utils.uuid':['()','使用安全随机数生成 UUID v4。'],
};
function completions(context:CompletionContext, stage:string, variables:string[],modules:ScriptModules) {
 const node=syntaxTree(context.state).resolveInner(context.pos,-1);if(/String|Comment/.test(node.name))return null;
 const member=context.matchBefore(/(?:request\.headers|response\.headers|request|response|variables|encoding|crypto|utils|console|Date|JSON|Math)\.\w*/);
 if(member){const dot=member.text.lastIndexOf('.');const object=member.text.slice(0,dot);if(object.startsWith('response')&&stage==='before')return null;
 if((object==='encoding'||object==='crypto'||object==='utils')&&!modules[object])return null;
 const names:Record<string,string[]>={encoding:['convert','base64Encode','base64Decode','base64urlEncode','base64urlDecode','hexEncode','hexDecode'],crypto:['hash','hmac','encrypt','decrypt','randomBytes'],utils:['timestamp','uuid'],request:['method','url','headers','bodyBase64'],response:['status','headers','bodyBase64'],variables,console:['log'],Date:['now'],JSON:['parse','stringify'],Math:['floor','round','random'], 'request.headers':['push','map','filter','find','findIndex','splice'], 'response.headers':['push','map','filter','find','findIndex','splice']};
 return {from:member.from+dot+1,options:(names[object]??[]).map(label=>({label,detail:moduleHelp[object+'.'+label]?.[0],type:((object==='encoding'||object==='crypto'||object==='utils')?'function':['log','now','parse','stringify','floor','round','random','push','map','filter','find','findIndex','splice'].includes(label)?'function':'property'),info:(moduleHelp[object+'.'+label]?.[1] ? t(moduleHelp[object+'.'+label][1]) : undefined)??(object==='encoding'?t("输入和输出使用 utf8 / base64 / base64url / hex，默认文本为 UTF-8。"):undefined)??(label==='headers'?t("有序 {name, value}[]，保留重复字段"):label==='bodyBase64'?t("正文原始 Base64；用 encodeText 写入文本"):undefined)})),validFor:/^\w*$/};}
 const word=context.matchBefore(/\w*/);if(!word||word.from===word.to&&!context.explicit)return null;
 const options:Completion[]=functions.map(([label,args,detail,info])=>snippetCompletion(`${label}(${args.split(', ').map(a=>'${'+a+'}').join(', ')})`,{label,type:'function',detail,info:t(info)}));
 options.push(...[...Object.keys(modules).filter(k=>modules[k as keyof ScriptModules]),'request','variables','console','Date','JSON','Math',...(stage==='after'?['response']:[])].map(label=>({label,type:'variable'})));
 return {from:word.from,options,validFor:/^\w*$/};
}
export default function ScriptCodeEditor({value,onChange,stage,variables,modules={encoding:true,crypto:true,utils:true}}:{value:string;onChange:(text:string)=>void;stage:'before'|'after'|'variables';variables:Record<string,string>;modules?:ScriptModules}) {
 const { language: uiLanguage } = useLanguage();
 const attributes = useRef(new Compartment());
 const parent=useRef<HTMLDivElement>(null);const view=useRef<EditorView>();const props=useRef({onChange,variables,modules});props.current={onChange,variables,modules};
 const worker=useRef<Worker>();const formatting=useRef(false);const formatAction=useRef(()=>{});const [busy,setBusy]=useState(false);const [message,setMessage]=useState('');const [position,setPosition]=useState({ line: 1, column: 1 });
 const jsonError=useMemo(()=>{if(stage!=='variables')return '';try{parseVariables(value);return '';}catch(e){return e instanceof Error?e.message:String(e);}},[value,stage]);
 function formatCode(){const editor=view.current;if(!editor||formatting.current)return;const original=editor.state.doc.toString();if(new TextEncoder().encode(original).length>65536){setMessage(t("脚本超过 64 KiB，无法格式化"));return;}formatting.current=true;setBusy(true);setMessage('');
 const w=new Worker(new URL('./script-format.worker.ts',import.meta.url),{type:'module'});worker.current=w;
 const done=()=>{clearTimeout(timer);w.terminate();worker.current=undefined;formatting.current=false;setBusy(false);};
 const timer=setTimeout(()=>{done();setMessage(t("格式化超时，原代码已保留"));},8000);
 w.onerror=()=>{done();setMessage(t("格式化失败，原代码已保留"));};
 w.onmessage=e=>{done();if(e.data.error){setMessage(e.data.error);return;}if(!view.current)return;if(view.current.state.doc.toString()!==original){setMessage(t("代码已修改，请重新格式化"));return;}view.current.dispatch({changes:{from:0,to:original.length,insert:e.data.text}});setMessage(t("已格式化 · 可撤销"));};w.postMessage({text:original,parser:stage==='variables'?'json':'babel'});
 }
 formatAction.current=formatCode;
 useEffect(()=>{
  const editor=new EditorView({parent:parent.current!,state:EditorState.create({doc:value,extensions:[stage==='variables'?json():javascript(),lineNumbers(),history(),foldGutter(),bracketMatching(),closeBrackets(),indentOnInput(),drawSelection(),highlightActiveLine(),highlightActiveLineGutter(),syntaxHighlighting(highlight),
   attributes.current.of(EditorView.contentAttributes.of({'aria-label':stage==='variables'?t("脚本变量 JSON"):stage==='before'?t("请求前 JavaScript"):t("响应后 JavaScript"),spellcheck:'false'})),
   autocompletion({override:stage==='variables'?[ctx=>{
    const word=ctx.matchBefore(/[\w"]*/);if(!word||word.from===word.to&&!ctx.explicit)return null;
    const prefix=ctx.state.sliceDoc(0,word.from);if(/:\s*$/.test(prefix))return null;
    return {from:word.from,options:[...new Set(['secret','token','baseUrl',...Object.keys(props.current.variables)])].map(name=>snippetCompletion(`"${name}": "\${value}"`,{label:`"${name}"`,type:'property',detail:t("字符串变量")}))};
   }]:[ctx=>completions(ctx,stage,Object.keys(props.current.variables),props.current.modules),localCompletionSource]}),
   keymap.of([{key:'Shift-Alt-f',run:()=>{formatAction.current();return true;}},...completionKeymap,...closeBracketsKeymap,...defaultKeymap,...historyKeymap,...foldKeymap,indentWithTab]),
   EditorView.updateListener.of(update=>{if(update.docChanged){setMessage('');props.current.onChange(update.state.doc.toString());}if(update.docChanged||update.selectionSet){const pos=update.state.selection.main.head;const line=update.state.doc.lineAt(pos);setPosition({ line: line.number, column: pos-line.from+1 });}}),
   EditorView.theme({'&':{fontSize:'13px',height:stage==='variables'?'180px':'300px',backgroundColor:'var(--code-gutter, #fafafa)'},'.cm-scroller':{overflow:'auto',fontFamily:'Consolas, monospace',lineHeight:'1.7'},'.cm-gutters':{backgroundColor:'var(--code-gutter, #fafafa)',color:'var(--code-muted, #888)',borderRight:'1px solid var(--code-border, #ddd)'},'.cm-activeLine,.cm-activeLineGutter':{backgroundColor:'var(--code-active, #fff4df)'},'.cm-content':{padding:'10px 0'},'.cm-line':{padding:'0 12px'},'&.cm-focused':{outline:'none'},'.cm-tooltip':{backgroundColor:'var(--code-gutter, #fafafa)',color:'inherit',border:'1px solid var(--code-border, #ddd)'},'.cm-tooltip-autocomplete ul li[aria-selected]':{backgroundColor:'var(--code-active, #fff4df)',color:'inherit'}})
  ]})});view.current=editor;
  return()=>{worker.current?.terminate();editor.destroy();view.current=undefined;};
 // Stage changes remount this component; incoming edits are synchronized below without losing selection or undo history.
 },[stage]);
 useEffect(() => { view.current?.dispatch({ effects: attributes.current.reconfigure(EditorView.contentAttributes.of({ 'aria-label': stage === 'variables' ? t('脚本变量 JSON') : stage === 'before' ? t('请求前 JavaScript') : t('响应后 JavaScript'), spellcheck: 'false' })) }); }, [uiLanguage, stage]);
 useEffect(()=>{const editor=view.current;if(editor&&editor.state.doc.toString()!==value)editor.dispatch({changes:{from:0,to:editor.state.doc.length,insert:value}});},[value]);
 return <div className="script-code"><div className="script-code-toolbar"><span>{stage==='variables'?t("JSON · 字符串变量"):'JavaScript'}</span><button type="button" disabled={busy} onClick={formatCode}>{busy?t("格式化中…"):t("格式化")}</button><button type="button" onClick={()=>{if(view.current){view.current.focus();startCompletion(view.current);}}}>{t("代码提示")}</button><span className="script-shortcuts">{t("Shift+Alt+F 格式化 · Ctrl+Space 提示")}</span></div><div ref={parent}/><div className="script-code-status"><span>{t("行 {v0}，列 {v1}", { v0: position.line, v1: position.column })} {t("· Tab 缩进 · 可折叠")}</span><span role={jsonError?'alert':'status'}>{jsonError||message}</span></div></div>;
}
