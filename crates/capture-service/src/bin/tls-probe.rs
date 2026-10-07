use capture_core::{model::RequestDraft,replay::send};
use base64::{Engine,engine::general_purpose::STANDARD};
#[tokio::main]
async fn main()->anyhow::Result<()> {
 let args:Vec<_>=std::env::args().collect();
 let mut request:RequestDraft=serde_json::from_slice(&std::fs::read(&args[1])?)?;
 for engine in args.iter().skip(2) {
  request.engine=if engine=="chrome"||engine=="firefox" {"wreq"} else {engine}.into();
  request.tls.preset=if request.engine=="wreq"{engine}else{"native"}.into();
  let(response,_)=send(&request).await?;
  anyhow::ensure!(response.status==200,"HTTP {}",response.status);
  let body:serde_json::Value=serde_json::from_slice(&STANDARD.decode(response.body_base64)?)?;
  anyhow::ensure!(body["tls"].is_object(),"Missing TLS echo");
  let report=serde_json::json!({"engine":engine,"tlsConfig":request.tls,"httpVersion":body["http_version"],"tls":body["tls"],"http2":body["http2"]});
  std::fs::write(format!(".local/tls-peet-{engine}.json"),serde_json::to_vec_pretty(&report)?)?;
  println!("{}",serde_json::json!({"engine":engine,"status":response.status,"protocol":body["http_version"],"tls":body["tls"]["tls_version_negotiated"],"ja3Hash":body["tls"]["ja3_hash"]}));
 }
 Ok(())
}
