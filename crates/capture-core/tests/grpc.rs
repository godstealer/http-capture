use capture_core::{Engine,model::RequestDraft};
use base64::{Engine as _,engine::general_purpose::STANDARD};
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
 let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();let body=[0,0,0,0,4,10,2,111,107];
 let request:RequestDraft=serde_json::from_value(serde_json::json!({"method":"POST","url":format!("http://{addr}/test.Echo/Bidi"),"engine":"h2","headers":[{"name":"content-type","value":"application/grpc"},{"name":"te","value":"trailers"}],"bodyBase64":STANDARD.encode(body)})).unwrap();
 let id=capture_core::grpc_client::start(engine.clone(),request,false).unwrap();
 loop{let f=engine.store.get(&id).unwrap().unwrap();if f.response.as_ref().is_some_and(|r|!r.body_base64.is_empty()){assert_eq!(f.state,"receiving");break}assert!(f.error.is_none(),"{:?}",f.error);tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
 capture_core::grpc_client::send(&engine,&id,&STANDARD.encode(body),false).await.unwrap();
 capture_core::grpc_client::send(&engine,&id,"",true).await.unwrap();
 loop{let f=engine.store.get(&id).unwrap().unwrap();if f.state=="completed"{let response=f.response.unwrap();assert_eq!(STANDARD.decode(response.body_base64).unwrap(),body.repeat(2));assert_eq!(STANDARD.decode(f.request.body_base64).unwrap(),body.repeat(2));assert_eq!(response.trailers[0].value,"0");break}assert!(f.error.is_none(),"{:?}",f.error);tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
 assert!(capture_core::grpc_client::send(&engine,&id,"",true).await.is_err());server.await.unwrap();
 }).await.unwrap();
}
