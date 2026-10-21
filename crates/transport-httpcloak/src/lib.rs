//! One isolated helper per request. Cancellation drops/kills the child, with no local TCP listener.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::{Path, PathBuf}, process::Stdio};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, process::Command};
use transport_api::{CapturedResponse, Header, RequestDraft, SharedBody, TlsProfile, UpstreamProxyConfig};

/// Serialize only enabled fields without building a second header collection.
struct EnabledHeaders<'a>(&'a [Header]);
impl Serialize for EnabledHeaders<'_> {
 fn serialize<S:serde::Serializer>(&self,serializer:S)->std::result::Result<S::Ok,S::Error>{
  serializer.collect_seq(self.0.iter().filter(|header|!header.disabled))
 }
}

/// The helper wire format borrows the original body and script configuration.
/// It intentionally mirrors every RequestDraft field, including explicit nulls.
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct RequestInput<'a,S> {
 upstream_profile_id:&'a Option<String>,
 scripts:&'a S,
 pseudo_headers:EnabledHeaders<'a>,
 engine:&'a str,
 method:&'a str,
 url:&'a str,
 headers:EnabledHeaders<'a>,
 body_base64:&'a SharedBody,
 tls:&'a TlsProfile,
}
impl<'a,S> From<&'a RequestDraft<S>> for RequestInput<'a,S> {
 fn from(request:&'a RequestDraft<S>)->Self{
  // Exhaustive destructuring makes new draft fields require an explicit wire
  // decision rather than silently dropping them from this projection.
  let RequestDraft{upstream_profile_id,scripts,pseudo_headers,engine,method,url,headers,body_base64,tls}=request;
  Self{upstream_profile_id,scripts,pseudo_headers:EnabledHeaders(pseudo_headers),engine,method,url,
   headers:EnabledHeaders(headers),body_base64,tls}
 }
}

#[derive(Serialize)]
struct Input<'a,S> {
 request:RequestInput<'a,S>,
 preset:&'a str,
 proxy:Option<&'a UpstreamProxyConfig>,
 #[serde(skip_serializing_if="Option::is_none")]
 security:Option<&'a transport_api::TlsMaterial>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all="camelCase")]
pub struct Capabilities { #[serde(default)] pub custom_client_hello: bool,#[serde(default)]pub certificate_roots:bool, pub protocol_version: u32, pub browser_versions: BTreeMap<String,Vec<u16>> }
#[derive(Deserialize)]
pub struct Reply { pub response: Option<CapturedResponse>, #[serde(default)] pub notes: Vec<String>, pub error: Option<String> }
#[derive(Clone)]
pub struct Helper { pub path: PathBuf, pub capabilities: Capabilities }
impl Helper {
 pub fn load(path: impl AsRef<Path>)->Result<Self>{
  let path=path.as_ref().to_path_buf();ensure!(path.is_file(),"httpcloak helper not built");
  let capabilities:Capabilities=serde_json::from_slice(&std::fs::read(path.with_extension("json"))?)?;
  ensure!(capabilities.protocol_version==1,"Unsupported httpcloak helper protocol");
  Ok(Self{path,capabilities})
 }
 pub async fn send<S:Serialize>(&self,request:&RequestDraft<S>,preset:&str,proxy:Option<&transport_api::UpstreamProxyConfig>)->Result<(CapturedResponse,Vec<String>)>{
  self.send_with_security(request,preset,proxy,None).await
 }
 pub async fn send_with_security<S:Serialize>(&self,request:&RequestDraft<S>,preset:&str,proxy:Option<&transport_api::UpstreamProxyConfig>,security:Option<&transport_api::TlsMaterial>)->Result<(CapturedResponse,Vec<String>)>{
  ensure!(request.tls.client_hello_hex.is_none() || self.capabilities.custom_client_hello,"This httpcloak helper does not support ClientHello Hex; rebuild the helper");
  if let Some(security)=security{
   ensure!(security.client_certificate_pem.is_none()&&security.client_private_key_pem.is_none(),"httpcloak 当前不支持客户端证书，请为此目标选择 Native/H2/H3 或 wreq；未忽略证书配置");
   ensure!(security.trusted_ca_pem.is_empty()||self.capabilities.certificate_roots,"此 httpcloak helper 不支持自定义信任 CA，请重新构建 helper");
  }
  let input=serde_json::to_vec(&Input{request:request.into(),preset,proxy,security})?;
  let mut cmd=Command::new(&self.path);
  cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
  #[cfg(windows)] cmd.creation_flags(0x08000000);
  let mut child=cmd.spawn().context("Starting httpcloak helper")?;
  let mut stdin=child.stdin.take().context("Helper stdin unavailable")?;
  let mut stdout=child.stdout.take().context("Helper stdout unavailable")?;
  let write=async { stdin.write_all(&input).await?;stdin.shutdown().await?;drop(stdin);Ok::<_,std::io::Error>(()) };
  let read=async {let mut bytes=Vec::new();stdout.read_to_end(&mut bytes).await?;Ok::<_,std::io::Error>(bytes)};
  let (_,bytes)=tokio::try_join!(write,read)?;
  let status=child.wait().await?;
  let reply:Reply=serde_json::from_slice(&bytes).context("Invalid helper response")?;
  if let Some(error)=reply.error{anyhow::bail!("{error}");}
  ensure!(status.success(),"httpcloak helper exited unsuccessfully");
  Ok((reply.response.context("Missing helper response")?,reply.notes))
 }
}

#[cfg(test)]
mod tests {
 use super::*;
 use serde_json::{json,Value};

 #[tokio::test]
 async fn unsupported_certificate_material_is_rejected_before_spawning_helper(){
  let helper=Helper{path:PathBuf::from("does-not-exist"),capabilities:serde_json::from_value(json!({"customClientHello":true,"protocolVersion":1,"browserVersions":{}})).unwrap()};
  let request=draft();let security=transport_api::TlsMaterial{trusted_ca_pem:vec!["certificate".into()],..Default::default()};
  let error=helper.send_with_security(&request,"chrome-155-windows",None,Some(&security)).await.err().unwrap().to_string();assert!(error.contains("不支持自定义信任 CA"));
  let security=transport_api::TlsMaterial{client_private_key_pem:Some("secret fixture".into()),..Default::default()};
  let error=helper.send_with_security(&request,"chrome-155-windows",None,Some(&security)).await.err().unwrap().to_string();assert!(error.contains("不支持客户端证书"));assert!(!error.contains("secret fixture"));
 }

 // Deliberately has no Clone implementation: send must accept Serialize alone.
 #[derive(Serialize)]
 struct NonCloneScripts { enabled:bool, source:String, variables:BTreeMap<String,String> }

 fn header(name:&str,value:&str,disabled:bool)->Header{
  Header{disabled,name:name.into(),value:value.into()}
 }

 fn draft()->RequestDraft<NonCloneScripts>{
  RequestDraft{
   upstream_profile_id:Some("business-proxy".into()),
   scripts:NonCloneScripts{enabled:true,source:"request.headers.push({name: 'X-Test', value: '\"\\\\'})".into(),
    variables:BTreeMap::from([("key".into(),"中文\0fixture".into())])},
   pseudo_headers:vec![header(":method","POST",false),header(":authority","disabled.invalid",true),
    header(":path","/first",false),header(":path","/second",false)],
   engine:"httpcloak".into(),method:"POST".into(),url:"https://example.invalid/路径?q=%22".into(),
   headers:vec![header("X-A","first",false),header("X-Disabled","never sent",true),
    header("x-a","",false),header("X-A","last",false),header("Cookie","a=1",false),header("cookie","b=2",false)],
   // Includes binary 00/ff and an invalid UTF-8 sequence, kept as exact Base64.
   body_base64:"AP8iwCgNXA==".into(),
   tls:TlsProfile{client_hello_hex:Some("1603010000".into()),browser_version:Some("155".into()),version:Some("1.3".into()),
    preset:"custom".into(),cipher_list:Some("TLS_AES_128_GCM_SHA256".into()),sigalgs_list:Some("rsa_pss_rsae_sha256".into()),
    curves_list:Some("X25519".into()),grease:Some(false),permute_extensions:Some(true)},
  }
 }

 #[test]
 fn helper_input_preserves_the_complete_wire_contract_with_non_clone_scripts(){
  let request=draft();
  let proxy=UpstreamProxyConfig{url:"http://127.0.0.1:7897/".into(),username:"fixture-user".into(),password:"disposable-fixture-password".into()};
  let bytes=serde_json::to_vec(&Input{request:(&request).into(),preset:"chrome-155-windows",proxy:Some(&proxy),security:None}).unwrap();
  let wire:Value=serde_json::from_slice(&bytes).unwrap();
  assert_eq!(wire,json!({
   "request":{
    "upstreamProfileId":"business-proxy",
    "scripts":{"enabled":true,"source":request.scripts.source,"variables":{"key":"中文\0fixture"}},
    "pseudoHeaders":[{"name":":method","value":"POST"},{"name":":path","value":"/first"},{"name":":path","value":"/second"}],
    "engine":"httpcloak","method":"POST","url":"https://example.invalid/路径?q=%22",
    "headers":[{"name":"X-A","value":"first"},{"name":"x-a","value":""},{"name":"X-A","value":"last"},
     {"name":"Cookie","value":"a=1"},{"name":"cookie","value":"b=2"}],
    "bodyBase64":"AP8iwCgNXA==",
    "tls":{"clientHelloHex":"1603010000","browserVersion":"155","version":"1.3","preset":"custom",
     "cipherList":"TLS_AES_128_GCM_SHA256","sigalgsList":"rsa_pss_rsae_sha256","curvesList":"X25519","grease":false,"permuteExtensions":true}
   },
   "preset":"chrome-155-windows",
   "proxy":{"url":"http://127.0.0.1:7897/","username":"fixture-user","password":"disposable-fixture-password"}
  }));
  assert_eq!(request.headers.len(),6);
  assert!(request.headers[1].disabled);
  assert_eq!(request.pseudo_headers.len(),4);
  assert!(request.pseudo_headers[1].disabled);
  assert_eq!(request.body_base64.as_str(),"AP8iwCgNXA==");
  assert_eq!(proxy.password,"disposable-fixture-password");
 }

 #[test]
 fn helper_input_keeps_explicit_null_options_and_empty_body_and_header_arrays(){
  let mut request=draft();
  request.upstream_profile_id=None;
  request.headers=vec![header("Hidden","hidden",true)];
  request.pseudo_headers.clear();
  request.body_base64=SharedBody::default();
  request.tls=TlsProfile::default();
  let wire:Value=serde_json::from_slice(&serde_json::to_vec(&Input{request:(&request).into(),preset:"chrome-155-windows",proxy:None,security:None}).unwrap()).unwrap();
  assert_eq!(wire["proxy"],Value::Null);
  assert_eq!(wire["request"]["upstreamProfileId"],Value::Null);
  assert_eq!(wire["request"]["headers"],json!([]));
  assert_eq!(wire["request"]["pseudoHeaders"],json!([]));
  assert_eq!(wire["request"]["bodyBase64"],json!(""));
  assert_eq!(wire["request"]["tls"],json!({"clientHelloHex":null,"browserVersion":null,"version":null,"preset":"native",
   "cipherList":null,"sigalgsList":null,"curvesList":null,"grease":null,"permuteExtensions":null}));
  assert_eq!(wire["request"].as_object().unwrap().len(),9);
 }

 #[test]
 fn helper_projection_borrows_large_body_and_scripts_until_the_single_serialization(){
  let mut request=draft();
  request.body_base64="A".repeat(2*1024*1024).into();
  let pointer=request.body_base64.as_ptr();
  let input=Input{request:(&request).into(),preset:"chrome-155-windows",proxy:None,security:None};
  assert!(std::ptr::eq(input.request.body_base64,&request.body_base64));
  assert!(std::ptr::eq(input.request.scripts,&request.scripts));
  let bytes=serde_json::to_vec(&input).unwrap();
  assert_eq!(request.body_base64.as_ptr(),pointer);
  assert_eq!(request.body_base64.len(),2*1024*1024);
  let wire:Value=serde_json::from_slice(&bytes).unwrap();
  assert_eq!(wire["request"]["bodyBase64"].as_str().unwrap().len(),2*1024*1024);
  assert!(wire["request"]["bodyBase64"].as_str().unwrap().bytes().all(|byte|byte==b'A'));
 }
}
