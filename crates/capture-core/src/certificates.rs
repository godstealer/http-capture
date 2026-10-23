//! Local certificate identities. Private material never enters Flow, SQLite,
//! request JSON, configuration migration, or API snapshots.
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use openssl::{pkcs12::Pkcs12,pkey::PKey};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::{Path, PathBuf}, sync::{Arc, Mutex}};
use x509_parser::prelude::*;

const MAX_FILE: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 64;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
struct Entry { id:String, name:String, kind:String, host:String, port:u16, enabled:bool, certificate_pem:String, private_key_pem:String }
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
struct Vault { entries:Vec<Entry>, selected_ca:Option<String>, version:String }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct CertificateEntry { pub id:String,pub name:String,pub kind:String,pub host:String,pub port:u16,pub enabled:bool,pub certificate:crate::model::CertificateDetails }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct Snapshot {pub entries:Vec<CertificateEntry>,pub version:String,pub selected_ca:Option<String>,pub active_ca:Option<String>,pub restart_required:bool}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Import { pub name:String,pub kind:String,#[serde(default)]pub host:String,#[serde(default="https_port")]pub port:u16,pub certificate_base64:String,#[serde(default)]pub private_key_pem:String,#[serde(default)]pub container_format:Option<String>,#[serde(default)]pub password:Option<String>,pub expected_version:String }
fn https_port()->u16{443}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Change {pub action:String,#[serde(default)]pub id:Option<String>,#[serde(default)]pub enabled:bool,pub expected_version:String}
pub struct Certificates {path:PathBuf,state:Mutex<Vault>,active_ca:Option<String>}

/// Ephemeral material for one exact destination, never a serializable request field.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Security {pub roots:Vec<Vec<u8>>,pub identity:Option<(String,String)>}
tokio::task_local! {pub(crate) static CURRENT: Security;}
pub(crate) fn current()->Security {CURRENT.try_with(Clone::clone).unwrap_or_default()}
impl Security {
 pub(crate) fn material(&self)->Option<transport_api::TlsMaterial>{
  if self.roots.is_empty()&&self.identity.is_none(){return None}
  Some(transport_api::TlsMaterial{trusted_ca_pem:self.roots.iter().map(|der|pem("CERTIFICATE",der)).collect(),client_certificate_pem:self.identity.as_ref().map(|i|i.0.clone()),client_private_key_pem:self.identity.as_ref().map(|i|i.1.clone())})
 }
}

pub(crate) fn chain(pem:&str)->Result<Vec<CertificateDer<'static>>>{
 ensure!(!pem.contains("PRIVATE KEY"),"公钥证书文件不应包含私钥，请分别选择证书和私钥文件");
 let certs=rustls_pemfile::certs(&mut pem.as_bytes()).collect::<std::result::Result<Vec<_>,_>>().context("证书 PEM 格式无效")?;
 ensure!(!certs.is_empty()&&certs.len()<=16,"证书链应包含 1–16 张证书");Ok(certs)
}
pub(crate) fn key(pem:&str)->Result<PrivateKeyDer<'static>>{
 ensure!(!pem.contains("ENCRYPTED"),"请先将加密私钥转换为未加密的 PEM；不支持直接导入加密私钥或 P12/PFX");
 rustls_pemfile::private_key(&mut pem.as_bytes()).context("私钥 PEM 格式无效")?.context("缺少 PEM 私钥")
}
fn pem(label:&str,bytes:&[u8])->String{let b64=STANDARD.encode(bytes);format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n",b64.as_bytes().chunks(64).map(|c|std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("\n"))}
fn imported_material(input:&Import,data:&[u8])->Result<(String,String)>{
 ensure!(input.password.as_ref().is_none_or(|value|value.len()<=1024),"证书密码不能超过 1024 字节");
 ensure!(input.password.as_ref().is_none_or(|value|!value.contains('\0')),"证书密码不能包含空字符");
 if let Some(format)=input.container_format.as_deref(){
  ensure!(format=="p12","证书容器格式无效");
  ensure!(input.private_key_pem.is_empty(),"P12/PFX 已包含私钥，请勿再选择单独的私钥文件");
  let parsed=Pkcs12::from_der(data).and_then(|container|container.parse2(input.password.as_deref().unwrap_or("")))
   .map_err(|_|anyhow::anyhow!("P12/PFX 无法解锁或格式无效"))?;
  let certificate=parsed.cert.context("P12/PFX 缺少证书")?;
  let mut certificate_pem=String::from_utf8(certificate.to_pem()?)?;
  if let Some(chain)=parsed.ca{
   ensure!(chain.len()<16,"证书链应包含 1–16 张证书");
   for cert in chain{certificate_pem.push_str(&String::from_utf8(cert.to_pem()?)?);}
  }
  let private_key_pem=match parsed.pkey{
   Some(key)=>{ensure!(input.kind!="trust","服务器信任只需要 CA 公钥证书，请使用不含私钥的证书文件");String::from_utf8(key.private_key_to_pem_pkcs8()?)?},
   None=>{ensure!(input.kind=="trust","P12/PFX 缺少私钥");String::new()},
  };
  return Ok((certificate_pem,private_key_pem));
 }
 let certificate_pem=if data.starts_with(b"-----BEGIN"){String::from_utf8(data.to_vec()).context("证书 PEM 应为 UTF-8")?}else{pem("CERTIFICATE",data)};
 let private_key_pem=if input.kind=="trust"{input.private_key_pem.clone()}else{
  ensure!(input.private_key_pem.len()<=MAX_FILE,"私钥文件不能超过 1 MiB");
  let key_pem=if input.private_key_pem.contains("ENCRYPTED"){
   let password=input.password.as_deref().context("请输入加密私钥的密码")?;
   let key=PKey::private_key_from_pem_passphrase(input.private_key_pem.as_bytes(),password.as_bytes())
    .map_err(|_|anyhow::anyhow!("加密私钥无法解锁或格式无效"))?;
   String::from_utf8(key.private_key_to_pem_pkcs8()?)?
  }else{input.private_key_pem.clone()};
  key(&key_pem)?;rcgen::KeyPair::from_pem(&key_pem).context("不支持此 PEM 私钥")?.serialize_pem()
 };
 Ok((certificate_pem,private_key_pem))
}
fn normalize_host(value:&str)->Result<String>{
 ensure!(!value.contains(['/', '@', '*', '?', '#', '%'])&&!value.chars().any(char::is_whitespace),"请填写精确域名或 IP，不含协议、路径或通配符");
 let host=url::Host::parse(value.trim_end_matches('.')).context("域名或 IP 无效")?;
 Ok(host.to_string().trim_matches(['[',']']).to_ascii_lowercase())
}
fn validate(entry:&Entry)->Result<()> {
 validate_at(entry,true)
}
fn validate_at(entry:&Entry,check_dates:bool)->Result<()> {
 ensure!(matches!(entry.kind.as_str(),"ca"|"client"|"trust"),"证书用途无效");
 ensure!(!entry.name.trim().is_empty()&&entry.name.chars().count()<=100,"名称应为 1–100 个字符");
 ensure!(entry.certificate_pem.len()<=MAX_FILE&&entry.private_key_pem.len()<=MAX_FILE,"单个证书或私钥不能超过 1 MiB");
 if entry.kind!="ca" {ensure!(entry.port>0,"端口必须为 1–65535");if entry.kind=="client"||!entry.host.is_empty(){ensure!(normalize_host(&entry.host)?==entry.host,"证书目标域名格式无效")}}
 let certs=chain(&entry.certificate_pem)?;
 for (index,der) in certs.iter().enumerate() {let (remaining,cert)=X509Certificate::from_der(der.as_ref()).map_err(|_|anyhow::anyhow!("X.509 证书无效"))?;ensure!(remaining.is_empty(),"证书包含额外数据");ensure!(!check_dates||cert.validity().is_valid(),"证书尚未生效或已经过期");
  if entry.kind!="client"||index>0{ensure!(cert.basic_constraints()?.is_some_and(|e|e.value.ca),"证书链中的签发证书必须是 CA");if let Some(usage)=cert.key_usage()?{ensure!(usage.value.key_cert_sign(),"CA 证书不允许签发证书")}}
 }
 let (_,leaf)=X509Certificate::from_der(certs[0].as_ref()).map_err(|_|anyhow::anyhow!("X.509 证书无效"))?;
 if matches!(entry.kind.as_str(),"ca"|"trust") {
  ensure!(leaf.basic_constraints()?.is_some_and(|e|e.value.ca),"该证书不是 CA 证书");
  if let Some(usage)=leaf.key_usage()?{ensure!(usage.value.key_cert_sign(),"CA 证书不允许签发证书")}
 }else{
  ensure!(!leaf.basic_constraints()?.is_some_and(|e|e.value.ca),"客户端身份应使用终端证书，不能使用 CA 证书");
  if let Some(usage)=leaf.extended_key_usage()?{ensure!(usage.value.client_auth||usage.value.any,"证书不允许客户端身份认证")}
 }
 if entry.kind=="trust"{ensure!(entry.private_key_pem.is_empty(),"服务器信任只需要 CA 公钥证书，请勿导入私钥");}
 else{
  let signing=rustls::crypto::ring::sign::any_supported_type(&key(&entry.private_key_pem)?).context("不支持此私钥算法")?;
  rustls::sign::CertifiedKey::new(certs,signing).keys_match().context("证书与私钥不匹配")?;
  if entry.kind=="ca"{rcgen::KeyPair::from_pem(&entry.private_key_pem).context("抓包 CA 私钥算法不受支持")?;}
 }
 Ok(())
}
impl Certificates {
 pub fn open(data_dir:&Path)->Result<Arc<Self>>{
  let directory=data_dir.join("certificate-manager");fs::create_dir_all(&directory)?;
  #[cfg(unix)]{use std::os::unix::fs::PermissionsExt;fs::set_permissions(&directory,fs::Permissions::from_mode(0o700))?;}
  let path=directory.join("identities.v1");
  let state=if path.exists(){ensure!(fs::metadata(&path)?.len()<=160*1024*1024,"证书库过大");serde_json::from_slice::<Vault>(&unprotect(&fs::read(&path)?)?).context("无法打开本机证书库")?}else{Vault{version:uuid::Uuid::new_v4().to_string(),..Default::default()}};
  ensure!(state.entries.len()<=MAX_ENTRIES,"证书数量超过限制");
  // Expired identities remain manageable; selected identities are validated at use.
  let active_ca=state.selected_ca.clone();Ok(Arc::new(Self{path,state:Mutex::new(state),active_ca}))
 }
 pub fn snapshot(&self)->Result<Snapshot>{let mut state=self.state.lock().unwrap();self.refresh(&mut state)?;self.snapshot_locked(&state)}
 fn refresh(&self,state:&mut Vault)->Result<()>{
  if self.path.exists(){let raw=fs::read(&self.path)?;ensure!(raw.len()<=160*1024*1024,"证书库过大");let next:Vault=serde_json::from_slice(&unprotect(&raw)?).context("证书库无效")?;ensure!(next.entries.len()<=MAX_ENTRIES,"证书数量超过限制");*state=next;}Ok(())
 }
 fn snapshot_locked(&self,state:&Vault)->Result<Snapshot>{
  let entries=state.entries.iter().map(|e|Ok(CertificateEntry{id:e.id.clone(),name:e.name.clone(),kind:e.kind.clone(),host:e.host.clone(),port:e.port,enabled:e.enabled,certificate:crate::tls_details::certificates(&chain(&e.certificate_pem)?).remove(0)})).collect::<Result<_>>()?;
  Ok(Snapshot{entries,version:state.version.clone(),selected_ca:state.selected_ca.clone(),active_ca:self.active_ca.clone(),restart_required:state.selected_ca!=self.active_ca})
 }
 fn persist(&self,next:&Vault,expected:&str)->Result<()> {
  let mut options=fs::OpenOptions::new();options.create(true).write(true).truncate(false);
  #[cfg(unix)]{use std::os::unix::fs::OpenOptionsExt;options.mode(0o600);}
  let lock=options.open(self.path.with_extension("lock"))?;lock.try_lock().context("另一个进程正在更新证书，请稍后重试")?;
  let mut disk=Vault::default();self.refresh(&mut disk)?;ensure!(!self.path.exists()||disk.version==expected,"证书配置已在其他进程修改，请刷新后重试");
  let bytes=protect(&serde_json::to_vec(next)?)?;
  let mut file=tempfile::NamedTempFile::new_in(self.path.parent().unwrap())?;
  file.write_all(&bytes)?;file.as_file().sync_all()?;file.persist(&self.path).map_err(|_|anyhow::anyhow!("保存证书库失败"))?;Ok(())
 }
 pub fn import(&self,input:Import)->Result<Snapshot>{
  ensure!(input.certificate_base64.len()<=MAX_FILE*4/3+4,"证书文件不能超过 1 MiB");
  let data=STANDARD.decode(&input.certificate_base64).context("证书文件 Base64 无效")?;
  ensure!(data.len()<=MAX_FILE,"证书文件不能超过 1 MiB");
  let (certificate_pem,private_key_pem)=imported_material(&input,&data)?;
  let host=if input.kind=="ca"||input.host.is_empty(){input.host}else{normalize_host(&input.host)?};
  let entry=Entry{id:uuid::Uuid::new_v4().to_string(),name:input.name.trim().into(),kind:input.kind,host,port:input.port,enabled:false,certificate_pem,private_key_pem};
  validate(&entry)?;
  let mut state=self.state.lock().unwrap();ensure!(input.expected_version==state.version,"证书配置已在其他窗口修改，请刷新后重试");ensure!(state.entries.len()<MAX_ENTRIES,"最多保存 64 个证书配置");
  let mut next=state.clone();next.entries.push(entry);next.version=uuid::Uuid::new_v4().to_string();self.persist(&next,&state.version)?;*state=next;self.snapshot_locked(&state)
 }
 pub fn change(&self,input:Change)->Result<Snapshot>{
  let mut state=self.state.lock().unwrap();ensure!(input.expected_version==state.version,"证书配置已在其他窗口修改，请刷新后重试");let mut next=state.clone();
  match input.action.as_str(){
   "selectCa"=>{if let Some(id)=&input.id{let entry=next.entries.iter().find(|e|&e.id==id&&e.kind=="ca").context("抓包 CA 不存在")?;validate(entry)?;}next.selected_ca=input.id;},
   "enable"=>{let entry=next.entries.iter_mut().find(|e|Some(&e.id)==input.id.as_ref()&&e.kind!="ca").context("证书配置不存在")?;if input.enabled{validate(entry)?;}entry.enabled=input.enabled;
    let clients:Vec<_>=next.entries.iter().filter(|e|e.enabled&&e.kind=="client").collect();let mut seen=std::collections::HashSet::new();ensure!(clients.iter().all(|e|seen.insert((&e.host,e.port))),"同一域名和端口只能启用一个客户端身份");},
   "delete"=>{ensure!(input.id.is_some(),"请选择证书");ensure!(input.id!=self.active_ca&&input.id!=next.selected_ca,"请先切换抓包 CA 并重启，再删除原 CA");let before=next.entries.len();next.entries.retain(|e|Some(&e.id)!=input.id.as_ref());ensure!(before!=next.entries.len(),"证书配置不存在");},
   _=>anyhow::bail!("证书操作无效")
  }
  next.version=uuid::Uuid::new_v4().to_string();self.persist(&next,&state.version)?;*state=next;self.snapshot_locked(&state)
 }
 pub(crate) fn authority(&self,builtin_dir:&Path)->Result<crate::ca::CertificateAuthority>{
  let state=self.state.lock().unwrap();if let Some(id)=&self.active_ca{
   let entry=state.entries.iter().find(|e|&e.id==id&&e.kind=="ca").context("选中的抓包 CA 不存在")?;validate_at(entry,false)?;
   let public=self.path.parent().unwrap().join(format!("public-{id}.pem"));fs::write(&public,&entry.certificate_pem)?;
   crate::ca::CertificateAuthority::from_pem(&entry.certificate_pem,&entry.private_key_pem,public)
  }else{crate::ca::CertificateAuthority::load_or_create(builtin_dir)}
 }
 pub(crate) fn security(&self,url:&url::Url)->Result<Security>{
  let host=url.host_str().unwrap_or_default().trim_matches(['[',']']).trim_end_matches('.').to_ascii_lowercase();let port=url.port_or_known_default().unwrap_or(443);
  if !matches!(url.scheme(),"https"|"wss"){return Ok(Security::default())}
  let mut state=self.state.lock().unwrap();self.refresh(&mut state)?;let mut security=Security::default();
  for entry in state.entries.iter().filter(|e|e.enabled&&e.kind!="ca"&&((e.kind=="trust"&&e.host.is_empty())||(e.host==host&&e.port==port))){
   validate(entry).with_context(||format!("证书配置不可用：{}",entry.name))?;
   if entry.kind=="trust"{security.roots.extend(chain(&entry.certificate_pem)?.into_iter().map(|c|c.to_vec()));}
   else{ensure!(security.identity.is_none(),"客户端证书匹配冲突");security.identity=Some((entry.certificate_pem.clone(),entry.private_key_pem.clone()));}
  }Ok(security)
 }
 pub(crate) fn config(&self,url:&url::Url,profile:&crate::model::TlsProfile,roots:&rustls::RootCertStore,engine:&str)->Result<rustls::ClientConfig>{
  configure(profile,roots,engine,&self.security(url)?)
 }
}
pub(crate) fn configure(profile:&crate::model::TlsProfile,roots:&rustls::RootCertStore,engine:&str,security:&Security)->Result<rustls::ClientConfig>{
 crate::tls_config::base_config(profile,roots,engine,security)
}

#[cfg(not(windows))]fn protect(data:&[u8])->Result<Vec<u8>>{Ok(data.to_vec())}
#[cfg(not(windows))]fn unprotect(data:&[u8])->Result<Vec<u8>>{Ok(data.to_vec())}
#[cfg(windows)]fn protect(data:&[u8])->Result<Vec<u8>>{windows_protect(data,true)}
#[cfg(windows)]fn unprotect(data:&[u8])->Result<Vec<u8>>{windows_protect(data,false)}
#[cfg(windows)]fn windows_protect(data:&[u8],encrypt:bool)->Result<Vec<u8>>{
 use windows_sys::Win32::{Foundation::LocalFree,Security::Cryptography::{CRYPT_INTEGER_BLOB,CryptProtectData,CryptUnprotectData,CRYPTPROTECT_UI_FORBIDDEN}};
 let input=CRYPT_INTEGER_BLOB{cbData:data.len().try_into()?,pbData:data.as_ptr() as *mut u8};let mut output=CRYPT_INTEGER_BLOB{cbData:0,pbData:std::ptr::null_mut()};
 let ok=unsafe{if encrypt{CryptProtectData(&input,std::ptr::null(),std::ptr::null(),std::ptr::null(),std::ptr::null(),CRYPTPROTECT_UI_FORBIDDEN,&mut output)}else{CryptUnprotectData(&input,std::ptr::null_mut(),std::ptr::null(),std::ptr::null(),std::ptr::null(),CRYPTPROTECT_UI_FORBIDDEN,&mut output)}};
 ensure!(ok!=0,"无法使用当前 Windows 用户保护证书库");let result=unsafe{std::slice::from_raw_parts(output.pbData,output.cbData as usize).to_vec()};unsafe{LocalFree(output.pbData.cast());}Ok(result)
}

#[cfg(test)]mod tests{
 use super::*;
 fn ca()->(rcgen::Certificate,rcgen::KeyPair){let mut p=rcgen::CertificateParams::new(vec!["test-ca".into()]).unwrap();p.is_ca=rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);p.key_usages=vec![rcgen::KeyUsagePurpose::KeyCertSign];let k=rcgen::KeyPair::generate().unwrap();(p.self_signed(&k).unwrap(),k)}
 fn import(store:&Certificates,kind:&str,cert:&str,key:&str,host:&str)->Snapshot{store.import(Import{name:kind.into(),kind:kind.into(),host:host.into(),port:443,certificate_base64:STANDARD.encode(cert),private_key_pem:key.into(),container_format:None,password:None,expected_version:store.snapshot().unwrap().version}).unwrap()}
 fn change(store:&Certificates,action:&str,id:Option<String>,enabled:bool)->Result<Snapshot>{store.change(Change{action:action.into(),id,enabled,expected_version:store.snapshot()?.version})}
 #[test]fn identities_are_persistent_private_disabled_and_versioned(){
  let dir=tempfile::tempdir().unwrap();let store=Certificates::open(dir.path()).unwrap();let(ca,key)=ca();
  let first=import(&store,"ca",&ca.pem(),&key.serialize_pem(),"");assert!(!first.entries[0].enabled);assert!(first.selected_ca.is_none());
  let public=serde_json::to_string(&first).unwrap();assert!(!public.contains("PRIVATE KEY"));assert!(!public.contains("privateKey"));assert!(!public.contains(&key.serialize_pem()));
  let selected=change(&store,"selectCa",Some(first.entries[0].id.clone()),false).unwrap();assert!(selected.restart_required);assert!(selected.active_ca.is_none());
  assert!(store.change(Change{action:"selectCa".into(),id:None,enabled:false,expected_version:first.version}).is_err());
  let reopened=Certificates::open(dir.path()).unwrap();let state=reopened.snapshot().unwrap();assert!(!state.restart_required);assert_eq!(state.active_ca,selected.selected_ca);
  let loaded=reopened.authority(&dir.path().join("builtin")).unwrap();assert_eq!(fs::read_to_string(loaded.cert_path).unwrap(),ca.pem());
  assert!(change(&reopened,"delete",state.active_ca.clone(),false).is_err());
  #[cfg(windows)]assert!(!String::from_utf8_lossy(&fs::read(&store.path).unwrap()).contains("PRIVATE KEY"));
  #[cfg(unix)]{use std::os::unix::fs::PermissionsExt;assert_eq!(fs::metadata(&store.path).unwrap().permissions().mode()&0o777,0o600);}
 }
 #[test]fn invalid_identity_and_ca_usage_are_rejected_without_mutation(){
  let dir=tempfile::tempdir().unwrap();let store=Certificates::open(dir.path()).unwrap();let(cert,key)=ca();let wrong=rcgen::KeyPair::generate().unwrap();let version=store.snapshot().unwrap().version;
  let leaf=rcgen::CertificateParams::new(vec![]).unwrap().signed_by(&wrong,&cert,&key).unwrap();
  for(kind,pem,key)in[("ca",cert.pem(),wrong.serialize_pem()),("trust",cert.pem(),key.serialize_pem()),("client",cert.pem(),key.serialize_pem()),("trust",cert.pem()+&key.serialize_pem(),String::new()),("trust",cert.pem()+&leaf.pem(),String::new())]{
   assert!(store.import(Import{name:"bad".into(),kind:kind.into(),host:"localhost".into(),port:443,certificate_base64:STANDARD.encode(pem),private_key_pem:key,container_format:None,password:None,expected_version:version.clone()}).is_err());
  }
  assert_eq!(store.snapshot().unwrap().entries.len(),0);assert_eq!(store.snapshot().unwrap().version,version);
 }
 #[test]fn scope_is_exact_identity_conflicts_fail_and_trust_never_contains_private_material(){
  let dir=tempfile::tempdir().unwrap();let store=Certificates::open(dir.path()).unwrap();let(ca,key)=ca();
  let client_key=rcgen::KeyPair::generate().unwrap();let mut p=rcgen::CertificateParams::new(vec![]).unwrap();p.extended_key_usages=vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];let client=p.signed_by(&client_key,&ca,&key).unwrap();
  let first=import(&store,"client",&client.pem(),&client_key.serialize_pem(),"EXAMPLE.COM.");let id=first.entries[0].id.clone();change(&store,"enable",Some(id.clone()),true).unwrap();
  assert!(store.security(&url::Url::parse("https://example.com/").unwrap()).unwrap().identity.is_some());
  for value in ["https://sub.example.com/","https://example.com:444/","https://example.com.attacker.invalid/","http://example.com/"]{assert!(store.security(&url::Url::parse(value).unwrap()).unwrap().identity.is_none());}
  let second=import(&store,"client",&client.pem(),&client_key.serialize_pem(),"example.com");let second_id=second.entries.last().unwrap().id.clone();assert!(change(&store,"enable",Some(second_id),true).is_err());
  change(&store,"enable",Some(id),false).unwrap();
  let trust=import(&store,"trust",&ca.pem(),"","example.com");change(&store,"enable",Some(trust.entries.last().unwrap().id.clone()),true).unwrap();
  let selected=store.security(&url::Url::parse("https://example.com/").unwrap()).unwrap();assert_eq!(selected.roots.len(),1);assert!(selected.material().unwrap().client_private_key_pem.is_none());
  assert!(store.security(&url::Url::parse("https://other.invalid/").unwrap()).unwrap().roots.is_empty());
 }
 #[test]fn concurrent_instances_do_not_overwrite_and_refresh_recovers(){
  let dir=tempfile::tempdir().unwrap();let a=Certificates::open(dir.path()).unwrap();let b=Certificates::open(dir.path()).unwrap();let stale=b.snapshot().unwrap().version;let(ca,_)=ca();import(&a,"trust",&ca.pem(),"","");
  assert!(b.import(Import{name:"stale".into(),kind:"trust".into(),host:"".into(),port:443,certificate_base64:STANDARD.encode(ca.pem()),private_key_pem:"".into(),container_format:None,password:None,expected_version:stale}).is_err());
  assert_eq!(b.snapshot().unwrap().entries.len(),1);let latest=import(&b,"trust",&ca.pem(),"","localhost");assert_eq!(latest.entries.len(),2);assert_eq!(a.snapshot().unwrap().entries.len(),2);
 }
 #[test]fn expired_ca_is_rejected_on_import_and_der_trust_is_supported(){
  let dir=tempfile::tempdir().unwrap();let store=Certificates::open(dir.path()).unwrap();let(ca,key)=ca();let mut p=rcgen::CertificateParams::new(vec![]).unwrap();p.is_ca=rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);p.not_before=rcgen::date_time_ymd(2000,1,1);p.not_after=rcgen::date_time_ymd(2001,1,1);let expired=p.self_signed(&key).unwrap();
  assert!(store.import(Import{name:"expired".into(),kind:"trust".into(),host:"".into(),port:443,certificate_base64:STANDARD.encode(expired.pem()),private_key_pem:"".into(),container_format:None,password:None,expected_version:store.snapshot().unwrap().version}).is_err());
  let state=store.import(Import{name:"DER".into(),kind:"trust".into(),host:"".into(),port:443,certificate_base64:STANDARD.encode(ca.der()),private_key_pem:"".into(),container_format:None,password:None,expected_version:store.snapshot().unwrap().version}).unwrap();assert_eq!(state.entries.len(),1);
 }
 #[test]fn encrypted_pem_and_p12_import_unlock_without_saving_password(){
  use openssl::{pkcs12::Pkcs12,symm::Cipher,x509::X509};
  let dir=tempfile::tempdir().unwrap();let store=Certificates::open(dir.path()).unwrap();let(cert,key)=ca();
  let pkey=PKey::private_key_from_pem(key.serialize_pem().as_bytes()).unwrap();
  let encrypted=String::from_utf8(pkey.private_key_to_pem_pkcs8_passphrase(Cipher::aes_256_cbc(),b"pem-secret").unwrap()).unwrap();
  let import_encrypted=|password:&str|store.import(Import{name:"encrypted".into(),kind:"ca".into(),host:"".into(),port:443,certificate_base64:STANDARD.encode(cert.pem()),private_key_pem:encrypted.clone(),container_format:None,password:Some(password.into()),expected_version:store.snapshot().unwrap().version});
  assert!(import_encrypted("wrong").is_err());assert!(store.snapshot().unwrap().entries.is_empty());
  assert_eq!(import_encrypted("pem-secret").unwrap().entries.len(),1);
  let cert_openssl=X509::from_pem(cert.pem().as_bytes()).unwrap();
  let archive=Pkcs12::builder().name("test ca").pkey(&pkey).cert(&cert_openssl).build2("p12-secret").unwrap().to_der().unwrap();
  let import_p12=|password:&str|store.import(Import{name:"archive".into(),kind:"ca".into(),host:"".into(),port:443,certificate_base64:STANDARD.encode(&archive),private_key_pem:String::new(),container_format:Some("p12".into()),password:Some(password.into()),expected_version:store.snapshot().unwrap().version});
  let version=store.snapshot().unwrap().version;
  assert!(import_p12("wrong").is_err());assert_eq!(store.snapshot().unwrap().version,version);
  assert!(import_p12("bad\0password").is_err());assert_eq!(store.snapshot().unwrap().version,version);
  assert_eq!(import_p12("p12-secret").unwrap().entries.len(),2);
  let saved=String::from_utf8(unprotect(&fs::read(&store.path).unwrap()).unwrap()).unwrap();
  assert!(!saved.contains("pem-secret")&&!saved.contains("p12-secret")&&!saved.contains("ENCRYPTED PRIVATE KEY"));
  assert!(!serde_json::to_string(&store.snapshot().unwrap()).unwrap().contains("PRIVATE KEY"));
 }
 #[test]fn p12_client_identity_keeps_chain_and_exact_destination_scope(){
  use openssl::{pkcs12::Pkcs12,stack::Stack,x509::X509};
  let dir=tempfile::tempdir().unwrap();let store=Certificates::open(dir.path()).unwrap();let(root,root_key)=ca();
  let leaf_key=rcgen::KeyPair::generate().unwrap();let mut params=rcgen::CertificateParams::new(vec![]).unwrap();params.extended_key_usages=vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
  let leaf=params.signed_by(&leaf_key,&root,&root_key).unwrap();
  let pkey=PKey::private_key_from_pem(leaf_key.serialize_pem().as_bytes()).unwrap();
  let mut root_chain=Stack::new().unwrap();root_chain.push(X509::from_pem(root.pem().as_bytes()).unwrap()).unwrap();
  let archive=Pkcs12::builder().name("client").pkey(&pkey).cert(&X509::from_pem(leaf.pem().as_bytes()).unwrap()).ca(root_chain).build2("secret").unwrap().to_der().unwrap();
  let state=store.import(Import{name:"client".into(),kind:"client".into(),host:"API.EXAMPLE.COM.".into(),port:8443,certificate_base64:STANDARD.encode(archive),private_key_pem:String::new(),container_format:Some("p12".into()),password:Some("secret".into()),expected_version:store.snapshot().unwrap().version}).unwrap();
  assert_eq!(state.entries[0].host,"api.example.com");change(&store,"enable",Some(state.entries[0].id.clone()),true).unwrap();
  let exact=store.security(&url::Url::parse("https://api.example.com:8443/").unwrap()).unwrap().identity.unwrap();
  assert_eq!(chain(&exact.0).unwrap().len(),2);assert!(key(&exact.1).is_ok());
  assert!(store.security(&url::Url::parse("https://api.example.com/").unwrap()).unwrap().identity.is_none());
 }
 #[tokio::test]async fn imported_intermediate_ca_serves_a_chain_accepted_by_original_root(){
  let _=rustls::crypto::ring::default_provider().install_default();
  let(dir,(root,root_key))=(tempfile::tempdir().unwrap(),ca());let store=Certificates::open(dir.path()).unwrap();
  let mut params=rcgen::CertificateParams::new(vec![]).unwrap();params.distinguished_name.push(rcgen::DnType::CommonName,"Imported intermediate");params.is_ca=rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);params.key_usages=vec![rcgen::KeyUsagePurpose::KeyCertSign];let key=rcgen::KeyPair::generate().unwrap();let intermediate=params.signed_by(&key,&root,&root_key).unwrap();
  let state=import(&store,"ca",&(intermediate.pem()+&root.pem()),&key.serialize_pem(),"");change(&store,"selectCa",Some(state.entries[0].id.clone()),false).unwrap();let restarted=Certificates::open(dir.path()).unwrap();let authority=restarted.authority(&dir.path().join("builtin")).unwrap();
  let mut roots=rustls::RootCertStore::empty();roots.add(root.der().clone()).unwrap();let config=rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();let (client,server)=tokio::io::duplex(65536);
  let acceptor=tokio_rustls::TlsAcceptor::from(authority.server_config("localhost").unwrap());let connector=tokio_rustls::TlsConnector::from(Arc::new(config));
  tokio::time::timeout(std::time::Duration::from_secs(5),async{let(a,b)=tokio::join!(acceptor.accept(server),connector.connect("localhost".try_into().unwrap(),client));a.unwrap();let tls=b.unwrap();assert_eq!(tls.get_ref().1.peer_certificates().unwrap().len(),3);}).await.unwrap();
 }
}
