use capture_core::{model::*,transport::SendEngines};
use tokio::{io::{AsyncWriteExt,BufReader},net::TcpListener};
#[tokio::test]
async fn helper_roundtrip_preserves_ua_and_explicit_version(){
 let engines=SendEngines::with_roots(rustls::RootCertStore::empty());
 if !engines.list().iter().any(|e|e.id=="httpcloak"&&e.available){eprintln!("Build httpcloak helper to run integration test");return;}
 let l=TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=l.local_addr().unwrap();
 let server=tokio::spawn(async move{let(s,_)=l.accept().await.unwrap();let mut s=BufReader::new(s);let raw=capture_core::http1::read_head(&mut s).await.unwrap();let(_,_,headers)=capture_core::http1::parse_request(&raw).unwrap();s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();headers});
 let mut d:RequestDraft=serde_json::from_value(serde_json::json!({"engine":"httpcloak","method":"GET","url":format!("http://{addr}/"),"headers":[{"name":"User-Agent","value":"Chrome/152.0.0.0"}],"bodyBase64":"","tls":{"preset":"chrome","browserVersion":"150"}})).unwrap();
 let before=serde_json::to_value(&d).unwrap();let(r,notes)=engines.send(&d).await.unwrap();assert_eq!(r.status,200);assert!(notes.iter().any(|n|n.contains("chrome-150-windows")));assert_eq!(serde_json::to_value(&d).unwrap(),before);
 assert!(server.await.unwrap().iter().any(|h|h.name.eq_ignore_ascii_case("user-agent")&&h.value=="Chrome/152.0.0.0"));
 d.tls.browser_version=Some("999".into());assert!(format!("{:#}",engines.send(&d).await.unwrap_err()).contains("not available"));
}
async fn proxy_connect_auth_and_rejection_never_falls_back(engine_id:&str){
 use capture_core::{Engine,upstream::UpstreamInput};
 use base64::{Engine as _,engine::general_purpose::STANDARD};
 tokio::time::timeout(std::time::Duration::from_secs(8),async{
 let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
 if !engine.send_engines.list().iter().any(|e|e.id==engine_id&&e.available){return;}
 let proxy=TcpListener::bind("127.0.0.1:0").await.unwrap();
 engine.upstream.update(UpstreamInput{enabled:true,url:format!("http://{}",proxy.local_addr().unwrap()),username:"test-user".into(),auth_enabled:true,password:Some("test-secret".into())}).unwrap();
 let server=tokio::spawn(async move{let(s,_)=proxy.accept().await.unwrap();let mut s=BufReader::new(s);let raw=capture_core::http1::read_head(&mut s).await.unwrap();let(method,target,headers)=capture_core::http1::parse_request(&raw).unwrap();assert_eq!(method,"CONNECT");assert_eq!(target,"does-not-resolve.invalid:443");assert!(headers.iter().any(|h|h.name.eq_ignore_ascii_case("proxy-authorization")&&h.value==format!("Basic {}",STANDARD.encode("test-user:test-secret"))));s.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();});
 let d:RequestDraft=serde_json::from_value(serde_json::json!({"engine":engine_id,"method":"GET","url":"https://does-not-resolve.invalid/","headers":[],"bodyBase64":"","tls":{"preset":"chrome","browserVersion":"latest"}})).unwrap();
 let result=engine.execute(d,None,"capture",None).await.unwrap();assert!(result.error.is_some());let serialized=serde_json::to_string(&result).unwrap();assert!(!serialized.contains("test-secret"));assert!(!serialized.contains(&STANDARD.encode("test-user:test-secret")));server.await.unwrap();
 }).await.unwrap();
}

#[tokio::test]
async fn helper_proxy_rejection(){proxy_connect_auth_and_rejection_never_falls_back("httpcloak").await;}
#[cfg(feature="browser-replay")]
#[tokio::test]
async fn wreq_proxy_rejection(){proxy_connect_auth_and_rejection_never_falls_back("wreq").await;}
