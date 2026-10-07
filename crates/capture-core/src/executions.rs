//! Reserve before sending so cancellation also works before execution starts.
use std::{collections::HashMap, sync::Mutex, time::{Duration, Instant}};
use anyhow::{ensure, Context, Result};
use tokio::sync::watch;

struct Entry { created: Instant, started: bool, cancel: watch::Sender<bool> }
#[derive(Default)]
pub struct Executions(Mutex<HashMap<String, Entry>>);
impl Executions {
    pub fn prepare(&self) -> Result<String> {
        let mut entries = self.0.lock().unwrap();
        entries.retain(|_, e| e.started || e.created.elapsed() < Duration::from_secs(60));
        ensure!(entries.len() < 128, "同时执行的请求过多");
        let id = uuid::Uuid::new_v4().to_string();
        entries.insert(id.clone(), Entry { created: Instant::now(), started: false, cancel: watch::channel(false).0 });
        Ok(id)
    }
    pub fn cancel(&self, id: &str) -> bool {
        let entries = self.0.lock().unwrap();
        if let Some(entry) = entries.get(id) { entry.cancel.send_replace(true); true } else { false }
    }
    pub fn begin(&self, id: String) -> Result<Execution<'_>> {
        let mut entries = self.0.lock().unwrap();
        let entry = entries.get_mut(&id).context("执行 ID 不存在或已过期，请重新发送")?;
        ensure!(!entry.started, "该执行已开始，不能重复发送");
        ensure!(entry.created.elapsed() < Duration::from_secs(60), "执行 ID 已过期，请重新发送");
        entry.started = true;
        Ok(Execution { id, cancel: entry.cancel.subscribe(), registry: self })
    }
}
pub struct Execution<'a> { pub id: String, cancel: watch::Receiver<bool>, registry: &'a Executions }
impl Execution<'_> {
    pub async fn cancelled(&mut self) {
        while !*self.cancel.borrow_and_update() {
            if self.cancel.changed().await.is_err() { break; }
        }
    }
}
impl Drop for Execution<'_> {
    fn drop(&mut self) { self.registry.0.lock().unwrap().remove(&self.id); }
}
