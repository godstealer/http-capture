use anyhow::Result;
use capture_core::{proxy, Engine};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let data = PathBuf::from(args.next().unwrap_or_else(|| ".local/capture".into()));
    let port: u16 = args.next().unwrap_or_else(|| "8080".into()).parse()?;
    let engine = Engine::open(&data)?;
    let handle = proxy::start(engine.clone(), port).await?;
    println!("Proxy: http://{}\nCA: {}\nCapture protocol: HTTP/1.1 (including HTTPS MITM)\nCtrl+C to stop", handle.address, engine.ca.cert_path.display());
    tokio::signal::ctrl_c().await?;
    handle.stop().await;
    Ok(())
}
