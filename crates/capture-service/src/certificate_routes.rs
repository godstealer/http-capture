use super::{authorize,error,ApiError,Shared};
use axum::{extract::{State,DefaultBodyLimit},http::HeaderMap,routing::{get,post},Json,Router};
use capture_core::certificates::{Snapshot,Import,Change};
#[derive(serde::Deserialize)]#[serde(deny_unknown_fields)]struct ImportInput{input:Import}
#[derive(serde::Deserialize)]#[serde(deny_unknown_fields)]struct ChangeInput{input:Change}
async fn list(State(s):State<Shared>,headers:HeaderMap)->Result<Json<Snapshot>,ApiError>{
 authorize(&headers,&s)?;s.engine.certificates.snapshot().map(Json).map_err(error)
}
async fn import(State(s):State<Shared>,headers:HeaderMap,Json(ImportInput{input}):Json<ImportInput>)->Result<Json<Snapshot>,ApiError>{
 authorize(&headers,&s)?;s.engine.store.configuration_ready().map_err(error)?;
 tokio::task::spawn_blocking(move||s.engine.certificates.import(input)).await.map_err(error)?.map(Json).map_err(error)
}
async fn change(State(s):State<Shared>,headers:HeaderMap,Json(ChangeInput{input}):Json<ChangeInput>)->Result<Json<Snapshot>,ApiError>{
 authorize(&headers,&s)?;s.engine.store.configuration_ready().map_err(error)?;
 tokio::task::spawn_blocking(move||s.engine.certificates.change(input)).await.map_err(error)?.map(Json).map_err(error)
}
pub(super) fn router()->Router<Shared>{Router::new().route("/certificates",get(list)).route("/certificates/import",post(import)).route("/certificates/change",post(change)).layer(DefaultBodyLimit::max(3*1024*1024))}

#[cfg(test)]mod tests{
 use base64::{engine::general_purpose::STANDARD,Engine as _};
 use serde_json::{json,Value};
 use tokio::io::{AsyncReadExt,AsyncWriteExt};
 async fn call(address:std::net::SocketAddr,method:&str,path:&str,body:Value,auth:bool)->(u16,Value){
  let body=body.to_string();let auth=if auth{"Authorization: Bearer certificate-test\r\n"}else{""};let mut socket=tokio::net::TcpStream::connect(address).await.unwrap();socket.write_all(format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();let mut bytes=vec![];socket.read_to_end(&mut bytes).await.unwrap();let split=bytes.windows(4).position(|v|v==b"\r\n\r\n").unwrap();(std::str::from_utf8(&bytes[..split]).unwrap().split_whitespace().nth(1).unwrap().parse().unwrap(),serde_json::from_slice(&bytes[split+4..]).unwrap_or(Value::Null))
 }
 #[tokio::test]async fn certificate_api_is_authenticated_versioned_and_never_returns_private_material(){
  // Certificate generation and key import compete with other crypto tests on Windows CI.
  tokio::time::timeout(std::time::Duration::from_secs(90),async{
   let dir=tempfile::tempdir().unwrap();let engine=capture_core::Engine::open(dir.path()).unwrap();let state=crate::Service::new(engine.clone(),"certificate-test".into()).unwrap();let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let server=tokio::spawn(async move{axum::serve(listener,crate::router(state)).await.unwrap()});
   assert_eq!(call(address,"GET","/certificates",Value::Null,false).await.0,401);
   let(_,snapshot)=call(address,"GET","/certificates",Value::Null,true).await;
   let cert=std::fs::read(&engine.ca.cert_path).unwrap();let key=std::fs::read_to_string(dir.path().join("certificates/capture-ca.key")).unwrap();
   let input=json!({"input":{"name":"Isolated test CA","kind":"ca","certificateBase64":STANDARD.encode(cert),"privateKeyPem":key,"expectedVersion":snapshot["version"]}});
   assert_eq!(call(address,"POST","/certificates/import",input.clone(),false).await.0,401);
   let(status,saved)=call(address,"POST","/certificates/import",input.clone(),true).await;assert_eq!(status,200);assert_eq!(saved["entries"].as_array().unwrap().len(),1);assert_eq!(saved["entries"][0]["enabled"],false);assert!(!saved.to_string().contains("privateKey"));assert!(!saved.to_string().contains("PRIVATE KEY"));
   assert_eq!(call(address,"POST","/certificates/import",input,true).await.0,400);
   let(status,selected)=call(address,"POST","/certificates/change",json!({"input":{"action":"selectCa","id":saved["entries"][0]["id"],"expectedVersion":saved["version"]}}),true).await;assert_eq!(status,200);assert_eq!(selected["restartRequired"],true);
   assert_eq!(call(address,"POST","/certificates/change",json!({"input":{"action":"delete","id":selected["selectedCa"],"expectedVersion":selected["version"]}}),true).await.0,400);
   server.abort();let _=server.await;
  }).await.unwrap();
 }
}
