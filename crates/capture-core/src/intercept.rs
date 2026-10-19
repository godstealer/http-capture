use crate::model::*;
use anyhow::{Result, ensure, bail};
use serde::{Serialize, Deserialize};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
use tokio::sync::oneshot;
use base64::Engine as _;
use sha2::{Digest, Sha256};
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Config { pub request: bool, pub response: bool, pub scope: String, #[serde(default)] pub rules: Vec<Rule> }
/// Conditions within a rule are AND; nonempty rules are OR. No rules means no interception.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Rule {
 #[serde(default)] pub disabled: bool,
 #[serde(default)] pub exclude: bool,
 #[serde(default)] pub group: String,
 #[serde(default)] pub url_regex: bool,
 #[serde(skip)] pub compiled_url: Option<regex::Regex>,
 #[serde(default)] pub url_contains: String,
 #[serde(default)] pub host: String,
 #[serde(default)] pub method: String,
 #[serde(default)] pub status: Option<u16>,
 #[serde(default)] pub header_name: String,
 #[serde(default)] pub header_contains: String,
}
impl Rule {
 fn nonempty(&self)->bool { !self.url_contains.is_empty() || !self.host.is_empty() || !self.method.is_empty() || self.status.is_some() || !self.header_name.is_empty() }
 pub(crate) fn matches(&self, f:&Flow)->bool {
  !self.disabled && self.nonempty() && (self.url_contains.is_empty() || if self.url_regex { self.compiled_url.as_ref().is_some_and(|r|r.is_match(&f.request.url)) } else { f.request.url.contains(&self.url_contains) })
   && (self.host.is_empty() || url::Url::parse(&f.request.url).ok().and_then(|u|u.host_str().map(str::to_owned)).is_some_and(|h|h.eq_ignore_ascii_case(&self.host)))
   && (self.method.is_empty() || f.request.method.eq_ignore_ascii_case(&self.method))
   && self.status.is_none_or(|status|f.response.as_ref().is_some_and(|r|r.status==status))
   && (self.header_name.is_empty() || f.request.headers.iter().any(|h|!h.disabled && h.name.eq_ignore_ascii_case(&self.header_name) && h.value.contains(&self.header_contains)))
 }
}
pub(crate) fn matches_rules(rules:&[Rule],flow:&Flow)->bool {
 !rules.iter().any(|r|r.exclude&&r.matches(flow)) && rules.iter().any(|r|!r.exclude&&r.matches(flow))
}
#[derive(Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct Item { pub id: String, pub stage: String, pub flow: Flow }
/// Small projection for automation: no headers, script state or body allocation.
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct PendingSummary {pub id:String,pub stage:String,pub request_id:String,pub source:String,pub method:String,pub url:String,pub status:Option<u16>,pub started_at:u64}
#[derive(Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Decision { pub id: String, pub action: String, pub request: Option<RequestDraft>, pub response: Option<CapturedResponse> }
#[derive(Serialize)]
pub struct Snapshot { pub config: Config, pub items: Vec<Item>, pub hits:Vec<u64> }
#[derive(Default)]
struct Inner { hits:Vec<u64>, config: Config, items: BTreeMap<String, (Item, oneshot::Sender<Decision>)> }
#[derive(Default)]
pub struct Interceptor { store:Option<std::sync::Arc<crate::store::Store>>, inner: Mutex<Inner> }
struct Guard<'a>(&'a Interceptor, String);
impl Drop for Guard<'_> { fn drop(&mut self) { self.0.inner.lock().unwrap().items.remove(&self.1); } }
impl Interceptor {
 pub fn open(store:std::sync::Arc<crate::store::Store>)->Result<Self>{
  let mut state=Self::default();
  let config=store.setting("interception.v1")?.unwrap_or(Config{scope:"all".into(),..Default::default()});
  state.configure(config)?;state.store=Some(store);Ok(state)
 }

 pub fn snapshot(&self) -> Snapshot { let s=self.inner.lock().unwrap(); Snapshot { hits:s.hits.clone(), config:s.config.clone(), items:s.items.values().map(|x|x.0.clone()).collect() } }
 pub fn pending_summaries(&self,offset:usize,limit:usize)->(usize,Vec<PendingSummary>){
  let state=self.inner.lock().unwrap();let items=state.items.values().skip(offset).take(limit.min(64)).map(|(item,_)|PendingSummary{id:item.id.clone(),stage:item.stage.clone(),request_id:item.flow.id.clone(),source:item.flow.source.chars().take(32).collect(),method:item.flow.request.method.chars().take(32).collect(),url:item.flow.request.url.chars().take(8192).collect(),status:item.flow.response.as_ref().map(|r|r.status),started_at:item.flow.started_at}).collect();(state.items.len(),items)
 }
 /// Clone only the immutable shared body handle for the requested pending item.
 pub fn pending_body(&self,id:&str,direction:&str)->Result<SharedBody>{
  let state=self.inner.lock().unwrap();let (item,_)=state.items.get(id).ok_or_else(||anyhow::anyhow!("Interception ended or timed out"))?;
  match direction {"request"=>Ok(item.flow.request.body_base64.clone()),"response"=>Ok(item.flow.response.as_ref().ok_or_else(||anyhow::anyhow!("No response is available at the request breakpoint"))?.body_base64.clone()),_=>bail!("direction must be request or response")}
 }
 /// Read the configuration and its content version under the same lock. Hit counters
 /// and pending traffic do not change this version.
 pub fn config_versioned(&self)->(Config,String) {
  let state=self.inner.lock().unwrap();(state.config.clone(),config_version(&state.config))
 }
 pub fn configure(&self, config:Config) -> Result<()> {self.configure_inner(config,None).map(|_|())}
 /// Compare and persist under the same lock, including writes from the desktop UI.
 pub fn configure_if_version(&self,config:Config,expected_version:&str)->Result<String>{self.configure_inner(config,Some(expected_version))}
 fn configure_inner(&self, mut config:Config,expected_version:Option<&str>) -> Result<String> {
  validate_and_compile(&mut config)?;
  let mut s=self.inner.lock().unwrap();
  if let Some(expected)=expected_version {ensure!(expected==config_version(&s.config),"Interception configuration changed; read get_interception again before applying changes");}
  if let Some(store)=&self.store{store.save_setting("interception.v1",&config)?;}s.hits=vec![0;config.rules.len()];s.config=config;
  // Disabling a breakpoint releases its waiting requests unchanged.
  let ids:Vec<_>=s.items.iter().filter(|(_, (i,_))| !enabled(&s.config,&i.stage,&i.flow.source) || !matches_rules(&s.config.rules,&i.flow)).map(|(id,_)|id.clone()).collect();
  for id in ids { if let Some((_,tx))=s.items.remove(&id) { let _=tx.send(Decision{id,action:"continue".into(),request:None,response:None}); } }
  Ok(config_version(&s.config))
 }
 pub fn resolve(&self, mut d:Decision)->Result<()> {
  let mut s=self.inner.lock().unwrap(); let (item,_) = s.items.get(&d.id).ok_or_else(||anyhow::anyhow!("拦截已结束或超时"))?;
  ensure!(["continue","modify","replace","abort"].contains(&d.action.as_str()),"无效操作");
  if d.action=="modify" && item.stage=="request" { let r=d.request.as_ref().ok_or_else(||anyhow::anyhow!("缺少请求"))?; crate::http1::prepare(r)?; }
  if d.action=="replace" || (d.action=="modify" && item.stage=="response") {
   let r=d.response.as_mut().ok_or_else(||anyhow::anyhow!("缺少响应"))?;
   r.retain_enabled_headers();
   ensure!((200..=599).contains(&r.status),"响应状态码必须在 200–599 之间");
   let body=base64::engine::general_purpose::STANDARD.decode(&r.body_base64)?; ensure!(body.len()<=8*1024*1024,"正文超过 8 MiB");
   ensure!(r.headers.len()<=512,"响应头过多");
   for h in &r.headers { let _:http::HeaderName=h.name.parse()?; let _:http::HeaderValue=h.value.parse()?; }
   r.headers.retain(|h| !["content-length","transfer-encoding","connection"].contains(&h.name.to_ascii_lowercase().as_str()));
   r.headers.push(Header { disabled: false,name:"Content-Length".into(),value:body.len().to_string()}); r.raw_head_base64=None;
   // TLS/protocol metadata comes from the actual connection, never the editor.
   if let Some(original)=&item.flow.response { r.version=original.version.clone(); r.upstream_tls=original.upstream_tls.clone(); r.tls_version=original.tls_version.clone(); r.sent_request_headers=original.sent_request_headers.clone(); }
   else { r.version="HTTP/1.1".into(); r.upstream_tls=None; r.tls_version=None; r.sent_request_headers=None; }
  }
  let (_,tx)=s.items.remove(&d.id).unwrap(); tx.send(d).map_err(|_|anyhow::anyhow!("连接已结束"))
 }
 pub async fn pause(&self, flow:&Flow, stage:&str)->Result<Option<Decision>> {
  let (tx,rx)=oneshot::channel(); let id=uuid::Uuid::new_v4().to_string();
  { let mut s=self.inner.lock().unwrap(); if !enabled(&s.config,stage,&flow.source) || !matches_rules(&s.config.rules,flow) { return Ok(None); }
    ensure!(s.items.len()<64,"拦截队列已满，请求已终止");
    let matched:Vec<usize>=s.config.rules.iter().enumerate().filter(|(_,r)|!r.exclude&&r.matches(flow)).map(|(i,_)|i).collect();
    for index in matched {s.hits[index]=s.hits[index].saturating_add(1);}
    s.items.insert(id.clone(),(Item{id:id.clone(),stage:stage.into(),flow:flow.clone()},tx)); }
  let _guard=Guard(self,id);
  let d=tokio::time::timeout(Duration::from_secs(120),rx).await.map_err(|_|anyhow::anyhow!("拦截等待超过 120 秒，请求已终止"))??;
  if d.action=="abort" { bail!("用户终止了请求"); } Ok(Some(d))
 }
}
/// Validate imported breakpoint conditions without persisting them or releasing
/// any waiting requests. The caller's regex cache is left unchanged.
pub fn validate_config(config:&Config)->Result<()>{let mut candidate=config.clone();validate_and_compile(&mut candidate)}
fn validate_and_compile(config:&mut Config)->Result<()>{
 ensure!(["all","capture","replay"].contains(&config.scope.as_str()), "无效的拦截范围");
 ensure!(config.rules.len()<=32,"最多支持 32 条规则");
 for r in &mut config.rules {
  ensure!(r.nonempty(),"规则至少需要一个条件");
  ensure!(r.group.len()<=128 && r.url_contains.len()<=2048 && r.host.len()<=253 && r.method.len()<=32 && r.header_name.len()<=256 && r.header_contains.len()<=2048,"规则条件过长");
  r.compiled_url=if r.url_regex&&!r.url_contains.is_empty(){Some(regex::RegexBuilder::new(&r.url_contains).size_limit(1024*1024).build().map_err(|e|anyhow::anyhow!("URL 正则表达式无效：{e}"))?)}else{None};
  ensure!(r.status.is_none_or(|v|(100..=599).contains(&v)),"无效状态码");
  ensure!(r.header_contains.is_empty()||!r.header_name.is_empty(),"请填写请求头名称");
 }
 Ok(())
}
fn config_version(config:&Config)->String {format!("{:x}",Sha256::digest(serde_json::to_vec(config).expect("serializable interception configuration")))}
fn enabled(c:&Config,stage:&str,source:&str)->bool { (c.scope=="all" || c.scope==source) && if stage=="request" {c.request} else {c.response} }

#[cfg(test)] mod rule_tests {
 use super::*;
 #[test] fn conditions_are_conjunctive_and_headers_handle_duplicates() {
 let f:Flow=serde_json::from_value(serde_json::json!({"id":"test","parentId":null,"startedAt":0,"durationMs":0,"source":"capture","request":{"method":"POST","url":"https://example.test/api?a=1","headers":[{"name":"X-Key","value":"first"},{"name":"x-key","value":"second"}],"bodyBase64":""},"response":{"status":403,"version":"HTTP/2","headers":[],"bodyBase64":"","rawHeadBase64":null},"error":null,"rawRequestHeadBase64":null,"notes":[]})).unwrap();
 let mut r=Rule{host:"EXAMPLE.TEST".into(),method:"post".into(),url_contains:"/api?".into(),status:Some(403),header_name:"X-KEY".into(),header_contains:"second".into(),..Default::default()};assert!(r.matches(&f));
 r.method="GET".into();assert!(!r.matches(&f));r.method="".into();r.host="ample.test".into();assert!(!r.matches(&f));r.host="example.test".into();r.status=Some(200);assert!(!r.matches(&f));assert!(!Rule::default().matches(&f));
 }
 #[tokio::test] async fn exclusion_priority_disable_and_reconfigure_release(){
  let interceptor=std::sync::Arc::new(Interceptor::default());
  let f:Flow=serde_json::from_value(serde_json::json!({"id":"test","parentId":null,"startedAt":0,"durationMs":0,"source":"capture","request":{"method":"GET","url":"https://example.test/api","headers":[],"bodyBase64":""},"response":null,"error":null,"notes":[]})).unwrap();
  let positive=Rule{host:"example.test".into(),..Default::default()};
  let excluded=Rule{exclude:true,url_contains:"/api".into(),..Default::default()};
  let mut config=Config{request:true,response:false,scope:"all".into(),rules:vec![positive,excluded]};
  interceptor.configure(config.clone()).unwrap();assert!(interceptor.pause(&f,"request").await.unwrap().is_none());assert_eq!(interceptor.snapshot().hits,vec![0,0]);
  config.rules[1].disabled=true;interceptor.configure(config.clone()).unwrap();
  let cloned=interceptor.clone();let task=tokio::spawn(async move{cloned.pause(&f,"request").await.unwrap()});
  tokio::time::timeout(Duration::from_secs(2),async{while interceptor.snapshot().items.is_empty(){tokio::task::yield_now().await}}).await.unwrap();
  assert_eq!(interceptor.snapshot().hits,vec![1,0]);
  config.rules[1].disabled=false;interceptor.configure(config).unwrap();
  assert_eq!(task.await.unwrap().unwrap().action,"continue");assert!(interceptor.snapshot().items.is_empty());
 }

 #[tokio::test] async fn import_validation_neither_reconfigures_nor_releases_waiting_requests(){
  let interceptor=std::sync::Arc::new(Interceptor::default());
  let flow:Flow=serde_json::from_value(serde_json::json!({"id":"validation","parentId":null,"startedAt":0,"durationMs":0,"source":"capture","request":{"method":"GET","url":"https://example.test/api","headers":[],"bodyBase64":""},"response":null,"error":null,"notes":[]})).unwrap();
  interceptor.configure(Config{request:true,response:false,scope:"all".into(),rules:vec![Rule{host:"example.test".into(),..Default::default()}]}).unwrap();
  let expected=interceptor.config_versioned().1;
  let cloned=interceptor.clone();let task=tokio::spawn(async move{cloned.pause(&flow,"request").await.unwrap()});
  tokio::time::timeout(Duration::from_secs(2),async{while interceptor.snapshot().items.is_empty(){tokio::task::yield_now().await}}).await.unwrap();
  // Applying this candidate would release the waiter; a preview must not.
  let mut candidate=Config{request:false,response:false,scope:"all".into(),rules:vec![Rule{url_regex:true,url_contains:"^https://other[.]test/".into(),..Default::default()}]};
  validate_config(&candidate).unwrap();assert!(candidate.rules[0].compiled_url.is_none());assert_eq!(interceptor.config_versioned().1,expected);
  candidate.rules[0].url_contains="[".into();assert!(validate_config(&candidate).is_err());assert_eq!(interceptor.snapshot().items.len(),1);assert!(!task.is_finished());
  let id=interceptor.snapshot().items[0].id.clone();interceptor.resolve(Decision{id,action:"continue".into(),request:None,response:None}).unwrap();assert_eq!(task.await.unwrap().unwrap().action,"continue");
 }

}
