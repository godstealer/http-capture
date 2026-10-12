use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde_json::{json, Value};

async fn call(address:std::net::SocketAddr,path:&str,body:Value,authorized:bool)->(u16,Value){
 let body=body.to_string();
 let mut socket=tokio::net::TcpStream::connect(address).await.unwrap();
 let auth=if authorized{"Authorization: Bearer contract-test\r\n"}else{""};
 socket.write_all(format!("POST {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
 let mut bytes=Vec::new();socket.read_to_end(&mut bytes).await.unwrap();
 let split=bytes.windows(4).position(|v|v==b"\r\n\r\n").unwrap();
 let code=std::str::from_utf8(&bytes[..split]).unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
 (code,serde_json::from_slice(&bytes[split+4..]).unwrap_or(Value::Null))
}

#[tokio::test]
async fn authenticated_staging_delta_and_library_contract(){
 tokio::time::timeout(std::time::Duration::from_secs(30),async{
  let temp=tempfile::tempdir().unwrap();let engine=Engine::open(temp.path()).unwrap();
  let state=Arc::new(Service{engine:engine.clone(),proxy:Mutex::new(None),token:"contract-test".into()});
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
  let task=tokio::spawn(async move{axum::serve(listener,router(state)).await.unwrap()});
  for path in ["/database/clear","/database/backup","/imports/begin"]{assert_eq!(call(address,path,json!({}),false).await.0,401);}
  let (code,initial)=call(address,"/flows/changes",json!({"since":null}),true).await;assert_eq!(code,200);assert_eq!(initial["reset"],true);
  let (_,id)=call(address,"/imports/begin",json!({}),true).await;
  // Above the global 12 MiB limit: the staging endpoint has its own 64 MiB bound.
  let flow=json!({"id":"fixture","parentId":null,"startedAt":0,"durationMs":0,"source":"capture","request":{"method":"GET","url":"https://example.invalid/","headers":[],"bodyBase64":""},"response":null,"error":null,"notes":["x".repeat(13*1024*1024)]});
  let (code,count)=call(address,"/imports/append",json!({"id":id,"offset":0,"flows":[flow]}),true).await;assert_eq!(code,200);assert_eq!(count,1);assert!(engine.store.list().unwrap().is_empty());
  assert_eq!(call(address,"/imports/append",json!({"id":id,"offset":0,"flows":[]}),true).await.0,400);
  assert_eq!(call(address,"/imports/finish",json!({"id":id,"commit":true}),true).await,(200,json!(1)));
  let (_,delta)=call(address,"/flows/changes",json!({"since":initial["revision"]}),true).await;assert_eq!(delta["reset"],false);assert_eq!(delta["rows"].as_array().unwrap().len(),1);
  let library=json!({"revision":0,"collections":[],"requests":[],"environments":[],"scripts":[]});
  assert_eq!(call(address,"/library",json!({"library":library}),true).await,(200,json!(1)));
  assert_eq!(call(address,"/library",json!({"library":library}),true).await.0,400);
  assert_eq!(call(address,"/decryption",json!({"hosts":["*.example.com"]}),true).await.0,200);
  assert_eq!(call(address,"/decryption",json!({"hosts":["https://example.com/"]}),true).await.0,400);
  assert_eq!(call(address,"/database/clear",json!({}),true).await,(200,json!(1)));
  let (_,deleted)=call(address,"/flows/changes",json!({"since":delta["revision"]}),true).await;assert_eq!(deleted["deleted"].as_array().unwrap().len(),1);
  task.abort();let _=task.await;
 }).await.expect("API contract stalled");
}

// Safety contract: malformed/stale requests must not reach OS mutations.
#[tokio::test]
async fn system_settings_require_authentication_and_active_listener(){
 let temp=tempfile::tempdir().unwrap();let engine=Engine::open(temp.path()).unwrap();
 let state=Arc::new(Service{engine,proxy:Mutex::new(None),token:"contract-test".into()});
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
 let task=tokio::spawn(async move{axum::serve(listener,router(state)).await.unwrap()});
 assert_eq!(call(address,"/system-proxy",json!({"enabled":true}),false).await.0,401);
 assert_eq!(call(address,"/ca/trust",json!({"install":true,"sha256":"stale"}),false).await.0,401);
 assert_eq!(call(address,"/system-proxy",json!({"enabled":true}),true).await.0,400);
 assert_eq!(call(address,"/ca/trust",json!({"install":true,"sha256":"stale"}),true).await.0,400);
 task.abort();let _=task.await;
}

#[tokio::test]
async fn large_workspace_and_library_survive_restart() {
 let temp=tempfile::tempdir().unwrap();let engine=Engine::open(temp.path()).unwrap();
 let state=Arc::new(Service{engine:engine.clone(),proxy:Mutex::new(None),token:"contract-test".into()});
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
 let task=tokio::spawn(async move{axum::serve(listener,router(state)).await.unwrap()});
 // Reject before reading an unauthenticated body, even with a huge Content-Length.
 let mut socket=tokio::net::TcpStream::connect(address).await.unwrap();
 socket.write_all(b"POST /workspace HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 999999999\r\nConnection: close\r\n\r\n").await.unwrap();
 let mut response=[0;1024];let n=tokio::time::timeout(std::time::Duration::from_secs(3),socket.read(&mut response)).await.unwrap().unwrap();
 assert!(std::str::from_utf8(&response[..n]).unwrap().starts_with("HTTP/1.1 401"));
 let body="YWJj".repeat(4*1024*1024);
 let draft=json!({"method":"POST","url":"https://example.invalid/upload","headers":[],"bodyBase64":body});
 let workspace=json!({"revision":0,"activeEditor":"large","editors":[{"id":"large","draft":draft,"selected":null,"parentId":null}]});
 let library=json!({"revision":0,"collections":[{"id":"collection","name":"Large uploads"}],"requests":[{"id":"saved","name":"Binary","collectionId":"collection","draft":draft}],"environments":[],"scripts":[]});
 assert_eq!(call(address,"/workspace",json!({"workspace":workspace}),true).await,(200,json!(1)));
 assert_eq!(call(address,"/library",json!({"library":library}),true).await,(200,json!(1)));
 assert_eq!(call(address,"/workspace",json!({"workspace":workspace}),true).await.0,400);
 assert_eq!(call(address,"/library",json!({"library":library}),true).await.0,400);
 // Exercise the same oversized draft through the real replay endpoint.
 let origin=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
 let origin_address=origin.local_addr().unwrap();
 let receiver=tokio::spawn(async move {
  use tokio::io::AsyncBufReadExt;
  let (socket,_)=origin.accept().await.unwrap();let mut reader=tokio::io::BufReader::new(socket);
  let mut length=0usize;
  loop {let mut line=String::new();reader.read_line(&mut line).await.unwrap();if line=="\r\n" {break;}if let Some(value)=line.to_ascii_lowercase().strip_prefix("content-length:"){length=value.trim().parse().unwrap();}}
  assert_eq!(length,12*1024*1024);
  let mut remaining=length;let mut chunk=[0;8192];let mut offset=0;
  while remaining>0 {let count=remaining.min(chunk.len());reader.read_exact(&mut chunk[..count]).await.unwrap();for byte in &chunk[..count] {assert_eq!(*byte,b"abc"[offset%3]);offset+=1;}remaining-=count;}
  reader.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
 });
 let mut request=draft.clone();request["url"]=json!(format!("http://{origin_address}/upload"));
 let (code,flow)=call(address,"/replay",json!({"request":request}),true).await;
 assert_eq!(code,200);assert_eq!(flow["response"]["status"],200);assert_eq!(flow["response"]["bodyBase64"],"b2s=");
 receiver.await.unwrap();
 task.abort();let _=task.await;drop(engine);
 let reopened=Engine::open(temp.path()).unwrap();
 assert_eq!(reopened.store.workspace().unwrap().editors[0].draft.body_base64,body);
 assert_eq!(reopened.store.library().unwrap().requests[0].draft.body_base64,body);
 assert_eq!(reopened.store.workspace().unwrap().revision,1);
 assert_eq!(reopened.store.library().unwrap().revision,1);
}
