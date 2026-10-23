import {useEffect,useRef,useState} from 'react';
import {invoke} from './api';
import {t} from './i18n';
import {bytesBase64} from './requestBody';
import './certificates.css';
type Kind='ca'|'client'|'trust';
interface Item{id:string;name:string;kind:Kind;host:string;port:number;enabled:boolean;certificate:{subject:string;issuer:string;sha256:string;notBefore:string;notAfter:string}}
interface Snapshot{entries:Item[];version:string;selectedCa:string|null;activeCa:string|null;restartRequired:boolean}
const labels={ca:'抓包 CA',client:'客户端证书',trust:'服务器信任'};
export default function Certificates({connected,active=true}:{connected:boolean;active?:boolean}){
 const [snapshot,setSnapshot]=useState<Snapshot>(),[kind,setKind]=useState<Kind>('ca'),[name,setName]=useState(''),[host,setHost]=useState(''),[port,setPort]=useState('443');
 const [certificate,setCertificate]=useState(''),[privateKey,setPrivateKey]=useState(''),[certName,setCertName]=useState(''),[keyName,setKeyName]=useState('');
 const [p12,setP12]=useState(false),[encryptedKey,setEncryptedKey]=useState(false),[password,setPassword]=useState('');
 const [busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState(''),[confirmation,setConfirmation]=useState<{action:string;id:string|null;enabled?:boolean;version:string}>();
 const generation=useRef(0),certInput=useRef<HTMLInputElement>(null),keyInput=useRef<HTMLInputElement>(null),busyNow=useRef(false),certRead=useRef(0),keyRead=useRef(0);
 const context=useRef({connected,active});context.current={connected,active};
 function clearFiles(){certRead.current++;keyRead.current++;setCertificate('');setPrivateKey('');setCertName('');setKeyName('');setP12(false);setEncryptedKey(false);setPassword('');if(certInput.current)certInput.current.value='';if(keyInput.current)keyInput.current.value='';}
 useEffect(()=>{const ticket=++generation.current;setConfirmation(undefined);setError('');setNotice('');clearFiles();
  if(active&&connected)void invoke<Snapshot>('certificates').then(value=>{if(ticket===generation.current)setSnapshot(value)}).catch(error=>{if(ticket===generation.current)setError(String(error))});
  return()=>{generation.current++;certRead.current++;keyRead.current++};
 },[connected,active]);
 async function load(){if(busyNow.current||!connected)return;const ticket=++generation.current;setError('');try{const value=await invoke<Snapshot>('certificates');if(ticket===generation.current)setSnapshot(value)}catch(error){if(ticket===generation.current)setError(String(error))}}
 async function read(file:File,isKey:boolean){
  const counter=isKey?keyRead:certRead,ticket=++counter.current,epoch=generation.current;setError('');
  if(isKey){setPrivateKey('');setKeyName('');setEncryptedKey(false)}else{setCertificate('');setCertName('');setP12(false);setPassword('')}
  if(file.size>1024*1024){setError(t('证书或私钥文件不能超过 1 MiB。'));return;}
  try{const bytes=new Uint8Array(await file.arrayBuffer());if(ticket!==counter.current||epoch!==generation.current||!context.current.active||!context.current.connected)return;
   if(isKey){const value=new TextDecoder('utf-8',{fatal:true}).decode(bytes);if(!value.includes('-----BEGIN '))throw Error(t('请选择 PEM 私钥文件。'));setPrivateKey(value);setEncryptedKey(value.includes('ENCRYPTED'));setKeyName(file.name)}
   else{const container=/\.(p12|pfx)$/i.test(file.name);setCertificate(bytesBase64(bytes));setCertName(file.name);setP12(container);if(container){keyRead.current++;setPrivateKey('');setKeyName('');setEncryptedKey(false);if(keyInput.current)keyInput.current.value=''}}
  }catch(error){if(ticket===counter.current&&epoch===generation.current)setError(String(error))}
 }
 async function mutate(command:'import_certificate'|'change_certificate',input:Record<string,unknown>){
  if(busyNow.current||!snapshot||!active||!connected)return;
  const ticket=++generation.current;busyNow.current=true;setBusy(true);setError('');setNotice('');setConfirmation(undefined);
  try{const value=await invoke<Snapshot>(command,{input});if(ticket===generation.current){setSnapshot(value);clearFiles();setNotice(command==='import_certificate'?t('证书已导入，默认未启用。'):t('证书配置已保存。'));}}
  catch(error){if(ticket===generation.current)setError(String(error))}
  finally{busyNow.current=false;setBusy(false)}
 }
 function importFile(){const number=Number(port);if(kind!=='ca'&&(!/^\d+$/.test(port)||!Number.isInteger(number)||number<1||number>65535)){setError(t('端口必须为 1–65535 的整数。'));return;}
  void mutate('import_certificate',{name,kind,host:kind==='ca'?'':host,port:kind==='ca'?443:number,certificateBase64:certificate,privateKeyPem:kind==='trust'||p12?'':privateKey,...(p12?{containerFormat:'p12'}:{}),...(p12||encryptedKey?{password}:{}),expectedVersion:snapshot?.version});
 }
 function confirm(action:string,id:string|null,enabled=false){if(snapshot)setConfirmation({action,id,enabled,version:snapshot.version})}
 if(!active)return null;
 return <section className="certificate-manager settings-section">
  <div className="certificate-heading"><h3>{t('证书管理')}</h3><button type="button" disabled={!connected||busy} onClick={()=>void load()}>{t('刷新')}</button></div>
  <p>{t('分别管理抓包签发身份、客户端身份和服务器信任；私钥不会进入请求记录或全局配置导出。')}</p>
  {!connected&&<p role="status">{t('连接内核后可管理证书。')}</p>}
  {snapshot?.restartRequired&&<p className="format-note" role="status">{t('抓包 CA 变更将在重启内核后生效；当前连接继续使用原 CA。重启后请在客户端信任新 CA。')}</p>}
  {error&&<p role="alert" className="bad">{error}</p>}{notice&&<p role="status">{notice}</p>}
  <div className="certificate-kinds" role="group" aria-label={t('证书用途')}>{(['ca','client','trust']as const).map(value=><button type="button" key={value} aria-pressed={kind===value} disabled={busy} onClick={()=>{setKind(value);setConfirmation(undefined);clearFiles();setError('');setNotice('')}}>{t(labels[value])}</button>)}</div>
  <p>{kind==='ca'?t('导入自己的 CA 证书和配套私钥，用于签发抓包证书；仅导入不会替换当前 CA。'):kind==='client'?t('为精确域名和端口配置 mTLS 身份。Native、Auto、H2、H3、gRPC、WebSocket、wreq 和新版 httpcloak helper 可使用。'):t('添加验证上游 HTTPS 服务器的信任 CA；仍验证证书链、有效期和域名。域名留空时适用于所有目标。')}</p>
  <p className="muted">{t('支持 PEM / DER 证书、P12/PFX 容器和加密 PEM 私钥；解锁密码仅用于本次导入，不保存。Windows 使用当前用户加密保护，其他系统使用受限本机文件。')}</p>
  <fieldset disabled={busy||!connected||!snapshot} className="certificate-form"><legend>{t('导入证书')}</legend>
   <label>{t('名称')}<input value={name} maxLength={100} onChange={event=>setName(event.target.value)}/></label>
   {kind!=='ca'&&<><label>{t('精确域名或 IP')}<input value={host} maxLength={253} placeholder={kind==='client'?'api.example.com':t('留空表示所有目标')} onChange={event=>setHost(event.target.value)}/></label><label>{t('端口')}<input type="text" inputMode="numeric" value={port} onChange={event=>setPort(event.target.value.replace(/\D/g,''))}/></label></>}
   <label>{t('公钥证书或 P12/PFX 文件')}<input ref={certInput} type="file" accept=".pem,.crt,.cer,.der,.p12,.pfx" onChange={event=>{const file=event.target.files?.[0];if(file)void read(file,false)}}/>{certName&&<small>{certName}</small>}</label>
   {kind!=='trust'&&!p12&&<label>{t('配套私钥文件')}<input ref={keyInput} type="file" accept=".pem,.key" onChange={event=>{const file=event.target.files?.[0];if(file)void read(file,true)}}/>{keyName&&<small>{keyName}</small>}</label>}
   {(p12||encryptedKey)&&<label>{t('证书或私钥密码')}<input type="password" autoComplete="off" value={password} onChange={event=>setPassword(event.target.value)} maxLength={1024}/></label>}
   <button type="button" disabled={!name.trim()||!certificate||(kind!=='trust'&&!p12&&!privateKey)||(kind==='client'&&!host.trim())} onClick={importFile}>{busy?t('处理中…'):t('导入并保存')}</button>
  </fieldset>
  {kind==='ca'&&snapshot&&<div className="setting-card"><div><strong>{t('内置抓包 CA')}</strong><p>{snapshot.activeCa===null?t('当前使用'):t('保留原内置 CA，可切回使用。')}</p></div><button type="button" disabled={busy||!connected||snapshot.selectedCa===null} onClick={()=>confirm('selectCa',null)}>{snapshot.selectedCa===null?t('已选择'):t('切换为此 CA')}</button></div>}
  <div className="certificate-list">{snapshot?.entries.filter(item=>item.kind===kind).map(item=><article className="setting-card certificate-item" key={item.id}>
   <div><strong>{item.name}</strong><p>{item.host?`${item.host}:${item.port}`:item.kind==='trust'?t('所有目标'):item.certificate.subject}</p><p>{t('有效期至')}: {item.certificate.notAfter}</p><code>SHA-256: {item.certificate.sha256}</code><p>{t('签发者')}: {item.certificate.issuer}</p>{item.kind==='ca'&&<p>{snapshot.activeCa===item.id?t('当前使用'):snapshot.selectedCa===item.id?t('重启后使用'):t('未选择')}</p>}</div>
   <div className="certificate-actions">{item.kind==='ca'?<button type="button" disabled={busy||!connected||snapshot.selectedCa===item.id} onClick={()=>confirm('selectCa',item.id)}>{t('切换为此 CA')}</button>:<button type="button" disabled={busy||!connected} onClick={()=>confirm('enable',item.id,!item.enabled)}>{item.enabled?t('停用'):t('启用')}</button>}<button type="button" disabled={busy||!connected||item.id===snapshot.activeCa||item.id===snapshot.selectedCa} onClick={()=>confirm('delete',item.id)}>{t('删除')}</button></div>
  </article>)}</div>
  {confirmation&&<div className="setting-card certificate-confirm" role="alert"><p>{confirmation.action==='selectCa'?t('确认切换抓包 CA？仅在重启内核后使用，不会自动修改系统或其他设备的证书信任。'):confirmation.action==='delete'?t('确认删除此证书配置？删除后不能在本工具中恢复，请保留原始文件。'):confirmation.enabled?t('确认启用此证书配置？客户端身份只用于匹配域名和端口；服务器 CA 会扩展对应目标的信任。'):t('确认停用此证书配置？新的 TLS 连接将不再使用。')}</p><button type="button" disabled={busy||!connected} onClick={()=>void mutate('change_certificate',{action:confirmation.action,id:confirmation.id,enabled:confirmation.enabled??false,expectedVersion:confirmation.version})}>{t('确认')}</button><button type="button" disabled={busy} onClick={()=>setConfirmation(undefined)}>{t('取消')}</button></div>}
 </section>;
}
