//! Private, handle-owned scratch storage. No paths are accepted from the UI.
//! Anonymous/delete-on-close temporary files disappear on cancellation and exit.
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD,Engine as _};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};

pub(crate) struct BodySpool {
    file: Option<tokio::fs::File>,
    memory: std::io::Cursor<Vec<u8>>,
    pub len: u64,
}
impl BodySpool {
    pub async fn decode(encoded: &str) -> Result<Self> {
        if encoded.len()<=64*1024 {
            let bytes=STANDARD.decode(encoded)?;
            return Ok(Self{len:bytes.len() as u64,file:None,memory:std::io::Cursor::new(bytes)});
        }
        // tempfile creates an unlinked file on Unix and delete-on-close on Windows.
        let file=tokio::task::spawn_blocking(tempfile::tempfile).await??;
        Self::decode_into(encoded,file).await
    }
    async fn decode_into(encoded:&str,file:std::fs::File)->Result<Self> {
        let mut result=Self{file:Some(tokio::fs::File::from_std(file)),memory:std::io::Cursor::new(Vec::new()),len:0};
        let mut decoder=base64::read::DecoderReader::new(encoded.as_bytes(),&STANDARD);
        let mut chunk=vec![0u8;64*1024];
        loop {
            let count=std::io::Read::read(&mut decoder,&mut chunk)?;
            if count==0 {break;}
            result.file.as_mut().unwrap().write_all(&chunk[..count]).await?;
            result.len+=count as u64;
        }
        result.file.as_mut().unwrap().flush().await?;
        result.file.as_mut().unwrap().rewind().await?;
        Ok(result)
    }
    pub async fn next_chunk(&mut self)->Result<Vec<u8>> {
        let mut chunk=vec![0;64*1024];let count=match &mut self.file {Some(file)=>file.read(&mut chunk).await?,None=>std::io::Read::read(&mut self.memory,&mut chunk)?};chunk.truncate(count);Ok(chunk)
    }
    pub async fn send<W:AsyncWrite+Unpin+?Sized>(&mut self,writer:&mut W)->Result<()> {
        let copied=if let Some(file)=&mut self.file {
            file.rewind().await?;tokio::io::copy(file,writer).await?
        } else {self.memory.set_position(0);tokio::io::copy(&mut self.memory,writer).await?};
        anyhow::ensure!(copied==self.len,"Incomplete temporary body");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
 use super::*;
 use base64::Engine;
 #[tokio::test]
 async fn binary_roundtrip_rewind_and_invalid_tail() {
  let bytes:Vec<u8>=(0..9*1024*1024+2).map(|i|(i%251) as u8).collect();
  let encoded=STANDARD.encode(&bytes);let mut spool=BodySpool::decode(&encoded).await.unwrap();
  assert_eq!(spool.len,bytes.len() as u64);
  for _ in 0..2 {let mut out=Vec::new();spool.send(&mut out).await.unwrap();assert_eq!(out,bytes);}
  let mut bad=encoded;let n=bad.len();bad.replace_range(n-8..n-7,"!");assert!(BodySpool::decode(&bad).await.is_err());
  let mut empty=BodySpool::decode("").await.unwrap();assert_eq!(empty.len,0);empty.send(&mut tokio::io::sink()).await.unwrap();
 }
 #[tokio::test]
 async fn cancel_blocked_transfer_releases_temporary_file() {
  let dir=tempfile::tempdir().unwrap();
  let file=tempfile::tempfile_in(dir.path()).unwrap();
  let encoded=STANDARD.encode(vec![1;1024*1024]);
  let mut spool=BodySpool::decode_into(&encoded,file).await.unwrap();
  let (mut writer,_reader)=tokio::io::duplex(1);
  let task=tokio::spawn(async move {spool.send(&mut writer).await});
  tokio::task::yield_now().await;task.abort();assert!(task.await.unwrap_err().is_cancelled());
  // Also verifies the handle isn't left open on Windows.
  dir.close().unwrap();
 }
}
