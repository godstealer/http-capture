use capture_core::{Engine, model::*, transport::*};
use std::{sync::{Arc,Mutex}};
use base64::{Engine as _,engine::general_purpose::STANDARD};
struct Probe(Arc<Mutex<Vec<RequestDraft>>>);
impl EngineContract<RequestDraft,capture_core::upstream::UpstreamProxy> for Probe {
 fn id(&self)->&'static str{"probe"} fn profiles(&self)->Vec<String>{vec!["native".into()]}
 fn send<'a>(&'a self,r:&'a RequestDraft)->SendFuture<'a>{Box::pin(async move{self.0.lock().unwrap().push(r.clone());Ok((CapturedResponse { trailers: vec![], status:200,version:"HTTP/2".into(),headers:vec![Header { disabled: false,name:"Content-Encoding".into(),value:"br".into()}],body_base64:STANDARD.encode(b"original").into(),raw_head_base64:None,upstream_tls:None,tls_version:None,sent_request_headers:Some(r.headers.clone())},vec![]))})}
}
fn draft()->RequestDraft{serde_json::from_str(r#"{"method":"GET","url":"http://example.test/","engine":"probe","headers":[{"name":"X-A","value":"1"},{"name":"X-B","value":"2"},{"name":"X-A","value":"3"}],"bodyBase64":""}"#).unwrap()}
fn engine()->(tempfile::TempDir,Arc<Engine>,Arc<Mutex<Vec<RequestDraft>>>) {
 let dir=tempfile::tempdir().unwrap();let calls=Arc::new(Mutex::new(vec![]));let mut engines=SendEngines::empty();engines.register(Arc::new(Probe(calls.clone()))).unwrap();let engine=Engine::open_with_engines(dir.path(),rustls::RootCertStore::empty(),engines).unwrap();(dir,engine,calls)
}

fn plugin(id:&str,before:&str,after:&str)->capture_core::plugins::Plugin{serde_json::from_value(serde_json::json!({"id":id,"name":id,"version":"1","apiVersion":1,"enabled":true,"scope":"all","urlPattern":"^http://example\\.test/","before":before,"after":after})).unwrap()}
#[tokio::test]
async fn plugins_modify_both_sources_in_order_and_fail_transactionally(){
 let (_dir,e,calls)=engine();
 e.plugins.save(vec![plugin("first","assert(typeof fetch==='undefined' && typeof require==='undefined');request.headers.push({name:'X-Plugin',value:'one'});","response.status=201;"),plugin("broken","request.method='DELETE';throw Error('rollback');",""),plugin("last","assert(request.method==='GET');request.headers.push({name:'X-Plugin',value:'two'});", "response.headers.push({name:'X-After',value:'yes'});")]).unwrap();
 for source in ["capture","replay"]{let f=e.execute(draft(),None,source,None).await.unwrap();assert!(f.error.is_none(),"{:?}",f.error);let r=f.response.unwrap();assert_eq!(r.status,201);assert!(r.headers.iter().any(|h|h.name=="X-After"));assert_eq!(f.original_request.unwrap().headers.len(),3);assert_eq!(f.original_response.unwrap().status,200);}
 assert_eq!(calls.lock().unwrap()[0].headers.iter().filter(|h|h.name=="X-Plugin").map(|h|h.value.as_str()).collect::<Vec<_>>(),vec!["one","two"]);assert!(e.plugins.logs().iter().any(|l|l.contains("rollback")));
 let mut other=draft();other.url="http://other.test/".into();e.execute(other,None,"replay",None).await.unwrap();assert_eq!(calls.lock().unwrap()[2].headers.len(),3);
 let mut disabled=e.plugins.list();for p in &mut disabled{p.enabled=false}e.plugins.save(disabled).unwrap();e.execute(draft(),None,"replay",None).await.unwrap();assert_eq!(calls.lock().unwrap()[3].headers.len(),3);
}
#[test]
fn plugin_validation_and_persistence(){let (dir,e,_)=engine();let p=plugin("persist","","console.log('ok');");e.plugins.save(vec![p.clone()]).unwrap();assert!(e.plugins.save(vec![p.clone(),p.clone()]).is_err());let mut bad=p;bad.api_version=2;assert!(e.plugins.save(vec![bad]).is_err());drop(e);let reopened=Engine::open(dir.path()).unwrap();assert_eq!(reopened.plugins.list()[0].id,"persist");reopened.plugins.save(vec![]).unwrap();assert!(reopened.plugins.list().is_empty());}
