//! Explicit external QUIC diagnostic; never falls back to TCP or an upstream proxy.
use capture_core::{model::RequestDraft, replay::send};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut urls: Vec<_> = std::env::args().skip(1).collect();
    let state = capture_core::upstream::UpstreamState::default();
    if urls.first().is_some_and(|v| v == "--proxy") {
        anyhow::ensure!(urls.len() >= 3, "Usage: quic-probe [--proxy socks5://host:port] <https-url> ...");
        state.update(capture_core::upstream::UpstreamInput { enabled:true, url:urls[1].clone(), username:String::new(), password:None, auth_enabled:false })?;
        urls.drain(..2);
    }
    anyhow::ensure!(!urls.is_empty(), "Usage: quic-probe <https-url> ...");
    for url in urls {
        let draft: RequestDraft = serde_json::from_value(serde_json::json!({
            "url": url, "method": "GET", "engine": "h3", "headers": [],
            "bodyBase64": "", "tls": {"preset": "native"}
        }))?;
        let started = std::time::Instant::now();
        let result = if let Some(proxy) = state.snapshot() {
            let engine = capture_core::Engine::open(std::path::Path::new(".local/quic-probe-data"))?;
            tokio::time::timeout(std::time::Duration::from_secs(45), capture_core::multiplex::h3_send_via(&draft, &engine.upstream_roots, Some(&proxy))).await?
        } else { send(&draft).await };
        let report = match result {
            Ok((response, _)) => serde_json::json!({"url":url,"status":response.status,"protocol":response.version,"tls":response.tls_version}),
            Err(error) => serde_json::json!({"url":url,"error":format!("{error:#}")}),
        };
        println!("{}", serde_json::json!({"result": report,"elapsedMs":started.elapsed().as_millis()}));
    }
    Ok(())
}
