import { writeClipboard } from './clipboard';
import { t } from './i18n';
import { parseVariables } from './scriptVariables';
import { lazy, Suspense, useEffect, useState } from 'react';
import type { Scripts } from './types';
import { invoke } from './api';
import './scripts.css';
const ScriptCodeEditor = lazy(() => import('./ScriptCodeEditor'));
const CodeView = lazy(() => import('./CodeView'));
export const emptyScripts: Scripts = {enabled:false,before:'',after:'',variables:{},modules:{encoding:true,crypto:true,utils:true}};
export default function ScriptsEditor({value,onChange,logs=[]}:{value:Scripts;onChange:(s:Scripts)=>void;logs?:string[]}) {
 const [stage,setStage]=useState<'before'|'after'>('before');
 const [variables,setVariables]=useState<string|null>(null);const [error,setError]=useState('');
 function applyVariables(){try {const parsed=parseVariables(variables??'{}');onChange({...value,variables:parsed as Record<string,string>});setVariables(null);setError('');}catch(e){setError(String(e));}}
 return <div className="scripts-editor"><label><input type="checkbox" checked={value.enabled} onChange={e=>onChange({...value,enabled:e.target.checked})}/>{t("启用 JavaScript 脚本")}</label><div className="script-modules"><span>{t("内置模块")}</span>{(['encoding','crypto','utils'] as const).map(name=><label key={name}><input type="checkbox" checked={value.modules?.[name]??true} onChange={e=>onChange({...value,modules:{encoding:true,crypto:true,utils:true,...value.modules,[name]:e.target.checked}})}/><code>{name}</code></label>)}<small>{t("随应用打包，旧版全局函数始终兼容")}</small></div><p>{t("请求前脚本 → 请求拦截 → 发送 → 响应后脚本 → 响应拦截。每阶段限时 2 秒、内存 64 MiB；仅支持同步 JavaScript，无网络、文件或系统命令接口。错误将终止本次请求。")}</p><div className="tabs">{(['before','after'] as const).map(key=><button key={key} className={stage===key?'tab active':'tab'} onClick={()=>setStage(key)}>{key==='before'?t("请求前脚本"):t("响应后脚本")}</button>)}</div><Suspense fallback={<p>{t("加载脚本编辑器…")}</p>}><ScriptCodeEditor key={stage} stage={stage} modules={value.modules} value={value[stage]} variables={value.variables} onChange={text=>onChange({...value,[stage]:text})}/></Suspense><details><summary>{t("变量（字符串键值；请求前后共用）")}</summary><Suspense fallback={<p>{t("加载变量编辑器…")}</p>}><ScriptCodeEditor stage="variables" variables={Object.fromEntries([...Object.keys(value.variables),...[...(value.before+'\n'+value.after).matchAll(/variables\.(\w+)/g)].map(m=>m[1])].map(k=>[k,'']))} value={variables??JSON.stringify(value.variables,null,2)} onChange={text=>{setVariables(text);setError('');}}/></Suspense><button disabled={variables===null} onClick={applyVariables}>{t("应用变量")}</button>{variables!==null&&<span>{t("变量修改尚未应用")}</span>}{error&&<p role="alert">{error}</p>}</details><ScriptDocs onInsert={(target,code)=>{setStage(target);onChange({...value,[target]:[value[target].trimEnd(),code].filter(Boolean).join('\n\n')});}}/><details open><summary>{t("脚本日志")}</summary><pre>{logs.filter(v=>v.includes('脚本')).join('\n')||t("执行后在此显示日志；错误也会显示在响应区。")}</pre></details></div>;
}
export function CaptureScripts(){
 const [value,setValue]=useState<Scripts>(emptyScripts);const [ready,setReady]=useState(false);const [message,setMessage]=useState('');const [busy,setBusy]=useState(false);
 useEffect(()=>{let live=true;invoke<Scripts>('capture_scripts').then(s=>{if(live){setValue(s);setReady(true);}}).catch(e=>{if(live)setMessage(String(e));});return()=>{live=false;};},[]);
 async function save(){setBusy(true);try{await invoke('set_capture_scripts',{config:value});setMessage(t("抓包脚本已应用"));}catch(e){setMessage(String(e));}finally{setBusy(false);}}
 return <details className="capture-scripts"><summary>{t("抓包自动脚本")}</summary><p>{t("复用上方已应用的匹配规则，独立于手动拦截开关。无匹配规则时不执行。请求前、响应后分别匹配该阶段数据；状态码条件只适用于响应后。配置及变量仅保留至服务退出。")}</p>{ready&&<><ScriptsEditor value={value} onChange={setValue}/><button disabled={busy} onClick={()=>void save()}>{t("应用抓包脚本")}</button><button disabled={busy} onClick={()=>void invoke<Scripts>('capture_scripts').then(s=>{setValue(s);setMessage(t("已加载当前配置及变量"));}).catch(e=>setMessage(String(e)))}>{t("重新加载配置及变量")}</button></>}<p role="status">{message}</p></details>;
}

const examples: Array<{title:string;stage:'before'|'after';code:string}> = [
 {title:'Base64 / Base64URL / Hex',stage:'before',code:`const encoded = encoding.base64Encode("你好");
assert(encoding.base64Decode(encoded) === "你好");
const urlSafe = encoding.base64urlEncode("hello?");
const bytes = encoding.convert("00ff80", { from: "hex", to: "base64" });
console.log(encoded, urlSafe, bytes);`},
 {title:'统一摘要和 HMAC 接口',stage:'before',code:`const timestamp = String(utils.timestamp("seconds"));
const digest = crypto.hash("SHA-512", timestamp, { output: "base64" });
assert(variables.secret, "请配置 secret 字符串变量");
const signature = crypto.hmac("SHA-256", {
  key: variables.secret,
  message: timestamp + request.method,
  output: "base64",
});
request.headers.push({ name: "X-Timestamp", value: timestamp });
request.headers.push({ name: "X-Sign", value: signature });`},
 {title:'AES-GCM 加解密（128 / 256 位）',stage:'before',code:`// 实际接口中 key 应由双方约定；同一密钥下每次加密使用新的 nonce。
const key = crypto.randomBytes(32, "hex");
const nonce = crypto.randomBytes(12, "hex");
const options = {
  mode: "AES-GCM", key, keyEncoding: "hex",
  nonce, nonceEncoding: "hex",
};
const encrypted = crypto.encrypt({
  ...options, data: "hello", inputEncoding: "utf8", output: "base64",
});
const plaintext = crypto.decrypt({
  ...options, data: encrypted, inputEncoding: "base64", output: "utf8",
});
assert(plaintext === "hello");
console.log("AES-GCM 往返验证成功");`},
 {title:'时间戳 + SHA-256',stage:'before',code:`// 时间戳单位与拼接顺序按目标 API 的签名规范调整。
const timestamp = String(Math.floor(Date.now() / 1000));
const sign = sha256(timestamp + (variables.secret || ""));
request.headers.push({ name: "X-Timestamp", value: timestamp });
request.headers.push({ name: "X-Sign", value: sign });`},
 {title:'时间戳 + MD5',stage:'before',code:`const timestamp = String(Date.now());
const sign = md5(timestamp + (variables.secret || ""));
request.headers.push({ name: "X-Timestamp", value: timestamp });
request.headers.push({ name: "X-Sign", value: sign });
// 如果接口要求大写：sign.toUpperCase()`},
 {title:'HMAC-SHA256 密钥签名',stage:'before',code:`assert(variables.secret, "请先配置 secret 变量");
const timestamp = String(Date.now());
const message = request.method + "\\n" + request.url + "\\n" + timestamp;
const sign = hmacSha256(variables.secret, message);
request.headers.push({ name: "X-Timestamp", value: timestamp });
request.headers.push({ name: "X-Sign", value: sign });`},
 {title:'响应断言与替换 JSON',stage:'after',code:`assert(response.status === 200, "预期状态码 200");
// decodeText 适用于未压缩的 UTF-8 正文。
// variables.token = JSON.parse(decodeText(response.bodyBase64)).token;
response.bodyBase64 = encodeText(JSON.stringify({ ok: true }));
response.headers = [{ name: "Content-Type", value: "application/json" }];
console.log("响应已替换", response.status);`},
];
function ScriptDocs({onInsert}:{onInsert:(stage:'before'|'after',code:string)=>void}) {
 const [notice,setNotice]=useState('');
 async function copy(code:string){try{await writeClipboard(code);setNotice(t("示例已复制"));}catch{setNotice(t("无法访问剪贴板，可以在代码块中选择复制"));}}
 return <details className="script-docs"><summary>{t("脚本 API 与示例")}</summary><p>{t("模块随应用离线打包，勾选后在每个阶段创建新环境。统一接口可选择算法与输入/输出编码；旧版全局函数仍使用 UTF-8 输入和小写 hex 输出。AES 目前明确支持 AES-GCM，支持可选 aad / aadEncoding，解密时须保持一致。摘要 / HMAC 与 AES 加解密是不同操作。")}</p><div className="script-api-table"><table><thead><tr><th>{t("函数 / 对象")}</th><th>{t("用途与返回值")}</th></tr></thead><tbody>{[
 ['encoding.base64Encode / base64Decode',t("Base64 与 UTF-8 默认互转；可传第二参数指定源/目标编码")],['encoding.base64urlEncode / base64urlDecode',t("URL 安全的 Base64，输出省略填充，解码兼容有/无填充")],['encoding.hexEncode / hexDecode',t("Hex 与 UTF-8 默认互转；支持二进制编码转换")],['encoding.convert(input, {from, to})',t("格式可选 utf8、base64、base64url、hex")],['crypto.hash(algorithm, input, options?)',t("MD5、SHA-1/256/512；options: inputEncoding、output，默认 utf8 / hex")],['crypto.hmac(algorithm, options)','options: key、message、keyEncoding、inputEncoding、output'],['crypto.encrypt / decrypt(options)',t("AES-GCM；密文后拼接 16 字节认证标签，密钥 16/32 字节，nonce 12 字节")],['crypto.randomBytes(length, output?)',t("安全随机字节，默认 hex，最多 65536 字节")],['utils.timestamp(unit?) / utils.uuid()',t("秒（seconds）/ 毫秒（milliseconds）时间戳；UUID v4")],['sha256(text)',t("SHA-256 摘要，64 位 hex")],['md5(text)',t("MD5 摘要，32 位 hex")],['hmacSha256(key, text)',t("HMAC-SHA256，64 位 hex；密钥在前")],['hmacMd5(key, text)',t("HMAC-MD5，32 位 hex；密钥在前")],['encodeText(text) / decodeText(base64)',t("UTF-8 与 Base64 转换；不自动解压")],['assert(condition, message)',t("断言失败时终止本次请求")],['console.log(...values)',t("记录脚本日志")],['request','method、url、headers、bodyBase64'],['response',t("status、headers、bodyBase64，仅响应后可用")],['variables',t("字符串键值字典，请求前后共用")]
 ].map(([api,desc])=><tr key={api}><td><code>{api}</code></td><td>{desc}</td></tr>)}</tbody></table></div><p><code>headers</code> {t("是有序的")}<code>{'{ name, value }[]'}</code>{t("，保留重复字段。此 API 不兼容")}<code>pm.*</code>{t("。下面的示例可单独使用，不要把多个签名示例叠加执行。")}</p>{examples.map(example=><section className="script-example" key={t(example.title)}><div className="script-code-toolbar"><strong>{t(example.title)}</strong><span>{example.stage==='before'?t("请求前"):t("响应后")}</span><button onClick={()=>void copy(example.code)}>{t("复制示例")}</button><button onClick={()=>{onInsert(example.stage,example.code);setNotice(t("示例已追加到对应脚本，请检查变量名及签名规则"));}}>{t("插入到")}{example.stage==='before'?t("请求前"):t("响应后")}</button></div><div className="script-example-code"><Suspense fallback={<pre><code>{example.code}</code></pre>}><CodeView text={example.code} language="babel" label={example.title+t("代码示例")}/></Suspense></div></section>)}<p role="status">{notice}</p><p>{t("替换响应正文应写入未压缩内容，内核会移除原压缩头并重算长度。手动变量随结果保存；抓包变量在本次运行中共享，同一变量并发写入时后完成的写入生效。")}</p></details>;
}
