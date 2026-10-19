use crate::model::*;
use anyhow::{Result, ensure, Context as _};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rquickjs::{Context, Runtime, Function, CatchResultExt};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::{Arc, Mutex}, time::{Instant, Duration}};
use sha2::{Digest, Sha256};
use md5::Md5;
use hmac::{Hmac, Mac};

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Scripts {
 #[serde(default)] pub modules: crate::script_tools::Modules,
 #[serde(default)] pub enabled: bool,
 #[serde(default)] pub before: String,
 #[serde(default)] pub after: String,
 #[serde(default)] pub variables: BTreeMap<String,String>,
}
impl Scripts {
 pub fn validate(&self)->Result<()> {
  ensure!(self.before.len()<=64*1024 && self.after.len()<=64*1024,"每段脚本最多 64 KiB");
  ensure!(serde_json::to_vec(&self.variables)?.len()<=64*1024,"脚本变量最多 64 KiB"); Ok(())
 }
}
/// Parse both hooks without invoking either hook or registering any HTTP helpers.
pub fn validate_scripts(scripts:&Scripts)->Result<()> {
 scripts.validate()?;
 let runtime=Runtime::new()?;runtime.set_memory_limit(64*1024*1024);runtime.set_max_stack_size(256*1024);
 let deadline=Instant::now()+Duration::from_secs(2);
 runtime.set_interrupt_handler(Some(Box::new(move||Instant::now()>deadline)));
 let context=Context::full(&runtime)?;
 context.with(|ctx|->Result<()>{
  for (stage,source) in [("before",&scripts.before),("after",&scripts.after)] {
   ctx.globals().set("__scriptSource",source.as_str())?;
   ctx.eval::<(),_>("new Function('\"use strict\";\\n' + __scriptSource); void 0;")
    .catch(&ctx).map_err(|error|anyhow::anyhow!("Global script {stage} syntax: {error}"))?;
  }
  Ok(())
 })
}
fn scripts_version(scripts:&Scripts)->String {format!("{:x}",Sha256::digest(serde_json::to_vec(scripts).expect("serializable scripts")))}
pub struct ScriptState { store:Option<Arc<crate::store::Store>>, capture: Mutex<(Scripts,u64)>, slots: Arc<tokio::sync::Semaphore> }
impl Default for ScriptState { fn default()->Self {Self{store:None,capture:Mutex::new((Scripts::default(),0)),slots:Arc::new(tokio::sync::Semaphore::new(4))}} }
impl ScriptState {
 pub fn open(store:Arc<crate::store::Store>)->Result<Self>{
  let scripts:Scripts=store.setting("capture-scripts.v1")?.unwrap_or_default();scripts.validate()?;
  Ok(Self{store:Some(store),capture:Mutex::new((scripts,0)),..Default::default()})
 }

 pub fn snapshot(&self)->(Scripts,u64){self.capture.lock().unwrap().clone()}
 pub fn snapshot_versioned(&self)->(Scripts,String){let state=self.capture.lock().unwrap();(state.0.clone(),scripts_version(&state.0))}
 pub fn configure(&self,s:Scripts)->Result<()> {s.validate()?;let mut current=self.capture.lock().unwrap();if let Some(store)=&self.store{store.save_setting("capture-scripts.v1",&s)?;}current.0=s;current.1+=1;Ok(())}
 /// Content comparison includes variable changes committed by running scripts.
 pub fn configure_if_version(&self,s:Scripts,expected_version:&str)->Result<String>{
  s.validate()?;let mut current=self.capture.lock().unwrap();
  ensure!(expected_version==scripts_version(&current.0),"Global scripts changed; read get_global_scripts again before applying changes");
  if let Some(store)=&self.store{store.save_setting("capture-scripts.v1",&s)?;}current.0=s;current.1+=1;Ok(scripts_version(&current.0))
 }
 pub fn commit_variables(&self,revision:u64,before:&BTreeMap<String,String>,after:&BTreeMap<String,String>)->Result<()> {
  let mut current=self.capture.lock().unwrap();if current.1!=revision{return Ok(());}
  let mut next=current.0.clone();
  for k in before.keys(){if !after.contains_key(k){next.variables.remove(k);}}
  for (k,v) in after {if before.get(k)!=Some(v){next.variables.insert(k.clone(),v.clone());}}
  next.validate()?;if let Some(store)=&self.store{store.save_setting("capture-scripts.v1",&next)?;}current.0=next;Ok(())
 }
 pub async fn execute(&self,code:String,flow:&Flow,variables:BTreeMap<String,String>,modules:crate::script_tools::Modules,after:bool)->Result<Output>{
  let permit=self.slots.clone().try_acquire_owned().context("脚本执行器繁忙，请稍后重试")?;
  let request=flow.request.clone();let response=flow.response.clone();
  let stopped=Arc::new(std::sync::atomic::AtomicBool::new(false));
  struct Stop(Arc<std::sync::atomic::AtomicBool>);
  impl Drop for Stop { fn drop(&mut self){self.0.store(true,std::sync::atomic::Ordering::Relaxed);} }
  let _stop=Stop(stopped.clone());
  tokio::task::spawn_blocking(move||{let _permit=permit;run(code,request,response,variables,modules,after,stopped)}).await?
 }
}
#[derive(Deserialize)]
pub struct Output { pub request:RequestDraft, pub response:Option<CapturedResponse>, pub variables:BTreeMap<String,String>, pub logs:Vec<String>, pub error:Option<String> }
// The old script input passed through serde_json::Value, which sorted every
// object's keys. JavaScript can observe that order through Object.keys and
// JSON.stringify. Keep small metadata in Value while borrowing the large body.
struct ScriptEntity<'a> {
 body: &'a SharedBody,
 metadata: serde_json::Map<String,serde_json::Value>,
}
impl<'a> From<&'a RequestDraft> for ScriptEntity<'a> {
 fn from(request:&'a RequestDraft)->Self {
  let RequestDraft{upstream_profile_id,scripts,pseudo_headers,engine,method,url,headers,body_base64,tls}=request;
  let serde_json::Value::Object(metadata)=serde_json::json!({
   "upstreamProfileId":upstream_profile_id,"scripts":scripts,"pseudoHeaders":pseudo_headers,
   "engine":engine,"method":method,"url":url,"headers":headers,"tls":tls,
  }) else {unreachable!()};
  Self{body:body_base64,metadata}
 }
}
impl<'a> From<&'a CapturedResponse> for ScriptEntity<'a> {
 fn from(response:&'a CapturedResponse)->Self {
  let CapturedResponse{trailers,upstream_tls,tls_version,sent_request_headers,status,version,headers,body_base64,raw_head_base64}=response;
  let serde_json::Value::Object(metadata)=serde_json::json!({
   "trailers":trailers,"upstreamTls":upstream_tls,"tlsVersion":tls_version,
   "sentRequestHeaders":sent_request_headers,"status":status,"version":version,
   "headers":headers,"rawHeadBase64":raw_head_base64,
  }) else {unreachable!()};
  Self{body:body_base64,metadata}
 }
}
impl Serialize for ScriptEntity<'_> {
 fn serialize<S:serde::Serializer>(&self,serializer:S)->std::result::Result<S::Ok,S::Error> {
  use serde::ser::SerializeMap;
  let mut map=serializer.serialize_map(Some(self.metadata.len()+1))?;
  let mut body_written=false;
  for (name,value) in &self.metadata {
   if !body_written && name.as_str()>"bodyBase64" {
    map.serialize_entry("bodyBase64",self.body)?;body_written=true;
   }
   map.serialize_entry(name,value)?;
  }
  if !body_written {map.serialize_entry("bodyBase64",self.body)?;}
  map.end()
 }
}
#[derive(Serialize)]
struct Input<'a> {
 request: ScriptEntity<'a>,
 response: Option<ScriptEntity<'a>>,
 variables: &'a BTreeMap<String,String>,
}
fn run(code:String,mut request:RequestDraft,mut response:Option<CapturedResponse>,variables:BTreeMap<String,String>,modules:crate::script_tools::Modules,after:bool,stopped:Arc<std::sync::atomic::AtomicBool>)->Result<Output>{
 request.retain_enabled_headers();
 if let Some(response)=&mut response { response.retain_enabled_headers(); }
 ensure!(code.len()<=64*1024,"脚本过长");
 let runtime=Runtime::new()?;runtime.set_memory_limit(64*1024*1024);runtime.set_max_stack_size(256*1024);
 let deadline=Instant::now()+Duration::from_secs(2);
 runtime.set_interrupt_handler(Some(Box::new(move||stopped.load(std::sync::atomic::Ordering::Relaxed)||Instant::now()>deadline)));
 let context=Context::full(&runtime)?;
 let input=serde_json::to_string(&Input{request:(&request).into(),response:response.as_ref().map(Into::into),variables:&variables})?;
 let output=context.with(|ctx|->Result<String>{
  ctx.globals().set("__input",input)?;
  ctx.globals().set("encodeText",Function::new(ctx.clone(),|s:String| STANDARD.encode(s.as_bytes()))?)?;
  ctx.globals().set("decodeText",Function::new(ctx.clone(),|s:String|->rquickjs::Result<String>{
   let b=STANDARD.decode(s).map_err(|e|rquickjs::Error::new_from_js_message("Base64","UTF-8",e.to_string()))?;
   String::from_utf8(b).map_err(|e|rquickjs::Error::new_from_js_message("bytes","UTF-8",e.to_string()))
  })?)?;
  ctx.globals().set("sha256",Function::new(ctx.clone(),|s:String| format!("{:x}",Sha256::digest(s.as_bytes())))?)?;
  ctx.globals().set("md5",Function::new(ctx.clone(),|s:String| format!("{:x}",Md5::digest(s.as_bytes())))?)?;
  ctx.globals().set("hmacSha256",Function::new(ctx.clone(),|key:String,text:String| {
   let mut mac=Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");mac.update(text.as_bytes());format!("{:x}",mac.finalize().into_bytes())
  })?)?;
  ctx.globals().set("hmacMd5",Function::new(ctx.clone(),|key:String,text:String| {
   let mut mac=Hmac::<Md5>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");mac.update(text.as_bytes());format!("{:x}",mac.finalize().into_bytes())
  })?)?;
  ctx.globals().set("__modulesJson",serde_json::to_string(&modules)?)?;
  ctx.globals().set("__tool",Function::new(ctx.clone(),move|namespace:String,method:String,args:String|->rquickjs::Result<String>{
   crate::script_tools::call(&modules,&namespace,&method,&args).map_err(|e|rquickjs::Error::new_from_js_message("tool arguments","result",e.to_string()))
  })?)?;
  ctx.eval::<(),_>(format!("const __modules=JSON.parse(__modulesJson);{}",crate::script_tools::BOOTSTRAP)).catch(&ctx).map_err(|e|anyhow::anyhow!("脚本环境初始化失败：{e}"))?;
  let source=format!(r#"(() => {{
   const data=JSON.parse(__input), request=data.request, response=data.response, variables=data.variables;
   const logs=[]; let error=null;
   const console=Object.freeze({{log:(...args)=>{{if(logs.length<100)logs.push(args.map(x=>typeof x==='string'?x:JSON.stringify(x)).join(' ').slice(0,2048));}}}});
   const assert=(condition,message='断言失败')=>{{if(!condition)throw new Error(message);}};
   try {{ const result=(function(){{ 'use strict';
{code}
   }}).call(undefined); if(result && typeof result.then==='function')throw new Error('仅支持同步脚本，不支持 Promise / async');
   }} catch(e) {{error=(String(e)+'\n'+String(e.stack||'')).slice(0,8192);}}
   return JSON.stringify({{request,response,variables,logs,error}});
  }})()"#);
  ctx.eval::<String,_>(source).catch(&ctx).map_err(|e|anyhow::anyhow!("脚本执行失败（限时 2 秒、内存 64 MiB）：{e}"))
 })?;
 ensure!(!runtime.is_job_pending(),"仅支持同步脚本，不支持异步任务");
 ensure!(output.len()<=24*1024*1024,"脚本结果过大");
 let mut result:Output=serde_json::from_str(&output).context("脚本输出格式无效：请保留 request / response 结构，变量值应为字符串")?;
 ensure!(serde_json::to_vec(&result.variables)?.len()<=64*1024,"脚本变量超过 64 KiB");
 if result.error.is_some(){return Ok(result);}
 if after {
  result.request=request;
  let original=response.context("响应脚本没有响应")?;
  let r=result.response.as_mut().context("不能删除响应")?;
  r.retain_enabled_headers();
  ensure!((200..=599).contains(&r.status),"响应状态码必须为 200–599");
  let bytes=STANDARD.decode(&r.body_base64).context("响应正文不是有效 Base64")?;
  ensure!(bytes.len()<=8*1024*1024,"响应正文超过 8 MiB");
  ensure!(r.headers.iter().map(|h|h.name.len()+h.value.len()+4).sum::<usize>()<=64*1024,"响应头超过 64 KiB");
  for h in &r.headers {let _:http::HeaderName=h.name.parse()?;let _:http::HeaderValue=h.value.parse()?;}
  let changed=r.body_base64!=original.body_base64;
  if !changed { r.body_base64=original.body_base64; }
  if changed {r.headers.retain(|h|!["content-encoding","content-length","transfer-encoding"].contains(&h.name.to_ascii_lowercase().as_str()));r.headers.push(Header { disabled: false,name:"Content-Length".into(),value:bytes.len().to_string()});}
  r.version=original.version;r.tls_version=original.tls_version;r.upstream_tls=original.upstream_tls;r.sent_request_headers=original.sent_request_headers;r.raw_head_base64=None;
 } else {
  if result.request.body_base64==request.body_base64 { result.request.body_base64=request.body_base64; }
  result.request.scripts=request.scripts;
  result.request.retain_enabled_headers();
  crate::http1::prepare(&result.request).context("请求脚本生成了无效请求")?;
  result.response=response;
 }
 Ok(result)
}

#[cfg(test)]
mod input_tests {
 use super::*;
 use serde_json::{json,Value};

 fn request()->RequestDraft {
  serde_json::from_value(json!({
   "upstreamProfileId":"fixture-profile","engine":"native","method":"POST",
   "url":"https://example.invalid/路径?q=%22","bodyBase64":"AP+AKg==",
   "pseudoHeaders":[{"name":":path","value":"/first"},{"name":":path","value":"/second"}],
   "headers":[{"name":"X-A","value":"first"},{"name":"Hidden","value":"not sent","disabled":true},{"name":"x-a","value":"last"}],
   "scripts":{"enabled":true,"before":"request.headers.push({name:'X-Test',value:'中文\\\\'});","after":"response.status=201;",
    "modules":{"utils":true,"encoding":true,"crypto":false},"variables":{"z":"last","a":"first\0"}},
   "tls":{"clientHelloHex":"1603010000","browserVersion":"155","version":"1.3","preset":"custom",
    "cipherList":"TLS_AES_128_GCM_SHA256","sigalgsList":"rsa_pss_rsae_sha256","curvesList":"X25519","grease":false,"permuteExtensions":true}
  })).unwrap()
 }

 fn response()->CapturedResponse {
  serde_json::from_value(json!({
   "status":200,"version":"HTTP/2","bodyBase64":"AP8iXA==","rawHeadBase64":"SFRUUA==","tlsVersion":"TLS 1.3",
   "headers":[{"name":"X-Z","value":"first"},{"name":"Hidden","value":"not sent","disabled":true},{"name":"X-Z","value":"last"}],
   "trailers":[{"name":"grpc-status","value":"0"}],
   "sentRequestHeaders":[{"name":"X-B","value":"first"},{"name":"X-A","value":"second"}],
   "upstreamTls":{"version":"TLS 1.3","cipherSuite":"TLS_AES_128_GCM_SHA256","alpn":"h2","serverName":"example.invalid","handshakeKind":"full",
    "offeredCipherSuites":["first","second"],"offeredAlpn":["h2","http/1.1"],"signatureSchemes":["rsa_pss_rsae_sha256"],"supportedGroups":["X25519"],
    "certificates":[{"subject":"fixture subject","issuer":"fixture issuer","serial":"01","notBefore":"start","notAfter":"end","sha256":"fixture hash",
     "dnsNames":["example.invalid"],"derBase64":"AP8=","parseError":null}]}
  })).unwrap()
 }

 #[test]
 fn borrowed_script_input_matches_legacy_json_bytes_with_nested_metadata_and_null_response() {
  let mut request=request();request.retain_enabled_headers();
  let mut response=response();response.retain_enabled_headers();
  let variables=BTreeMap::from([("z".into(),"last".into()),("a".into(),"中文\0".into())]);
  for response in [None,Some(&response)] {
   let input=Input{request:(&request).into(),response:response.map(Into::into),variables:&variables};
   // This full-model Value is only the legacy test oracle. Production excludes
   // bodyBase64 from metadata and serializes the shared string directly.
   let legacy=serde_json::to_string(&json!({"request":&request,"response":response,"variables":&variables})).unwrap();
   let actual=serde_json::to_string(&input).unwrap();
   assert_eq!(actual,legacy);
   assert!(std::ptr::eq(input.request.body,&request.body_base64));
   assert!(!input.request.metadata.contains_key("bodyBase64"));
   if let Some(entity)=&input.response {
    assert!(std::ptr::eq(entity.body,&response.unwrap().body_base64));
    assert!(!entity.metadata.contains_key("bodyBase64"));
   }
   let value:Value=serde_json::from_str(&actual).unwrap();
   assert_eq!(value["request"]["headers"],json!([{"name":"X-A","value":"first"},{"name":"x-a","value":"last"}]));
   assert_eq!(value["request"]["bodyBase64"],"AP+AKg==");
   assert_eq!(value["response"].is_null(),response.is_none());
  }
 }

 #[test]
 fn borrowed_script_input_matches_legacy_json_bytes_for_empty_entities_and_null_tls() {
  let request:RequestDraft=serde_json::from_value(json!({"method":"GET","url":"https://example.invalid/","headers":[],"bodyBase64":""})).unwrap();
  let response:CapturedResponse=serde_json::from_value(json!({"status":204,"version":"HTTP/1.1","headers":[],"bodyBase64":""})).unwrap();
  let variables=BTreeMap::new();
  let input=Input{request:(&request).into(),response:Some((&response).into()),variables:&variables};
  assert_eq!(serde_json::to_string(&input).unwrap(),serde_json::to_string(&json!({"request":&request,"response":&response,"variables":&variables})).unwrap());
 }

 #[test]
 fn body_is_merged_at_its_sorted_position_without_a_metadata_value_copy() {
  let body:SharedBody="A".repeat(256*1024).into();let pointer=body.as_ptr();
  for names in [vec![],vec!["aBefore"],vec!["zAfter"],vec!["aBefore","zAfter"]] {
   let metadata=names.into_iter().map(|name|(name.to_owned(),json!(name))).collect();
   let entity=ScriptEntity{body:&body,metadata};
   assert_eq!(entity.body.as_ptr(),pointer);
   assert!(!entity.metadata.contains_key("bodyBase64"));
   let mut legacy=entity.metadata.clone();legacy.insert("bodyBase64".into(),json!(body));
   assert_eq!(serde_json::to_string(&entity).unwrap(),serde_json::to_string(&legacy).unwrap());
   assert_eq!(body.as_ptr(),pointer);
  }
 }
}
