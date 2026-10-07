//! quic-capture <data-dir> <https-origin> [port]
use anyhow::Result;
use capture_core::{http1::validate_url, Engine};
use std::path::PathBuf;
#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let data = PathBuf::from(args.next().unwrap_or_else(|| ".local/capture".into()));
    let target = validate_url(&args.next().ok_or_else(|| anyhow::anyhow!("Usage: quic-capture <data-dir> <https-origin> [port]"))?)?;
    let port: u16 = args.next().unwrap_or_else(|| "8443".into()).parse()?;
    capture_core::quic_proxy::run(Engine::open(&data)?, target, port, None).await
}