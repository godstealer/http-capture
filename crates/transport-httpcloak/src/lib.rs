//! One isolated helper per request. Cancellation drops/kills the child, with no local TCP listener.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::{Path, PathBuf}, process::Stdio};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, process::Command};
use transport_api::{CapturedResponse, RequestDraft};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all="camelCase")]
pub struct Capabilities { #[serde(default)] pub custom_client_hello: bool, pub protocol_version: u32, pub browser_versions: BTreeMap<String,Vec<u16>> }
#[derive(Deserialize)]
pub struct Reply { pub response: Option<CapturedResponse>, #[serde(default)] pub notes: Vec<String>, pub error: Option<String> }
#[derive(Clone)]
pub struct Helper { pub path: PathBuf, pub capabilities: Capabilities }
impl Helper {
 pub fn load(path: impl AsRef<Path>)->Result<Self>{
  let path=path.as_ref().to_path_buf();ensure!(path.is_file(),"httpcloak helper not built");
  let capabilities:Capabilities=serde_json::from_slice(&std::fs::read(path.with_extension("json"))?)?;
  ensure!(capabilities.protocol_version==1,"Unsupported httpcloak helper protocol");
  Ok(Self{path,capabilities})
 }
 pub async fn send<S:Serialize>(&self,request:&RequestDraft<S>,preset:&str,proxy:Option<&transport_api::UpstreamProxyConfig>)->Result<(CapturedResponse,Vec<String>)>{
  ensure!(request.tls.client_hello_hex.is_none() || self.capabilities.custom_client_hello,"This httpcloak helper does not support ClientHello Hex; rebuild the helper");
  let input=serde_json::to_vec(&serde_json::json!({"request":request,"preset":preset,"proxy":proxy}))?;
  ensure!(input.len()<=16*1024*1024,"Request exceeds helper IPC limit");
  let mut cmd=Command::new(&self.path);
  cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
  #[cfg(windows)] cmd.creation_flags(0x08000000);
  let mut child=cmd.spawn().context("Starting httpcloak helper")?;
  let mut stdin=child.stdin.take().context("Helper stdin unavailable")?;
  let mut stdout=child.stdout.take().context("Helper stdout unavailable")?.take(16*1024*1024+1);
  let write=async { stdin.write_all(&input).await?;stdin.shutdown().await?;drop(stdin);Ok::<_,std::io::Error>(()) };
  let read=async {let mut bytes=Vec::new();stdout.read_to_end(&mut bytes).await?;Ok::<_,std::io::Error>(bytes)};
  let (_,bytes)=tokio::try_join!(write,read)?;
  ensure!(bytes.len()<=16*1024*1024,"Helper response exceeds IPC limit");
  let status=child.wait().await?;
  let reply:Reply=serde_json::from_slice(&bytes).context("Invalid helper response")?;
  if let Some(error)=reply.error{anyhow::bail!("{error}");}
  ensure!(status.success(),"httpcloak helper exited unsuccessfully");
  Ok((reply.response.context("Missing helper response")?,reply.notes))
 }
}
