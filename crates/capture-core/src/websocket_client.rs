//! Active HTTP/1 WebSocket sessions share the capture handshake, validation and recorder.
use crate::{Engine, model::*, websocket, http1::*};
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{collections::HashMap, sync::{Arc,Mutex}, time::{Duration,Instant}};
use tokio::{io::{AsyncWriteExt,BufReader},sync::{mpsc,oneshot}};

struct Message {opcode:u8,bytes:Vec<u8>,ack:oneshot::Sender<Result<(),String>>}
#[derive(Default)]
pub struct Clients(Mutex<HashMap<String,mpsc::Sender<Message>>>);

pub fn connect(engine:Arc<Engine>,mut request:RequestDraft)->Result<String>{
 ensure!(matches!(request.engine.as_str(),"auto"|"native"),"WebSocket currently requires Auto or Native (HTTP/1 Upgrade)");
 ensure!(request.tls.preset=="native","WebSocket currently requires Native TLS");
 ensure!(!request.scripts.enabled || (request.scripts.before.trim().is_empty()&&request.scripts.after.trim().is_empty()),"Active WebSocket handshake scripts are not yet supported");
 ensure!(request.body_base64.is_empty(),"WebSocket handshake cannot contain a request body");
 let mut url=url::Url::parse(&request.url)?;
 let scheme=match url.scheme(){"ws"=>"http","wss"=>"https",_=>anyhow::bail!("Use ws:// or wss:// for WebSocket")};
 url.set_scheme(scheme).map_err(|_|anyhow::anyhow!("Invalid WebSocket URL"))?;
 validate_url(url.as_str())?;request.method="GET".into();
 request.headers.retain(|h|!matches!(h.name.to_ascii_lowercase().as_str(),"host"|"connection"|"upgrade"|"sec-websocket-key"|"sec-websocket-version"|"sec-websocket-extensions"|"content-length"|"transfer-encoding"|"expect"|"proxy-authorization"|"proxy-connection"));
 ensure!(request.headers.len()<=512 && request.headers.iter().map(|h|h.name.len()+h.value.len()+4).sum::<usize>()<=60000,"WebSocket request headers too large");
 for h in &request.headers {let _:http::HeaderName=h.name.parse()?;let _=http::HeaderValue::from_bytes(&header_bytes(&h.value)?)?;}
 let key=STANDARD.encode(uuid::Uuid::new_v4().as_bytes());
 for (name,value) in [("Host",authority(&url)),("Connection","Upgrade".into()),("Upgrade","websocket".into()),("Sec-WebSocket-Version","13".into()),("Sec-WebSocket-Key",key)]{request.headers.push(Header{name:name.into(),value});}
 crate::tls_config::config(&request.tls,&engine.upstream_roots,"native")?;
 if let Some(id)=request.upstream_profile_id.as_deref(){engine.upstream.profile_snapshot(id)?;}
 let id=engine.executions.prepare()?;
 let (tx,rx)=mpsc::channel(16);
 engine.websocket_clients.0.lock().unwrap().insert(id.clone(),tx);
 let execution_id=id.clone();
 tokio::spawn(async move{
  let (bridge,client)=tokio::io::duplex(65536);let mut bridge=BufReader::new(bridge);
  let capture=websocket::run(&mut bridge,request,&[],&engine,None,"replay",Some(execution_id.clone()));
  tokio::pin!(capture);
  let driver=drive(client,rx);tokio::pin!(driver);
  tokio::select!{
   _=&mut capture=>{},
   _=&mut driver=>{let _=tokio::time::timeout(Duration::from_secs(3),&mut capture).await;}
  }
  engine.websocket_clients.0.lock().unwrap().remove(&execution_id);
 });
 Ok(id)
}

pub async fn send(engine:&Engine,id:&str,opcode:u8,body:&str)->Result<()> {
 ensure!(matches!(opcode,1|2|8|9),"Send text, binary, ping or close only");
 ensure!(body.len()<=12*1024*1024,"WebSocket message too large");
 let bytes=STANDARD.decode(body)?;ensure!(bytes.len()<=MAX_BODY,"WebSocket message exceeds 8 MiB");
 if opcode==1 {std::str::from_utf8(&bytes)?;}
 if opcode>=8 {ensure!(bytes.len()<=125,"Control payload exceeds 125 bytes");}
 if opcode==8 {ensure!(bytes.is_empty(),"Close uses an empty payload");}
 let tx=engine.websocket_clients.0.lock().unwrap().get(id).cloned().ok_or_else(||anyhow::anyhow!("WebSocket connection is closed"))?;
 let (ack,answer)=oneshot::channel();
 tx.try_send(Message{opcode,bytes,ack}).map_err(|_|anyhow::anyhow!("WebSocket send queue full or connection closed"))?;
 tokio::time::timeout(Duration::from_secs(10),answer).await.map_err(|_|anyhow::anyhow!("WebSocket send timed out; check recorded frames before retrying"))?.map_err(|_|anyhow::anyhow!("WebSocket connection closed before sending"))?.map_err(anyhow::Error::msg)
}

async fn write<W:tokio::io::AsyncWrite+Unpin>(writer:&mut W,opcode:u8,bytes:&[u8])->Result<()> {
 let mut wire=vec![128|opcode];let len=bytes.len();
 if len<126 {wire.push(128|len as u8);}else if len<=65535{wire.push(128|126);wire.extend_from_slice(&(len as u16).to_be_bytes());}else{wire.push(128|127);wire.extend_from_slice(&(len as u64).to_be_bytes());}
 let random=uuid::Uuid::new_v4();let mask=&random.as_bytes()[..4];wire.extend_from_slice(mask);wire.extend(bytes.iter().enumerate().map(|(i,b)|b^mask[i%4]));
 writer.write_all(&wire).await?;writer.flush().await?;Ok(())
}

async fn drive(client:tokio::io::DuplexStream,mut commands:mpsc::Receiver<Message>)->Result<()> {
 let mut client=BufReader::new(client);let head=read_head(&mut client).await?;ensure!(parse_response(&head)?.0==101,"Upgrade failed");
 let (reader,mut writer)=tokio::io::split(client);let (tx,mut frames)=mpsc::channel(8);
 let read=websocket::relay(reader,tokio::io::sink(),false,false,Instant::now(),tx);tokio::pin!(read);
 let mut done=false;let mut closing=None;
 loop {
  tokio::select!{biased;
   Some(frame)=frames.recv()=>{
    let payload=STANDARD.decode(frame.payload_base64)?;
    if frame.opcode==9 {write(&mut writer,10,&payload).await?;}
    if frame.opcode==8 {if closing.is_none(){write(&mut writer,8,&payload).await?;}return Ok(());}
   },
   result=&mut read,if !done=>{result?;done=true;},
   Some(message)=commands.recv()=>{
    if message.ack.is_closed(){continue;}
    if closing.is_some(){let _=message.ack.send(Err("WebSocket is closing".into()));continue;}
    let result=write(&mut writer,message.opcode,&message.bytes).await;
    let failed=result.is_err();let _=message.ack.send(result.map_err(|e|e.to_string()));
    if failed{return Ok(());}
    if message.opcode==8 {closing=Some(Instant::now());}
   },
   _=tokio::time::sleep(Duration::from_millis(100)),if closing.is_some()=>{if closing.unwrap().elapsed()>Duration::from_secs(5){return Ok(());}},
  }
 }
}
