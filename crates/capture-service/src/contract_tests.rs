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
