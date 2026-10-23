use capture_core::{model::*,transport::SendEngines};
use tokio::{io::{AsyncReadExt,AsyncWriteExt,BufReader},net::TcpListener};
#[tokio::test]
async fn helper_sends_large_body_with_normalized_length_and_exact_bytes(){
 let engines=SendEngines::with_roots(rustls::RootCertStore::empty());
 if !engines.list().iter().any(|e|e.id=="httpcloak"&&e.available){eprintln!("Build httpcloak helper to run integration test");return;}
 let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
 let body:Vec<u8>=(0..256*1024+7).map(|index|(index%251) as u8).collect();
 let expected=body.clone();
 let server=tokio::spawn(async move{
  let(socket,_)=listener.accept().await.unwrap();let mut socket=BufReader::new(socket);
  let raw=capture_core::http1::read_head(&mut socket).await.unwrap();
  let (method,_,headers)=capture_core::http1::parse_request(&raw).unwrap();
  let length=headers.iter().find(|header|header.name.eq_ignore_ascii_case("content-length")).unwrap().value.parse::<usize>().unwrap();
  let mut received=vec![0u8;length];socket.read_exact(&mut received).await.unwrap();
  socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
  assert_eq!(method,"POST");assert_eq!(length,expected.len());assert_eq!(received,expected);
 });
 let draft:RequestDraft=serde_json::from_value(serde_json::json!({
  "engine":"httpcloak","method":"POST","url":format!("http://{address}/upload"),
  "headers":[{"name":"User-Agent","value":"Chrome/155.0.0.0"},{"name":"Content-Length","value":"1"}],
  "bodyBase64":base64::Engine::encode(&base64::engine::general_purpose::STANDARD,&body),
  "tls":{"preset":"chrome","browserVersion":"155"}
 })).unwrap();
 let (response,_)=tokio::time::timeout(std::time::Duration::from_secs(15),engines.send(&draft)).await.unwrap().unwrap();
 assert_eq!(response.status,200);server.await.unwrap();
}
#[tokio::test]
async fn upgraded_helper_selects_chrome155_from_ua_latest_and_explicit_version(){
 tokio::time::timeout(std::time::Duration::from_secs(15),async{
 let engines=SendEngines::with_roots(rustls::RootCertStore::empty());
 let info=engines.list();let Some(info)=info.iter().find(|e|e.id=="httpcloak"&&e.available)else{eprintln!("Build httpcloak helper to run integration test");return};
 assert_eq!(info.browser_versions.get("chrome").unwrap().first(),Some(&155));
 for(setting,expected)in[("auto",155),("latest",155),("155",155),("153",153),("150",150)]{
  let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
  let server=tokio::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let mut socket=BufReader::new(socket);let raw=capture_core::http1::read_head(&mut socket).await.unwrap();let(_,_,headers)=capture_core::http1::parse_request(&raw).unwrap();socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();headers});
  let draft:RequestDraft=serde_json::from_value(serde_json::json!({"engine":"httpcloak","method":"GET","url":format!("http://{address}/"),"headers":[{"name":"User-Agent","value":"Chrome/155.0.0.0"},{"name":"Accept","value":"application/custom"},{"name":"X-Order","value":"one"},{"name":"x-order","value":"two"}],"bodyBase64":"","tls":{"preset":"chrome","browserVersion":setting}})).unwrap();
  let before=serde_json::to_value(&draft).unwrap();let(response,notes)=engines.send(&draft).await.unwrap();assert_eq!(response.status,200);assert!(notes.iter().any(|n|n.contains(&format!("chrome-{expected}-windows"))));assert_eq!(serde_json::to_value(&draft).unwrap(),before);
  let headers=server.await.unwrap();let fields:Vec<_>=headers.iter().filter(|h|h.name.eq_ignore_ascii_case("user-agent")||h.name.eq_ignore_ascii_case("accept")||h.name.eq_ignore_ascii_case("x-order")).map(|h|(h.name.as_str(),h.value.as_str())).collect();assert_eq!(fields,vec![("User-Agent","Chrome/155.0.0.0"),("Accept","application/custom"),("X-Order","one"),("x-order","two")]);
 }
 }).await.unwrap();
}
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

#[tokio::test]
async fn captured_pseudo_headers_do_not_prevent_replay_or_override_edits(){
 let engines=SendEngines::with_roots(rustls::RootCertStore::empty());
 if !engines.list().iter().any(|e|e.id=="httpcloak"&&e.available){eprintln!("Build httpcloak helper to run integration test");return;}
 let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
 let server=tokio::spawn(async move{
  let(socket,_)=listener.accept().await.unwrap();let mut socket=BufReader::new(socket);
  let raw=capture_core::http1::read_head(&mut socket).await.unwrap();
  let request=capture_core::http1::parse_request(&raw).unwrap();
  socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();request
 });
 let mut draft:RequestDraft=serde_json::from_value(serde_json::json!({
  "engine":"httpcloak","method":"POST","url":format!("http://{address}/edited?q=2"),"headers":[],"bodyBase64":"",
  "pseudoHeaders":[{"name":":method","value":"GET"},{"name":":scheme","value":"https"},{"name":":authority","value":"old.invalid"},{"name":":path","value":"/old"}],
  "tls":{"preset":"chrome","browserVersion":"150"}
 })).unwrap();
 let original=serde_json::to_value(&draft).unwrap();
 let(response,notes)=engines.send(&draft).await.unwrap();assert_eq!(response.status,200);
 assert!(notes.iter().any(|note|note.contains("regenerates HTTP/2 pseudo headers")));
 assert_eq!(serde_json::to_value(&draft).unwrap(),original);
 let(method,path,headers)=server.await.unwrap();assert_eq!(method,"POST");assert_eq!(path,"/edited?q=2");
 assert!(headers.iter().any(|header|header.name.eq_ignore_ascii_case("host")&&header.value==address.to_string()));
 draft.pseudo_headers.push(Header{name:":protocol".into(),value:"websocket".into(),disabled:false});
 let error=format!("{:#}",engines.send(&draft).await.unwrap_err());assert!(error.contains("extended HTTP/2"),"{error}");
}
#[cfg(feature="browser-replay")]
#[tokio::test]
async fn wreq_proxy_rejection(){proxy_connect_auth_and_rejection_never_falls_back("wreq").await;}
