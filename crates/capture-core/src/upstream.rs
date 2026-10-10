//! Explicit, session-scoped upstream routing. Credentials never enter Flow records.
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::{RwLock, Mutex, Arc}, path::{Path, PathBuf}};
use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::TcpStream};
use crate::{http1::*, replay::Stream};

#[derive(Clone)]
pub struct UpstreamProxy { profile_id: Option<String>, profile_name: Option<String>, pub url: url::Url, pub username: String, password: String, pub listener: Option<SocketAddr> }
#[derive(Default)]
pub struct UpstreamState { profiles: Mutex<Profiles>, config: RwLock<Option<UpstreamProxy>>, pub listener: RwLock<Option<SocketAddr>> }
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamStatus { pub profile_id: Option<String>, pub profile_name: Option<String>, pub enabled: bool, pub url: String, pub username: String, pub auth_enabled: bool }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpstreamInput { pub enabled: bool, pub url: String, pub username: String, pub auth_enabled: bool, pub password: Option<String> }
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
struct Profile { id:String, name:String, url:String, username:String, auth_enabled:bool,
 #[serde(default)] remember_password:bool,
 #[serde(skip)] credential_error:Option<String>,
 #[serde(skip)] password:Option<String> }
trait CredentialStore: Send + Sync {
 fn read(&self,id:&str)->Result<Option<String>>;
 fn write(&self,id:&str,value:Option<&str>)->Result<()>;
}
struct SystemCredentials;
impl CredentialStore for SystemCredentials {
 fn read(&self,id:&str)->Result<Option<String>> {
  match keyring::Entry::new("http-capture.upstream",id)?.get_password() {
   Ok(value)=>Ok(Some(value)),Err(keyring::Error::NoEntry)=>Ok(None),Err(_)=>anyhow::bail!("无法读取系统凭据库，请解锁凭据库或重新填写密码")
  }
 }
 fn write(&self,id:&str,value:Option<&str>)->Result<()> {
  let entry=keyring::Entry::new("http-capture.upstream",id)?;
  let result=match value {Some(value)=>entry.set_password(value),None=>entry.delete_credential()};
  match result {Ok(())|Err(keyring::Error::NoEntry) if value.is_none()=>Ok(()),Ok(())=>Ok(()),Err(_)=>anyhow::bail!("无法更新系统凭据库，请确认凭据库已解锁且可用")}
 }
}
#[derive(Serialize,Deserialize)]
struct StoredCredential { url:String, username:String, password:String }

struct Profiles { path:Option<PathBuf>, items:Vec<Profile>, credentials:Arc<dyn CredentialStore> }
impl Default for Profiles {fn default()->Self{Self{path:None,items:vec![],credentials:Arc::new(SystemCredentials)}}}
impl Profiles {
 fn commit(&self,items:&[Profile],change:Option<(&str,Option<&str>)>)->Result<()> {
  let previous=if let Some((id,value))=change {
   let previous=self.credentials.read(id)?;self.credentials.write(id,value)?;previous
  }else{None};
  if let Err(error)=self.persist(items) {
   if let Some((id,_))=change {self.credentials.write(id,previous.as_deref()).context("配置保存失败，且系统凭据回滚失败，请重新保存该代理")?;}
   return Err(error.context("保存代理配置失败"));
  }
  Ok(())
 }
 fn restore_passwords(&mut self) {
  for p in &mut self.items {
   if !p.remember_password||!p.auth_enabled||p.password.is_some(){continue;}
   let result=(||->Result<String>{
    let raw=self.credentials.read(&p.id)?.context("系统凭据不存在，请重新填写密码")?;
    let saved:StoredCredential=serde_json::from_str(&raw).context("系统凭据格式无效，请重新填写密码")?;
    ensure!(saved.url==p.url&&saved.username==p.username,"系统凭据与代理配置不匹配，请重新填写密码");Ok(saved.password)
   })();
   match result {Ok(value)=>{p.password=Some(value);p.credential_error=None;},Err(error)=>p.credential_error=Some(error.to_string())}
  }
 }

 fn persist(&self,items:&[Profile])->Result<()> {
  if let Some(path)=&self.path { let temporary=path.with_extension("tmp"); std::fs::write(&temporary,serde_json::to_vec_pretty(items)?)?;std::fs::rename(&temporary,path)?; } Ok(())
 }
}
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct ProfileView { pub id:String,pub name:String,pub url:String,pub username:String,pub auth_enabled:bool,pub needs_password:bool,pub remember_password:bool,pub credential_error:Option<String> }
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct ProfileInput { pub id:Option<String>,pub name:String,#[serde(default)] pub remember_password:bool,pub config:UpstreamInput }
impl UpstreamState {
    pub fn open(directory:&Path)->Result<Self> {
        let path=directory.join("upstream-profiles.json");
        let items=if path.exists(){serde_json::from_slice(&std::fs::read(&path)?).context("读取代理列表失败")?}else{vec![]};
        let mut book=Profiles{path:Some(path),items,..Default::default()};book.restore_passwords();
        Ok(Self{profiles:Mutex::new(book),..Default::default()})
    }
    pub fn list_profiles(&self)->Vec<ProfileView>{let mut book=self.profiles.lock().unwrap();book.restore_passwords();book.items.iter().map(|p|ProfileView{id:p.id.clone(),name:p.name.clone(),url:p.url.clone(),username:p.username.clone(),auth_enabled:p.auth_enabled,needs_password:p.auth_enabled&&p.password.is_none(),remember_password:p.remember_password,credential_error:p.credential_error.clone()}).collect()}
    pub fn save_profile(&self,mut input:ProfileInput)->Result<()> {
        let mut book=self.profiles.lock().unwrap();let name=input.name.trim().to_string();
        ensure!(!name.is_empty()&&name.chars().count()<=80,"代理名称需为 1–80 个字符");
        let existing=input.id.as_ref().and_then(|id|book.items.iter().find(|p|&p.id==id)).cloned();
        ensure!(input.id.is_none()||existing.is_some(),"代理配置不存在");
        ensure!(!book.items.iter().any(|p|p.name==name&&Some(&p.id)!=input.id.as_ref()),"代理名称已存在");
        ensure!(existing.is_some()||book.items.len()<100,"最多保存 100 个代理");
        let temp=Self::default(); *temp.listener.write().unwrap()=*self.listener.read().unwrap();
        if let Some(p)=&existing {if let Some(password)=&p.password {*temp.config.write().unwrap()=Some(UpstreamProxy{profile_id:None,profile_name:None,url:url::Url::parse(&p.url)?,username:p.username.clone(),password:password.clone(),listener:None});}}
        input.config.enabled=true;temp.update(input.config)?;let mut proxy=temp.snapshot().unwrap();
        let id=input.id.unwrap_or_else(||uuid::Uuid::new_v4().to_string());
        let profile=Profile{id:id.clone(),name:name.clone(),url:proxy.url.to_string(),username:proxy.username.clone(),auth_enabled:!proxy.username.is_empty(),remember_password:input.remember_password&&!proxy.username.is_empty(),credential_error:None,password:Some(proxy.password.clone())};
        let secret=if profile.remember_password {Some(serde_json::to_string(&StoredCredential{url:profile.url.clone(),username:profile.username.clone(),password:proxy.password.clone()})?)}else{None};
        let change=(profile.remember_password||existing.as_ref().is_some_and(|p|p.remember_password)).then_some((id.as_str(),secret.as_deref()));
        let mut next=book.items.clone();if let Some(i)=next.iter().position(|p|p.id==id){next[i]=profile;}else{next.push(profile);}
        book.commit(&next,change)?;book.items=next;
        let mut active=self.config.write().unwrap();if active.as_ref().is_some_and(|p|p.profile_id.as_ref()==Some(&id)){proxy.profile_id=Some(id);proxy.profile_name=Some(name);*active=Some(proxy);}
        Ok(())
    }
    fn resolve_profile(&self,book:&Profiles,id:&str)->Result<UpstreamProxy> {
        let p=book.items.iter().find(|p|p.id==id).context("代理配置不存在，请重新选择请求代理")?;
        ensure!(!p.auth_enabled||p.password.is_some(),"该代理需要重新填写密码，请编辑并保存");
        let temp=Self::default();*temp.listener.write().unwrap()=*self.listener.read().unwrap();
        temp.update(UpstreamInput{enabled:true,url:p.url.clone(),username:p.username.clone(),auth_enabled:p.auth_enabled,password:p.password.clone()})?;
        let mut proxy=temp.snapshot().unwrap();proxy.profile_id=Some(p.id.clone());proxy.profile_name=Some(p.name.clone());Ok(proxy)
    }
    /// Resolve a per-request snapshot without changing the global capture route.
    pub fn profile_snapshot(&self,id:&str)->Result<UpstreamProxy> {
        let book=self.profiles.lock().unwrap();self.resolve_profile(&book,id)
    }
    pub fn select_profile(&self,id:Option<String>)->Result<()> {
        let book=self.profiles.lock().unwrap();
        let proxy=id.as_deref().map(|id|self.resolve_profile(&book,id)).transpose()?;
        *self.config.write().unwrap()=proxy;Ok(())
    }
    pub fn delete_profile(&self,id:String)->Result<()> {
        let mut book=self.profiles.lock().unwrap();let active=self.config.read().unwrap();
        ensure!(!active.as_ref().is_some_and(|p|p.profile_id.as_ref()==Some(&id)),"请先切换代理或选择直连，再删除当前代理");
        let p=book.items.iter().find(|p|p.id==id).context("代理配置不存在")?;let change=p.remember_password.then_some((id.as_str(),None));let next:Vec<_>=book.items.iter().filter(|p|p.id!=id).cloned().collect();book.commit(&next,change)?;book.items=next;Ok(())
    }
    pub fn status(&self) -> UpstreamStatus {
        self.config.read().unwrap().as_ref().map(|p| UpstreamStatus { profile_id: p.profile_id.clone(), profile_name:p.profile_name.clone(), enabled: true, url: p.url.to_string(), username: p.username.clone(), auth_enabled: !p.username.is_empty() }).unwrap_or_default()
    }
    pub fn update(&self, input: UpstreamInput) -> Result<()> {
        if !input.enabled { *self.config.write().unwrap() = None; return Ok(()); }
        let url = url::Url::parse(input.url.trim()).context("请输入有效的上游代理 URL")?;
        ensure!(matches!(url.scheme(), "http" | "socks5") && url.host_str().is_some(), "支持 http:// 和 socks5:// 上游代理");
        ensure!(url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none() && (url.path().is_empty() || url.path() == "/"), "代理 URL 只能包含主机和端口；认证信息请单独填写");
        let mut current = self.config.write().unwrap();
        let password = if input.auth_enabled {
            ensure!(!input.username.is_empty() && !input.username.contains(':') && input.username.len() <= 255, "代理用户名不能为空、不能包含冒号且长度不超过 255 字节");
            match input.password {
                Some(value) => value,
                None => current.as_ref().filter(|p| p.url == url && p.username == input.username).map(|p| p.password.clone()).context("请输入代理密码")?,
            }
        } else { String::new() };
        ensure!(password.len() <= if url.scheme() == "socks5" { 255 } else { 1024 }, "代理密码过长");
        ensure!(!(input.auth_enabled && url.scheme() == "socks5" && password.is_empty()), "SOCKS5 认证密码不能为空");
        let listener = *self.listener.read().unwrap();
        if let Some(local) = listener {
            ensure!(!(url.port().unwrap_or(if url.scheme() == "socks5" { 1080 } else { 80 }) == local.port() && (hostname(&url).eq_ignore_ascii_case("localhost") || hostname(&url).parse::<std::net::IpAddr>().is_ok_and(|ip| crate::proxy::listener_ip_matches(ip, local)))), "上游代理不能指向当前抓包监听端口");
        }
        *current = Some(UpstreamProxy { profile_id:None, profile_name:None, url, username: if input.auth_enabled { input.username } else { String::new() }, password, listener });
        Ok(())
    }
    pub fn snapshot(&self) -> Option<UpstreamProxy> {
        self.config.read().unwrap().clone().map(|mut p| { p.listener = *self.listener.read().unwrap(); p })
    }
}
impl UpstreamProxy {
    pub(crate) async fn for_helper(&self)->Result<transport_api::UpstreamProxyConfig> {
        let addresses:Vec<_>=tokio::net::lookup_host((hostname(&self.url).as_str(),self.port())).await.context("无法解析上游代理")?.collect();
        ensure!(!addresses.is_empty(),"上游代理没有可用地址");
        if let Some(local)=self.listener {
            let local_ips=if_addrs::get_if_addrs()?.into_iter().map(|a|a.ip()).collect::<Vec<_>>();
            ensure!(!addresses.iter().any(|peer|peer.port()==local.port()&&(crate::proxy::listener_ip_matches(peer.ip(),local)||peer.ip().is_loopback()||local_ips.contains(&peer.ip()))),"上游代理解析到本机抓包监听端口，已阻止循环转发");
        }
        let mut url=self.url.clone();url.set_ip_host(addresses[0].ip()).map_err(|_|anyhow::anyhow!("无效代理地址"))?;
        Ok(transport_api::UpstreamProxyConfig{url:url.to_string(),username:self.username.clone(),password:self.password.clone()})
    }

    fn port(&self) -> u16 { self.url.port().unwrap_or(if self.url.scheme() == "socks5" { 1080 } else { 80 }) }
    fn authorization(&self) -> Option<String> {
        (!self.username.is_empty()).then(|| format!("Basic {}", STANDARD.encode(format!("{}:{}", self.username, self.password))))
    }
}
pub struct Connection { pub stream: Stream, pub absolute_form: bool, pub authorization: Option<String> }
pub async fn connect(target: &url::Url, proxy: Option<&UpstreamProxy>) -> Result<Connection> {
    let Some(proxy) = proxy else {
        return Ok(Connection { stream: Box::new(TcpStream::connect((hostname(target).as_str(), target.port_or_known_default().unwrap())).await?), absolute_form: false, authorization: None });
    };
    let mut tcp = TcpStream::connect((hostname(&proxy.url).as_str(), proxy.port())).await.context("无法连接上游代理")?;
    let peer = tcp.peer_addr()?;
    ensure!(!proxy.listener.is_some_and(|local| local.port() == peer.port() && (crate::proxy::listener_ip_matches(peer.ip(),local) || tcp.local_addr().is_ok_and(|a|a.ip().to_canonical()==peer.ip().to_canonical()))), "上游代理解析到本机抓包监听端口，已阻止循环转发");
    if proxy.url.scheme() == "socks5" {
        let method = if proxy.username.is_empty() { 0 } else { 2 };
        tcp.write_all(&[5, 1, method]).await?;
        let mut reply = [0; 2]; tcp.read_exact(&mut reply).await?;
        ensure!(reply == [5, method], "SOCKS5 上游不接受所选认证方式；未回退直连");
        if method == 2 {
            let mut auth = vec![1, proxy.username.len() as u8]; auth.extend_from_slice(proxy.username.as_bytes());
            auth.push(proxy.password.len() as u8); auth.extend_from_slice(proxy.password.as_bytes());
            tcp.write_all(&auth).await?; tcp.read_exact(&mut reply).await?;
            ensure!(reply == [1, 0], "SOCKS5 用户名/密码认证失败");
        }
        let mut request = vec![5, 1, 0];
        match target.host().unwrap() {
            url::Host::Ipv4(ip) => { request.push(1); request.extend_from_slice(&ip.octets()); }
            url::Host::Ipv6(ip) => { request.push(4); request.extend_from_slice(&ip.octets()); }
            url::Host::Domain(host) => {
                ensure!(host.len() <= 255, "SOCKS5 目标域名过长");
                request.extend_from_slice(&[3, host.len() as u8]); request.extend_from_slice(host.as_bytes());
            }
        }
        request.extend_from_slice(&target.port_or_known_default().unwrap().to_be_bytes());
        tcp.write_all(&request).await?;
        let mut response = [0; 4]; tcp.read_exact(&mut response).await?;
        ensure!(response[0] == 5 && response[2] == 0, "SOCKS5 响应格式错误");
        ensure!(response[1] == 0, "SOCKS5 连接目标失败，错误码 {}；未回退直连", response[1]);
        let count = match response[3] { 1 => 4, 4 => 16, 3 => tcp.read_u8().await? as usize, _ => anyhow::bail!("SOCKS5 返回未知地址类型") };
        let mut bound = vec![0; count + 2]; tcp.read_exact(&mut bound).await?;
        return Ok(Connection { stream: Box::new(tcp), absolute_form: false, authorization: None });
    }
    let authorization = proxy.authorization();
    if target.scheme() == "http" { return Ok(Connection { stream: Box::new(tcp), absolute_form: true, authorization }); }
    let host = if hostname(target).contains(':') { format!("[{}]", hostname(target)) } else { hostname(target) };
    let destination = format!("{host}:{}", target.port_or_known_default().unwrap());
    let mut head = format!("CONNECT {destination} HTTP/1.1\r\nHost: {destination}\r\n");
    if let Some(auth) = authorization { head.push_str(&format!("Proxy-Authorization: {auth}\r\n")); }
    head.push_str("\r\n"); tcp.write_all(head.as_bytes()).await?;
    let mut reader = BufReader::new(tcp);
    for _ in 0..8 {
        let raw = read_head(&mut reader).await.context("读取上游代理 CONNECT 响应失败")?;
        let (status, _, _) = parse_response(&raw).context("上游代理 CONNECT 响应无效")?;
        if (100..200).contains(&status) { continue; }
        ensure!((200..300).contains(&status), "上游代理 CONNECT 失败：HTTP {status}（407 表示认证失败）；未回退直连");
        return Ok(Connection { stream: Box::new(reader), absolute_form: false, authorization: None });
    }
    anyhow::bail!("上游代理 CONNECT 临时响应过多")
}

#[cfg(test)]
mod credential_tests {
 use super::*;
 #[derive(Default)]
 struct Vault(Mutex<std::collections::HashMap<String,String>>);
 impl CredentialStore for Vault {
  fn read(&self,id:&str)->Result<Option<String>>{Ok(self.0.lock().unwrap().get(id).cloned())}
  fn write(&self,id:&str,value:Option<&str>)->Result<()>{let mut data=self.0.lock().unwrap();if let Some(value)=value{data.insert(id.into(),value.into());}else{data.remove(id);}Ok(())}
 }
 fn input(id:Option<String>,remember:bool,password:Option<&str>)->ProfileInput {
  ProfileInput{id,name:"test".into(),remember_password:remember,config:UpstreamInput{enabled:true,url:"http://127.0.0.1:8888".into(),username:"tester".into(),auth_enabled:true,password:password.map(str::to_owned)}}
 }
 #[test]
 fn credentials_restore_remove_and_rollback_without_plaintext_on_disk(){
  let dir=tempfile::tempdir().unwrap();let path=dir.path().join("upstream-profiles.json");let vault=Arc::new(Vault::default());
  let state=UpstreamState{profiles:Mutex::new(Profiles{path:Some(path.clone()),items:vec![],credentials:vault.clone()}),..Default::default()};
  state.save_profile(input(None,true,Some("first-secret"))).unwrap();let id=state.list_profiles()[0].id.clone();
  assert!(!std::fs::read_to_string(&path).unwrap().contains("first-secret"));assert!(!serde_json::to_string(&state.list_profiles()).unwrap().contains("first-secret"));
  let reload=||UpstreamState{profiles:Mutex::new(Profiles{path:Some(path.clone()),items:serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap(),credentials:vault.clone()}),..Default::default()};
  let reopened=reload();assert!(!reopened.list_profiles()[0].needs_password);reopened.select_profile(Some(id.clone())).unwrap();assert_eq!(reopened.snapshot().unwrap().password,"first-secret");
  state.save_profile(input(Some(id.clone()),true,Some("updated-secret"))).unwrap();assert_eq!(reload().list_profiles()[0].remember_password,true);
  let old=vault.read(&id).unwrap();state.profiles.lock().unwrap().path=Some(dir.path().join("missing/config.json"));
  assert!(state.save_profile(input(Some(id.clone()),true,Some("failed-secret"))).is_err());assert_eq!(vault.read(&id).unwrap(),old);
  state.profiles.lock().unwrap().path=Some(path.clone());
  let mut mismatch=reload();mismatch.profiles.get_mut().unwrap().items[0].username="other".into();assert!(mismatch.list_profiles()[0].needs_password);
  state.save_profile(input(Some(id.clone()),false,None)).unwrap();assert!(vault.read(&id).unwrap().is_none());assert!(reload().list_profiles()[0].needs_password);assert!(!state.list_profiles()[0].needs_password);
  state.save_profile(input(Some(id.clone()),true,None)).unwrap();state.delete_profile(id.clone()).unwrap();assert!(vault.read(&id).unwrap().is_none());assert!(reload().list_profiles().is_empty());
 }
 #[test]
 fn unavailable_vault_does_not_publish_saved_profile(){
  struct Unavailable;
  impl CredentialStore for Unavailable {
   fn read(&self,_:&str)->Result<Option<String>>{anyhow::bail!("locked")}
   fn write(&self,_:&str,_:Option<&str>)->Result<()>{anyhow::bail!("locked")}
  }
  let state=UpstreamState{profiles:Mutex::new(Profiles{credentials:Arc::new(Unavailable),..Default::default()}),..Default::default()};
  assert!(state.save_profile(input(None,true,Some("secret"))).is_err());assert!(state.list_profiles().is_empty());
  state.save_profile(input(None,false,Some("secret"))).unwrap();
  {let mut book=state.profiles.lock().unwrap();book.items[0].remember_password=true;book.items[0].password=None;}
  let list=state.list_profiles();assert!(list[0].needs_password);assert!(list[0].credential_error.is_some());
 }
 #[test]
 #[ignore = "uses a unique temporary entry in the real OS credential store"]
 fn system_credential_lifecycle(){
  let id=format!("test-{}",uuid::Uuid::new_v4());let vault=SystemCredentials;
  struct Cleanup(String);impl Drop for Cleanup{fn drop(&mut self){let _=SystemCredentials.write(&self.0,None);}}
  let _cleanup=Cleanup(id.clone());
  vault.write(&id,Some("temporary-test-value")).unwrap();assert_eq!(vault.read(&id).unwrap().as_deref(),Some("temporary-test-value"));
  vault.write(&id,None).unwrap();assert!(vault.read(&id).unwrap().is_none());
 }
}

/// Test only CONNECT/SOCKS tunnel establishment; do not change the active route.
pub async fn test_profile(state:&UpstreamState,id:&str)->Result<u64>{
 let proxy=state.profile_snapshot(id)?;
 let start=std::time::Instant::now();
 tokio::time::timeout(std::time::Duration::from_secs(10),connect(&url::Url::parse("https://example.com/")?,Some(&proxy))).await.context("Proxy tunnel test timed out")??;
 Ok(start.elapsed().as_millis() as u64)
}
