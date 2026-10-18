//! The single listener/lifecycle owner shared by desktop, REST and MCP adapters.
use crate::{model::{Flow, ProxyStatus, RequestDraft}, proxy::{self, ProxyHandle}, Engine};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use tokio::sync::Mutex;

pub struct CaptureRuntime {
    pub engine: Arc<Engine>,
    pub proxy: Mutex<Option<ProxyHandle>>,
    pub network_lifecycle: Mutex<()>,
    pub shutting_down: AtomicBool,
}
impl CaptureRuntime {
    pub fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine, proxy: Mutex::new(None), network_lifecycle: Mutex::new(()), shutting_down: AtomicBool::new(false) })
    }
    pub fn ensure_network_ready(&self) -> Result<(), String> {
        if self.shutting_down.load(Ordering::Acquire) { Err("应用正在安装更新或退出，请稍后再操作".into()) } else { Ok(()) }
    }
    pub fn resume_network_commands(&self) { self.shutting_down.store(false, Ordering::Release); }
    pub async fn shutdown_network(&self) -> Result<(), String> {
        let _lifecycle = self.network_lifecycle.lock().await;
        self.shutting_down.store(true, Ordering::Release);
        self.engine.executions.cancel_all();
        let engine = self.engine.clone();
        let restored = tokio::task::spawn_blocking(move || engine.system_proxy.restore()).await;
        let mut errors = Vec::new();
        match restored {
            Ok(Ok(())) => {},
            Ok(Err(error)) => errors.push(format!("恢复系统代理失败：{error}")),
            Err(error) => errors.push(format!("恢复系统代理任务失败：{error}")),
        }
        if let Some(proxy) = self.proxy.lock().await.take() { proxy.stop().await; }
        if let Err(error) = self.engine.tun.stop().await { errors.push(format!("停止 TUN 失败：{error}")); }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("；")) }
    }
    pub async fn status(&self) -> ProxyStatus {
        let proxy = self.proxy.lock().await;
        ProxyStatus { upstream: self.engine.upstream.status(), send_engines: self.engine.send_engines.list(), running: proxy.is_some(), address: proxy.as_ref().map(|p| p.address.to_string()), ca_path: self.engine.ca.cert_path.to_string_lossy().into_owned(), browser_replay: cfg!(feature = "browser-replay") }
    }
    pub async fn start_proxy(&self, host: String, port: u16) -> Result<(), String> {
        let _lifecycle = self.network_lifecycle.lock().await;
        self.ensure_network_ready()?;
        let mut handle = self.proxy.lock().await;
        if handle.is_some() { return Err("代理已经启动".into()); }
        let address = crate::network::listener_address(&host, port).map_err(|e| format!("{e:#}"))?;
        *handle = Some(proxy::start_at(self.engine.clone(), address).await.map_err(|e| format!("{e:#}"))?);
        Ok(())
    }
    pub async fn stop_proxy(&self) -> Result<(), String> {
        let _lifecycle = self.network_lifecycle.lock().await;
        let mut handle = self.proxy.lock().await;
        let engine = self.engine.clone();
        let restored = tokio::task::spawn_blocking(move || engine.system_proxy.restore()).await.map_err(|e| e.to_string()).and_then(|value|value.map_err(|e|e.to_string()));
        if let Some(proxy) = handle.take() { proxy.stop().await; }
        restored
    }
    pub async fn replay(&self, request: RequestDraft, parent_id: Option<String>, execution_id: Option<String>) -> Result<Flow, String> {
        let execution_id = {
            let _lifecycle = self.network_lifecycle.lock().await;
            self.ensure_network_ready()?;
            match execution_id { Some(id) => id, None => self.engine.executions.prepare().map_err(|e| e.to_string())? }
        };
        self.engine.replay(request, parent_id, Some(execution_id)).await.map_err(|e| e.to_string())
    }
}
