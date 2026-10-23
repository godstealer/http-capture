use capture_core::{Engine,model::RequestDraft};
use base64::{Engine as _,engine::general_purpose::STANDARD};
#[tokio::test]
async fn oversized_interactive_message_is_rejected_before_base64_decode() {
 let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
 let encoded="!".repeat((16usize*1024*1024).div_ceil(3)*4+4);
 let error=capture_core::grpc_client::send(&engine,"missing",&encoded,false).await.unwrap_err();
 assert!(error.to_string().contains("16 MiB"),"{error:#}");
}
#[tokio::test]
async fn invalid_initial_body_is_rejected_before_creating_a_session() {
 let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
 let request:RequestDraft=serde_json::from_value(serde_json::json!({
  "method":"POST","url":"http://127.0.0.1:12345/test.Echo/Call","engine":"h2",
  "headers":[],"bodyBase64":"!!!!"
 })).unwrap();
 assert!(capture_core::grpc_client::start(engine.clone(),request,true).await.is_err());
 assert!(engine.store.list().unwrap().is_empty());
}
#[tokio::test]
async fn grpc_h2c_preserves_messages_and_trailers() {
 tokio::time::timeout(std::time::Duration::from_secs(10),async {
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
 let server=tokio::spawn(async move {
  let (socket,_)=listener.accept().await.unwrap();let mut connection=h2::server::handshake(socket).await.unwrap();
  let (request,mut respond)=connection.accept().await.unwrap().unwrap();
  let task=tokio::spawn(async move {
   assert_eq!(request.uri().path(),"/test.Echo/Call");assert_eq!(request.headers()["te"],"trailers");
   let mut recv=request.into_body();let mut body=Vec::new();while let Some(chunk)=recv.data().await {let chunk=chunk.unwrap();recv.flow_control().release_capacity(chunk.len()).unwrap();body.extend_from_slice(&chunk);}
   let head=http::Response::builder().header("content-type","application/grpc").body(()).unwrap();let mut send=respond.send_response(head,false).unwrap();
   send.send_data(bytes::Bytes::from(body),false).unwrap();let mut trailers=http::HeaderMap::new();trailers.insert("grpc-status",http::HeaderValue::from_static("0"));trailers.insert("x-trailer",http::HeaderValue::from_static("preserved"));send.send_trailers(trailers).unwrap();
  });
  while connection.accept().await.is_some(){} task.await.unwrap();
 });
 let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();let body=[0,0,0,0,4,10,2,111,107];
 let request:RequestDraft=serde_json::from_value(serde_json::json!({"method":"POST","url":format!("http://{addr}/test.Echo/Call"),"engine":"h2","headers":[{"name":"content-type","value":"application/grpc"},{"name":"te","value":"trailers"}],"bodyBase64":STANDARD.encode(body)})).unwrap();
 let flow=engine.replay(request,None,None).await.unwrap();assert!(flow.error.is_none(),"{:?}",flow.error);let response=flow.response.unwrap();assert_eq!(STANDARD.decode(response.body_base64).unwrap(),body);assert!(response.trailers.iter().any(|h|h.name=="grpc-status"&&h.value=="0"));assert!(response.trailers.iter().any(|h|h.name=="x-trailer"));server.await.unwrap();
 }).await.unwrap();
}

#[tokio::test]
async fn interactive_bidi_receives_before_half_close_and_records_all_messages() {
 tokio::time::timeout(std::time::Duration::from_secs(10),async{
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
 let server=tokio::spawn(async move{
  let (socket,_)=listener.accept().await.unwrap();let mut connection=h2::server::handshake(socket).await.unwrap();
  let (request,mut respond)=connection.accept().await.unwrap().unwrap();let task=tokio::spawn(async move{
   let head=http::Response::builder().header("content-type","application/grpc").body(()).unwrap();let mut send=respond.send_response(head,false).unwrap();let mut recv=request.into_body();
   while let Some(chunk)=recv.data().await{let chunk=chunk.unwrap();recv.flow_control().release_capacity(chunk.len()).unwrap();send.send_data(chunk,false).unwrap();}
   let mut trailers=http::HeaderMap::new();trailers.insert("grpc-status",http::HeaderValue::from_static("0"));send.send_trailers(trailers).unwrap();
  });while connection.accept().await.is_some(){}task.await.unwrap();
 });
 let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
 let payload=vec![0x5a;96*1024];let mut body=vec![0];body.extend_from_slice(&(payload.len() as u32).to_be_bytes());body.extend_from_slice(&payload);
 let request:RequestDraft=serde_json::from_value(serde_json::json!({"method":"POST","url":format!("http://{addr}/test.Echo/Bidi"),"engine":"h2","headers":[{"name":"content-type","value":"application/grpc"},{"name":"te","value":"trailers"}],"bodyBase64":STANDARD.encode(&body)})).unwrap();
 let id=capture_core::grpc_client::start(engine.clone(),request,false).await.unwrap();
 loop{let f=engine.store.get(&id).unwrap().unwrap();if f.response.as_ref().is_some_and(|r|!r.body_base64.is_empty()){assert_eq!(f.state,"receiving");break}assert!(f.error.is_none(),"{:?}",f.error);tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
 capture_core::grpc_client::send(&engine,&id,&STANDARD.encode(&body),false).await.unwrap();
 capture_core::grpc_client::send(&engine,&id,"",true).await.unwrap();
 loop{let f=engine.store.get(&id).unwrap().unwrap();if f.state=="completed"{let response=f.response.unwrap();assert_eq!(STANDARD.decode(response.body_base64).unwrap(),body.repeat(2));assert_eq!(STANDARD.decode(f.request.body_base64).unwrap(),body.repeat(2));assert_eq!(response.trailers[0].value,"0");break}assert!(f.error.is_none(),"{:?}",f.error);tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
 assert!(capture_core::grpc_client::send(&engine,&id,"",true).await.is_err());server.await.unwrap();
 }).await.unwrap();
}

#[tokio::test]
async fn half_closed_large_initial_message_is_forwarded_without_append() {
 tokio::time::timeout(std::time::Duration::from_secs(10),async{
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
 let server=tokio::spawn(async move{
  let (socket,_)=listener.accept().await.unwrap();let mut connection=h2::server::handshake(socket).await.unwrap();
  let (request,mut respond)=connection.accept().await.unwrap().unwrap();
  let task=tokio::spawn(async move{
   let mut recv=request.into_body();let mut received=Vec::new();
   while let Some(chunk)=recv.data().await{let chunk=chunk.unwrap();recv.flow_control().release_capacity(chunk.len()).unwrap();received.extend_from_slice(&chunk);}
   let head=http::Response::builder().header("content-type","application/grpc").body(()).unwrap();
   let mut send=respond.send_response(head,false).unwrap();send.send_data(bytes::Bytes::from(received.clone()),true).unwrap();received
  });
  while connection.accept().await.is_some(){}task.await.unwrap()
 });
 let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
 let mut body=vec![0];body.extend_from_slice(&(96u32*1024).to_be_bytes());body.extend(std::iter::repeat_n(0x42,96*1024));
 let request:RequestDraft=serde_json::from_value(serde_json::json!({"method":"POST","url":format!("http://{addr}/test.Echo/Once"),"engine":"h2","headers":[{"name":"content-type","value":"application/grpc"},{"name":"te","value":"trailers"}],"bodyBase64":STANDARD.encode(&body)})).unwrap();
 let id=capture_core::grpc_client::start(engine.clone(),request,true).await.unwrap();
 loop{let flow=engine.store.get(&id).unwrap().unwrap();if flow.state=="completed"{assert_eq!(flow.request.body_base64,STANDARD.encode(&body).into());assert_eq!(STANDARD.decode(flow.response.unwrap().body_base64).unwrap(),body);break}assert!(flow.error.is_none(),"{:?}",flow.error);tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
 assert_eq!(server.await.unwrap(),body);
 }).await.unwrap();
}

#[tokio::test]
async fn cancelling_interactive_stream_preserves_received_response_bytes() {
 tokio::time::timeout(std::time::Duration::from_secs(10),async{
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
 let (sent_tx,sent_rx)=tokio::sync::oneshot::channel();
 let server=tokio::spawn(async move{
  let(socket,_)=listener.accept().await.unwrap();let mut connection=h2::server::handshake(socket).await.unwrap();
  let (_,mut respond)=connection.accept().await.unwrap().unwrap();
  let head=http::Response::builder().header("content-type","application/grpc").body(()).unwrap();let mut send=respond.send_response(head,false).unwrap();
  let bytes=[0,0,0,0,4,10,2,111,107];send.send_data(bytes::Bytes::copy_from_slice(&bytes),false).unwrap();let _=sent_tx.send(());
  while connection.accept().await.is_some(){}
 });
 let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
 let request:RequestDraft=serde_json::from_value(serde_json::json!({"method":"POST","url":format!("http://{addr}/test.Echo/Stream"),"engine":"h2","headers":[{"name":"content-type","value":"application/grpc"},{"name":"te","value":"trailers"}],"bodyBase64":""})).unwrap();
 let id=capture_core::grpc_client::start(engine.clone(),request,true).await.unwrap();sent_rx.await.unwrap();
 loop{if engine.store.get(&id).unwrap().unwrap().state=="receiving"{break}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
 tokio::time::sleep(std::time::Duration::from_millis(30)).await;
 assert!(engine.executions.cancel(&id));
 loop{let flow=engine.store.get(&id).unwrap().unwrap();if flow.state=="cancelled"{assert_eq!(STANDARD.decode(flow.response.unwrap().body_base64).unwrap(),[0,0,0,0,4,10,2,111,107]);break}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
 server.await.unwrap();
 }).await.unwrap();
}
