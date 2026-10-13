//! Independent nghttp2 observer: scripts/h2-order-observer.cjs.
use capture_core::{model::*};
#[tokio::main]
async fn main()->anyhow::Result<()>{
 let url=std::env::args().nth(1).expect("Pass h2c origin URL");
 let request:RequestDraft=serde_json::from_value(serde_json::json!({"method":"GET","url":url,"engine":"h2","headers":[{"name":"x-a","value":"one"},{"name":"x-b","value":"two"},{"name":"x-a","value":"three"},{"name":"cookie","value":"a=1"},{"name":"x-c","value":"four"},{"name":"cookie","value":"b=2"}],"bodyBase64":""}))?;
 let dir=tempfile::tempdir()?;let engine=capture_core::Engine::open(dir.path())?;
 let flow=engine.replay(request,None,None).await?;
 anyhow::ensure!(flow.error.is_none(),"{:?}",flow.error);
 println!("{}",serde_json::to_string_pretty(&flow.response.unwrap().sent_request_headers)?);Ok(())
}
