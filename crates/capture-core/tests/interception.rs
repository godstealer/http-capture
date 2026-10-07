use capture_core::{Engine, model::*, intercept::*, transport::*};
use std::sync::{Arc,Mutex};
struct Probe(Arc<Mutex<Vec<RequestDraft>>>);
impl EngineContract<RequestDraft,capture_core::upstream::UpstreamProxy> for Probe {
 fn id(&self)-> &'static str {"probe"} fn profiles(&self)->Vec<String>{vec!["native".into()]}
 fn send<'a>(&'a self,r:&'a RequestDraft)->SendFuture<'a>{Box::pin(async move {self.0.lock().unwrap().push(r.clone());Ok((CapturedResponse{status:200,version:"HTTP/1.1".into(),headers:vec![],body_base64:"b2s=".into(),raw_head_base64:None,upstream_tls:None,tls_version:None,sent_request_headers:None},vec![]))})}
}
fn draft()->RequestDraft {serde_json::from_str(r#"{"engine":"probe","method":"GET","url":"http://example.test/","headers":[],"bodyBase64":""}"#).unwrap()}
async fn waiting(e:&Engine)->Item {tokio::time::timeout(std::time::Duration::from_secs(3),async {loop {if let Some(i)=e.intercept.snapshot().items.first(){return i.clone();} tokio::task::yield_now().await;}}).await.unwrap()}
fn decision(i:&Item,action:&str)->Decision {Decision{id:i.id.clone(),action:action.into(),request:None,response:None}}
#[tokio::test]
async fn shared_pipeline_edits_both_stages_and_retains_originals(){
 let temp=tempfile::tempdir().unwrap();let calls=Arc::new(Mutex::new(vec![]));let mut send=SendEngines::empty();send.register(Arc::new(Probe(calls.clone()))).unwrap();let e=Engine::open_with_engines(temp.path(),rustls::RootCertStore::empty(),send).unwrap();
 e.intercept.configure(Config{request:true,response:true,scope:"all".into(),rules:vec![Rule{url_contains:"example.test".into(),..Default::default()}]}).unwrap();
 for source in ["capture","replay"] {
 let other=e.clone();let task=tokio::spawn(async move{other.execute(draft(),None,source,None).await.unwrap()});
 let i=waiting(&e).await;assert_eq!(i.stage,"request");let mut d=decision(&i,"modify");let mut r=draft();r.method="POST".into();r.headers=vec![Header{name:"X-A".into(),value:"1".into()},Header{name:"X-B".into(),value:"2".into()},Header{name:"X-A".into(),value:"3".into()}];d.request=Some(r);e.intercept.resolve(d).unwrap();
 let i=waiting(&e).await;assert_eq!(i.stage,"response");let mut d=decision(&i,"modify");let mut r=i.flow.response.clone().unwrap();r.status=201;r.body_base64="ZWRpdGVk".into();d.response=Some(r);e.intercept.resolve(d).unwrap();let f=task.await.unwrap();assert_eq!(f.original_request.unwrap().method,"GET");assert_eq!(f.request.method,"POST");assert_eq!(f.original_response.unwrap().status,200);assert_eq!(f.response.unwrap().status,201);
 }
 assert_eq!(calls.lock().unwrap().len(),2);assert_eq!(calls.lock().unwrap()[0].headers[2].value,"3");assert!(e.intercept.snapshot().items.is_empty());
}
#[tokio::test]
async fn replacement_abort_disable_and_cancel_cleanup(){
 let temp=tempfile::tempdir().unwrap();let e=Engine::open(temp.path()).unwrap();
 e.intercept.configure(Config{request:true,response:false,scope:"all".into(),rules:vec![Rule{url_contains:"example.test".into(),..Default::default()}]}).unwrap();
 let other=e.clone();let task=tokio::spawn(async move{other.execute(draft(),None,"capture",None).await.unwrap()});let i=waiting(&e).await;let mut d=decision(&i,"replace");d.response=Some(CapturedResponse{status:202,version:"fake".into(),headers:vec![],body_base64:"".into(),raw_head_base64:None,upstream_tls:None,tls_version:None,sent_request_headers:None});e.intercept.resolve(d).unwrap();assert_eq!(task.await.unwrap().response.unwrap().status,202);
 let other=e.clone();let task=tokio::spawn(async move{other.execute(draft(),None,"replay",None).await.unwrap()});let i=waiting(&e).await;e.intercept.resolve(decision(&i,"abort")).unwrap();assert!(task.await.unwrap().error.unwrap().contains("终止"));
 let other=e.clone();let task=tokio::spawn(async move{other.execute(draft(),None,"replay",None).await.unwrap()});waiting(&e).await;task.abort();let _=task.await;assert!(e.intercept.snapshot().items.is_empty());
 let other=e.clone();let task=tokio::spawn(async move{other.execute(draft(),None,"replay",None).await.unwrap()});waiting(&e).await;e.intercept.configure(Config{request:false,response:false,scope:"all".into(),rules:vec![Rule{url_contains:"example.test".into(),..Default::default()}]}).unwrap();assert!(task.await.unwrap().error.is_some());assert!(e.intercept.snapshot().items.is_empty());
}

#[tokio::test]
async fn unmatched_and_empty_rules_never_pause_and_rule_updates_release_waiters(){
 let temp=tempfile::tempdir().unwrap();let e=Engine::open(temp.path()).unwrap();
 for rules in [vec![],vec![Rule{host:"other.test".into(),..Default::default()}],vec![Rule{status:Some(200),..Default::default()}]] {
 e.intercept.configure(Config{request:true,response:true,scope:"all".into(),rules}).unwrap();
 // The unregistered engine fails immediately; a breakpoint would stall this future.
 let f=tokio::time::timeout(std::time::Duration::from_secs(1),e.execute(draft(),None,"replay",None)).await.unwrap().unwrap();assert!(f.error.is_some());assert!(e.intercept.snapshot().items.is_empty());
 }
 e.intercept.configure(Config{request:true,response:false,scope:"all".into(),rules:vec![Rule{host:"EXAMPLE.TEST".into(),method:"get".into(),url_contains:"http://".into(),..Default::default()}]}).unwrap();
 let other=e.clone();let task=tokio::spawn(async move{other.execute(draft(),None,"replay",None).await.unwrap()});waiting(&e).await;
 e.intercept.configure(Config{request:true,response:false,scope:"all".into(),rules:vec![]}).unwrap();assert!(task.await.unwrap().error.is_some());assert!(e.intercept.snapshot().items.is_empty());
 assert!(e.intercept.configure(Config{request:true,response:true,scope:"all".into(),rules:vec![Rule::default()]}).is_err());
}

#[tokio::test]
async fn regex_rules_validate_before_apply_and_match_urls() {
 let temp=tempfile::tempdir().unwrap();let e=Engine::open(temp.path()).unwrap();
 let config=Config{request:true,response:false,scope:"all".into(),rules:vec![Rule{url_regex:true,url_contains:r"(?i)^http://EXAMPLE\.test/$".into(),..Default::default()}]};
 e.intercept.configure(config.clone()).unwrap();
 let mut invalid=config.clone();invalid.rules[0].url_contains="[".into();assert!(e.intercept.configure(invalid).is_err());assert_eq!(e.intercept.snapshot().config.rules[0].url_contains,config.rules[0].url_contains);
 let other=e.clone();let task=tokio::spawn(async move{other.execute(draft(),None,"replay",None).await.unwrap()});let item=waiting(&e).await;e.intercept.resolve(decision(&item,"abort")).unwrap();assert!(task.await.unwrap().error.is_some());
 let mut unmatched=draft();unmatched.url="http://exampleXtest/".into();assert!(tokio::time::timeout(std::time::Duration::from_secs(1),e.execute(unmatched,None,"replay",None)).await.unwrap().unwrap().error.is_some());
}
