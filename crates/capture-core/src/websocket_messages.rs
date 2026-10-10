//! Inspection only: reassembly/decompression never alters the relayed frames.
use crate::{model::Header,websocket::Frame};
use base64::{Engine as _,engine::general_purpose::STANDARD};
use serde::{Serialize,Deserialize};
use flate2::{Decompress,FlushDecompress};
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct Message {pub direction:String,pub at_ms:u64,pub opcode:u8,pub payload_base64:String,pub frame_count:usize,pub error:Option<String>}
struct Direction {pending:Option<(u8,bool,u64,usize,Vec<u8>)>,inflater:Decompress,reset:bool,broken:bool}
pub struct Assembler {client:Direction,server:Direction,total:usize}
impl Assembler {
 pub fn new(headers:&[Header])->Self {
  let extension=headers.iter().filter(|h|h.name.eq_ignore_ascii_case("sec-websocket-extensions")).map(|h|h.value.to_lowercase()).collect::<Vec<_>>().join(";");
  let make=|name:&str|Direction{pending:None,inflater:Decompress::new(false),reset:extension.split(';').any(|v|v.trim()==name),broken:false};
  Self{client:make("client_no_context_takeover"),server:make("server_no_context_takeover"),total:0}
 }
 pub fn push(&mut self,frame:&Frame)->Option<Message>{
  if frame.opcode>=8{return None;}
  let state=if frame.direction=="client"{&mut self.client}else{&mut self.server};
  if frame.opcode!=0 {state.pending=Some((frame.opcode,frame.compressed,frame.at_ms,0,Vec::new()));}
  let pending=state.pending.as_mut()?;pending.3+=1;pending.4.extend(STANDARD.decode(&frame.payload_base64).ok()?);
  if !frame.fin{return None;}
  let (opcode,compressed,at_ms,frame_count,mut payload)=state.pending.take()?;
  let mut error=None;
  if compressed {
   if state.reset {state.inflater=Decompress::new(false);state.broken=false;}
   let result=(||->anyhow::Result<Vec<u8>>{
    anyhow::ensure!(!state.broken,"Previous compressed message failed; dictionary is unavailable");
    payload.extend_from_slice(&[0,0,255,255]);let mut offset=0;let mut decoded=Vec::new();
    loop {
     let before_in=state.inflater.total_in();let before_out=state.inflater.total_out();let mut buffer=[0u8;8192];
     state.inflater.decompress(&payload[offset..],&mut buffer,FlushDecompress::Sync)?;
     let consumed=(state.inflater.total_in()-before_in) as usize;let produced=(state.inflater.total_out()-before_out) as usize;
     offset+=consumed;anyhow::ensure!(self.total+decoded.len()+produced<=8*1024*1024,"Decoded WebSocket messages exceed 8 MiB");decoded.extend_from_slice(&buffer[..produced]);
     if consumed==0&&produced==0 {anyhow::ensure!(offset==payload.len(),"Incomplete compressed message");break;}
     if offset==payload.len()&&produced<buffer.len(){break;}
    }
    Ok(decoded)
   })();
   match result {Ok(decoded)=>payload=decoded,Err(e)=>{state.broken=true;payload.clear();error=Some(e.to_string());}}
  }
  self.total+=payload.len();
  Some(Message{direction:frame.direction.clone(),at_ms,opcode,payload_base64:STANDARD.encode(payload),frame_count,error})
 }
}
#[cfg(test)]
mod tests {
 use super::*;
 fn frame(opcode:u8,fin:bool,compressed:bool,payload:&[u8])->Frame{Frame{direction:"server".into(),at_ms:1,opcode,fin,compressed,payload_base64:STANDARD.encode(payload)}}
 #[test] fn fragments_and_control_frames(){let mut a=Assembler::new(&[]);assert!(a.push(&frame(1,false,false,b"hel")).is_none());assert!(a.push(&frame(9,true,false,b"ping")).is_none());let m=a.push(&frame(0,true,false,b"lo")).unwrap();assert_eq!(STANDARD.decode(m.payload_base64).unwrap(),b"hello");assert_eq!(m.frame_count,2);}
 #[test] fn compression_with_context_takeover(){let mut a=Assembler::new(&[]);let mut compressor=flate2::Compress::new(flate2::Compression::default(),false);for _ in 0..3{let body=b"dictionary reuse dictionary reuse";let mut output=Vec::with_capacity(256);compressor.compress_vec(body,&mut output,flate2::FlushCompress::Sync).unwrap();assert!(output.ends_with(&[0,0,255,255]));output.truncate(output.len()-4);let m=a.push(&frame(1,true,true,&output)).unwrap();assert!(m.error.is_none(),"{:?}",m.error);assert_eq!(STANDARD.decode(m.payload_base64).unwrap(),body);}}
}
