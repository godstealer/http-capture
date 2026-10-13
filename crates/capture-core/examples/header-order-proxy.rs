//! Isolated local comparison proxy; never changes system proxy settings.
#[tokio::main]
async fn main()->anyhow::Result<()>{
 let dir=tempfile::tempdir()?;
 let engine=capture_core::Engine::open(dir.path())?;
 let proxy=capture_core::proxy::start(engine,0).await?;
 println!("Comparison proxy: {}",proxy.address);
 tokio::signal::ctrl_c().await?;proxy.stop().await;Ok(())
}
