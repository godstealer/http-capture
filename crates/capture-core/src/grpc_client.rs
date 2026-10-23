//! Interactive native gRPC sessions. Request half-close is independent of response completion.
use crate::{Engine,model::*,http1::*,body_spool::BodySpool};
use anyhow::{Result,ensure};
use base64::{Engine as _,engine::general_purpose::STANDARD};
use std::{sync::{Arc,Mutex},collections::HashMap,time::{Duration,Instant,SystemTime,UNIX_EPOCH}};
use tokio::sync::{mpsc,oneshot};
use bytes::Bytes;
struct Action{body:Vec<u8>,end:bool,ack:oneshot::Sender<Result<(),String>>}
#[derive(Default)]pub struct Sessions(Mutex<HashMap<String,mpsc::Sender<Action>>>);
struct Abort(tokio::task::JoinHandle<Result<()>>);
impl Drop for Abort{fn drop(&mut self){self.0.abort()}}
fn save(engine:&Engine,flow:&Flow)->Result<()>{engine.store.insert(flow)?;if engine.events.receiver_count()>0{let _=engine.events.send(flow.clone());}Ok(())}
fn save_streaming(engine:&Engine,flow:&Flow,slot:&str)->Result<()>{engine.store.insert_streaming(flow,slot)?;if engine.events.receiver_count()>0{let _=engine.events.send(flow.clone());}Ok(())}
pub async fn start(engine:Arc<Engine>,mut request:RequestDraft,half_close:bool)->Result<String>{
 request.retain_enabled_headers();
 ensure!(request.tls.preset=="native","Interactive gRPC requires Native TLS");
 ensure!(!request.scripts.enabled,"Interactive gRPC scripts are not supported; disable scripts for this request");
 validate_url(&request.url)?;request.engine="h2".into();request.method="POST".into();
 let body=BodySpool::decode(&request.body_base64).await?;
 request.headers.retain(|h|!matches!(h.name.to_ascii_lowercase().as_str(),"content-length"|"transfer-encoding"));
 let id=engine.executions.prepare()?;
 let flow=initial_flow(id.clone(),request)?;
 save(&engine,&flow)?;
 let (tx,rx)=mpsc::channel(16);
 engine.grpc_clients.0.lock().unwrap().insert(id.clone(),tx);
 let output=id.clone();
 tokio::spawn(async move{
  let started=Instant::now();let flow=Arc::new(Mutex::new(flow));
  let result:Result<()>=async{
   let mut execution=engine.executions.begin(id.clone())?;
   tokio::select!{biased;_=execution.cancelled()=>{flow.lock().unwrap().state="cancelled".into();anyhow::bail!("gRPC cancelled")}, result=tokio::time::timeout(Duration::from_secs(300),drive(engine.clone(),flow.clone(),body,half_close,rx))=>result.map_err(|_|anyhow::anyhow!("gRPC session timeout (300 seconds)"))?}
  }.await;
  engine.grpc_clients.0.lock().unwrap().remove(&id);
  let mut flow=flow.lock().unwrap();flow.duration_ms=started.elapsed().as_millis() as u64;
  if !matches!(flow.state.as_str(),"cancelled"|"blocked"){flow.state=if result.is_ok(){"completed"}else{"failed"}.into()}
  flow.error=result.err().map(|e|format!("{e:#}"));let _=save(&engine,&flow);
 });Ok(output)
}
fn initial_flow(id:String,request:RequestDraft)->Result<Flow>{
 Ok(Flow{
  traffic_origin:None,state:"sending".into(),websocket:None,original_request:None,original_response:None,
  client_tls:None,client_protocol:None,id,parent_id:None,
  started_at:SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,duration_ms:0,
  source:"replay".into(),request,response:None,error:None,raw_request_head_base64:None,
  notes:vec!["Interactive gRPC: Native HTTP/2; request half-close is independent of response completion.".into()],
 })
}
pub async fn send(engine:&Engine,id:&str,body:&str,end:bool)->Result<()>{
 ensure!(body.len()<=(16usize*1024*1024).div_ceil(3)*4,"gRPC message exceeds 16 MiB interactive limit");
 let body=STANDARD.decode(body)?;ensure!(body.len()<=16*1024*1024,"gRPC message exceeds 16 MiB interactive limit");
 let tx=engine.grpc_clients.0.lock().unwrap().get(id).cloned().ok_or_else(||anyhow::anyhow!("gRPC session is closed"))?;
 let (ack,rx)=oneshot::channel();tx.send(Action{body,end,ack}).await.map_err(|_|anyhow::anyhow!("gRPC request stream is closed"))?;
 tokio::time::timeout(Duration::from_secs(30),rx).await.map_err(|_|anyhow::anyhow!("gRPC send timed out"))??.map_err(anyhow::Error::msg)
}
async fn drive(engine:Arc<Engine>,flow:Arc<Mutex<Flow>>,mut initial:BodySpool,half_close:bool,mut rx:mpsc::Receiver<Action>)->Result<()>{
 { let mut current=flow.lock().unwrap(); if engine.blocking.apply(&mut current) { save(&engine,&current)?; return Ok(()); } }
 let request=flow.lock().unwrap().request.clone();let url=validate_url(&request.url)?;
 let route=request.upstream_profile_id.as_deref().map(|id|engine.upstream.profile_snapshot(id)).transpose()?;
 let conn=crate::upstream::connect(&url,route.as_ref()).await?;ensure!(!conn.absolute_form,"h2c requires direct or SOCKS5 connection");
 let mut details=None;
 let socket:crate::replay::Stream=if url.scheme()=="https"{
  let config=engine.certificates.config(&url,&request.tls,&engine.upstream_roots,"h2")?;
  let tls=tokio_rustls::TlsConnector::from(Arc::new(config)).connect(hostname(&url).try_into()?,conn.stream).await?;
  ensure!(tls.get_ref().1.alpn_protocol()==Some(b"h2"),"Server did not negotiate h2");details=Some(crate::tls_details::negotiated(tls.get_ref().1));Box::new(tls)
 }else{conn.stream};
 let (head,_,_,sent)=crate::multiplex::request_h2(&request).await?;
 let (sender,connection)=h2::client::handshake(socket).await?;
 let _driver=Abort(tokio::spawn(async move{connection.await?;Ok(())}));
 let (response,mut stream)=sender.ready().await?.send_request(head,false)?;
 let upload_engine=engine.clone();let upload_flow=flow.clone();
 let mut upload=Abort(tokio::spawn(async move{
  let total=initial.len;let mut sent=0u64;
  if total==0 {crate::multiplex::send_h2_chunk(&mut stream,Bytes::new(),half_close).await?;}
  loop {
   let chunk=initial.next_chunk().await?;
   if chunk.is_empty(){break;}
   sent+=chunk.len() as u64;
   crate::multiplex::send_h2_chunk(&mut stream,Bytes::from(chunk),half_close&&sent==total).await?;
  }
  ensure!(sent==total,"Incomplete initial gRPC request body");
  if half_close{return Ok(())}
  while let Some(action)=rx.recv().await{
   let chunk=Bytes::from(action.body);
   let result=crate::multiplex::send_h2_chunk(&mut stream,chunk.clone(),action.end).await;
   if let Err(e)=result{let _=action.ack.send(Err(e.to_string()));return Err(e)}
   if !chunk.is_empty() {
    let mut f=upload_flow.lock().unwrap();
    crate::body_base64::append_complete(&mut f.request.body_base64,&chunk)?;
    save_streaming(&upload_engine,&f,"request")?;
   }
   let _=action.ack.send(Ok(()));
   if action.end{return Ok(())}
  }Ok(())
 }));
 let receiving=async{
  let response=response.await?;let status=response.status().as_u16();let headers=crate::multiplex::headers(response.headers());
  {let mut f=flow.lock().unwrap();f.state="receiving".into();f.response=Some(CapturedResponse{trailers:vec![],status,version:"HTTP/2".into(),headers,body_base64:Default::default(),raw_head_base64:None,tls_version:details.as_ref().and_then(|d|d.version.clone()),upstream_tls:details,sent_request_headers:Some(sent)});save(&engine,&f)?;}
  let mut recv=response.into_body();
  let mut tick=tokio::time::interval(Duration::from_millis(250));
  tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
  tick.tick().await;
  let mut dirty=false;
  loop{tokio::select!{
   next=recv.data()=>match next{
    Some(Ok(chunk))=>{
     recv.flow_control().release_capacity(chunk.len())?;
     let mut f=flow.lock().unwrap();
     crate::body_base64::append_complete(&mut f.response.as_mut().expect("gRPC response initialized").body_base64,&chunk)?;
     dirty=true;
    },
    Some(Err(error))=>return Err(error.into()),
    None=>break,
   },
   _=tick.tick(),if dirty=>{
    let f=flow.lock().unwrap();save_streaming(&engine,&f,"response")?;dirty=false;
   }
  }}
  let trailers=recv.trailers().await?.map(|m|crate::multiplex::headers(&m)).unwrap_or_default();flow.lock().unwrap().response.as_mut().unwrap().trailers=trailers;Ok::<(),anyhow::Error>(())
 };tokio::pin!(receiving);
 tokio::select!{result=&mut receiving=>result,result=&mut upload.0=>{result??;receiving.await}}
}

#[cfg(test)]
mod tests {
 use super::*;

 #[test]
 fn initial_flow_moves_large_request_body_without_json_round_trip() {
  let mut request:RequestDraft=serde_json::from_value(serde_json::json!({
   "method":"POST","url":"https://example.test/service.Call/Method","headers":[],"bodyBase64":""
  })).unwrap();
  request.body_base64="A".repeat(8*1024*1024).into();
  let pointer=request.body_base64.as_ptr();
  let flow=initial_flow("session".into(),request).unwrap();
  assert_eq!(flow.request.body_base64.as_ptr(),pointer);
  assert_eq!(flow.state,"sending");
  assert_eq!(flow.source,"replay");
  assert_eq!(flow.id,"session");
  assert_eq!(flow.notes.len(),1);
 }
}
