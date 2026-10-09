//! Bounded, backpressured SSE events shared by native transports and capture adapters.
use crate::model::*;
use anyhow::Result;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use tokio::sync::mpsc;

#[derive(Clone)]
pub enum Event { Head(CapturedResponse), Data(Vec<u8>) }
pub struct Context { pub tx: mpsc::Sender<Event>, pub started: Arc<AtomicBool> }
tokio::task_local! { pub static UPDATES: Context; pub static DOWNSTREAM: mpsc::Sender<Event>; }
pub fn active() -> bool { UPDATES.try_with(|_| ()).is_ok() }
pub fn started() -> bool { UPDATES.try_with(|c| c.started.load(Ordering::Relaxed)).unwrap_or(false) }
pub fn is_sse(headers: &[Header]) -> bool {
    headers.iter().any(|h| h.name.eq_ignore_ascii_case("content-type") && h.value.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("text/event-stream"))
}
pub async fn head(response: &CapturedResponse) -> Result<bool> {
    if !active() || !is_sse(&response.headers) { return Ok(false); }
    emit(Event::Head(response.clone())).await?;
    UPDATES.with(|c| c.started.store(true, Ordering::Relaxed));
    Ok(true)
}
pub async fn emit(event: Event) -> Result<()> {
    if let Ok(tx) = UPDATES.try_with(|c| c.tx.clone()) { tx.send(event).await.map_err(|_| anyhow::anyhow!("SSE consumer closed"))?; }
    Ok(())
}
pub async fn downstream(event: Event) -> Result<()> {
    if let Ok(tx) = DOWNSTREAM.try_with(Clone::clone) { tx.send(event).await.map_err(|_| anyhow::anyhow!("SSE client disconnected"))?; }
    Ok(())
}
