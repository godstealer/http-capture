pub mod browser_profiles;
pub mod tls_config;
pub mod tun;
pub mod network;
pub mod executions;
pub mod script_tools;
pub mod scripts;
pub mod intercept;
pub mod tls_details;
pub mod upstream;
mod socks_udp;
pub mod ca;
pub mod http1;
pub mod model;
pub mod proxy;
pub mod replay;
pub mod store;
pub mod transport;
pub mod multiplex;
pub mod quic_proxy;
mod quic_diagnostics;

use anyhow::Result;
use model::*;
use std::{path::Path, sync::Arc, time::{Instant, SystemTime, UNIX_EPOCH}};
use tokio::sync::broadcast;

pub struct Engine {
    pub tun: tun::TunState,
    pub executions: executions::Executions,
    pub scripts: scripts::ScriptState,
    pub intercept: intercept::Interceptor,
    pub upstream: upstream::UpstreamState,
    pub store: Arc<store::Store>,
    pub ca: ca::CertificateAuthority,
    pub events: broadcast::Sender<Flow>,
    pub upstream_roots: rustls::RootCertStore,
    pub send_engines: transport::SendEngines,
}

struct PendingFlow<'a> { engine: &'a Engine, flow: Option<Flow> }
impl Drop for PendingFlow<'_> {
    fn drop(&mut self) {
        if let Some(mut flow) = self.flow.take() {
            flow.error = Some("请求已取消：代理停止或连接超时".into());
            if self.engine.store.insert(&flow).is_ok() { let _ = self.engine.events.send(flow); }
        }
    }
}

impl Engine {
    pub fn open(data_dir: &Path) -> Result<Arc<Self>> {
        Self::open_with_roots(data_dir, rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()))
    }
    /// Explicit trust store injection for enterprise roots and isolated local tests.
    /// Never bypasses hostname or certificate verification.
    pub fn open_with_roots(data_dir: &Path, upstream_roots: rustls::RootCertStore) -> Result<Arc<Self>> {
        let send_engines = transport::SendEngines::with_roots(upstream_roots.clone());
        Self::open_with_engines(data_dir, upstream_roots, send_engines)
    }
    pub fn open_with_engines(data_dir: &Path, upstream_roots: rustls::RootCertStore,
        send_engines: transport::SendEngines) -> Result<Arc<Self>> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        std::fs::create_dir_all(data_dir)?;
        let (events, _) = broadcast::channel(128);
        let store=Arc::new(store::Store::open(&data_dir.join("sessions.db"))?);
        Ok(Arc::new(Self { tun: Default::default(), executions: Default::default(), scripts: scripts::ScriptState::open(store.clone())?, intercept: intercept::Interceptor::open(store.clone())?, upstream: upstream::UpstreamState::open(data_dir)?, store,
            ca: ca::CertificateAuthority::load_or_create(&data_dir.join("certificates"))?, events,
            send_engines, upstream_roots }))
    }
    pub async fn execute(&self, request: RequestDraft, parent_id: Option<String>, source: &str,
        raw_request_head_base64: Option<String>) -> Result<Flow> {
        self.execute_protocol(request, parent_id, source, raw_request_head_base64, None).await
    }
    pub async fn execute_protocol(&self, request: RequestDraft, parent_id: Option<String>, source: &str,
        raw_request_head_base64: Option<String>, client_protocol: Option<String>) -> Result<Flow> {
        self.execute_with_tls(request, parent_id, source, raw_request_head_base64, client_protocol, None).await
    }
    pub async fn execute_with_tls(&self, request: RequestDraft, parent_id: Option<String>, source: &str,
        raw_request_head_base64: Option<String>, client_protocol: Option<String>, client_tls: Option<TlsDetails>) -> Result<Flow> {
        self.execute_inner(request, parent_id, source, raw_request_head_base64, client_protocol, client_tls, None).await
    }
    pub async fn replay(&self, request: RequestDraft, parent_id: Option<String>, execution_id: Option<String>) -> Result<Flow> {
        let id = match execution_id { Some(id) => id, None => self.executions.prepare()? };
        let execution = self.executions.begin(id)?;
        self.execute_inner(request, parent_id, "replay", None, None, None, Some(execution)).await
    }
    async fn execute_inner(&self, request: RequestDraft, parent_id: Option<String>, source: &str,
        raw_request_head_base64: Option<String>, client_protocol: Option<String>, client_tls: Option<TlsDetails>,
        mut execution: Option<executions::Execution<'_>>) -> Result<Flow> {
        let started_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
        let start = Instant::now();
        let mut flow = Flow { original_request: None, original_response: None, client_tls, id: execution.as_ref().map(|e|e.id.clone()).unwrap_or_else(||uuid::Uuid::new_v4().to_string()), parent_id, started_at, duration_ms: 0,
            source: source.into(), request, response: None, error: None, raw_request_head_base64, notes: vec![], client_protocol };
        self.store.insert(&flow)?;
        let _ = self.events.send(flow.clone());
        let mut pending = PendingFlow { engine: self, flow: Some(flow.clone()) };
        let upstream = if source == "capture" { Ok(self.upstream.snapshot()) } else { flow.request.upstream_profile_id.as_deref().map(|id|self.upstream.profile_snapshot(id)).transpose() };
        let (mut script, revision) = if source == "capture" { self.scripts.snapshot() } else { (flow.request.scripts.clone(),0) };
        let rules = self.intercept.snapshot().config.rules;
        let operation = async {
            let route=upstream.as_ref().map_err(|e|anyhow::anyhow!("{e:#}"))?.as_ref();
            script.validate()?;
            if script.enabled && !script.before.trim().is_empty() && (source != "capture" || rules.iter().any(|r|r.matches(&flow))) {
                self.apply_script(&mut flow, &mut script, revision, false).await?;
            }
            if let Some(d) = self.intercept.pause(&flow, "request").await? {
                if d.action == "modify" { flow.original_request.get_or_insert_with(||flow.request.clone()); flow.request = d.request.unwrap(); flow.notes.push("请求已通过拦截编辑；原始请求另行保留。".into()); }
                if d.action == "replace" { flow.response = d.response; flow.notes.push("请求未发往上游，已返回自定义响应。".into()); return Ok(()); }
            }
            let (response, notes) = self.send_engines.send_via(&flow.request, route).await?;
            flow.response = Some(response); flow.notes.extend(notes);
            if script.enabled && !script.after.trim().is_empty() && (source != "capture" || rules.iter().any(|r|r.matches(&flow))) {
                self.apply_script(&mut flow, &mut script, revision, true).await?;
            }
            if let Some(d) = self.intercept.pause(&flow, "response").await? {
                if d.action == "modify" || d.action == "replace" { if flow.original_response.is_none() { flow.original_response = flow.response.clone(); } flow.response = d.response; flow.notes.push("响应已通过拦截修改或替换；原始响应另行保留。".into()); }
            }
            Ok(())
        };
        let outcome: Result<()> = tokio::select! {
            biased;
            _ = async { match execution.as_mut() { Some(e) => e.cancelled().await, None => std::future::pending::<()>().await } } => Err(anyhow::anyhow!("请求已取消：用户取消发送")),
            result = operation => result,
        };
        if let Err(err) = outcome { flow.error = Some(format!("{err:#}")); flow.response = None; }
        if let Ok(Some(proxy)) = upstream { flow.notes.push(format!("上游代理：{}；失败不回退直连。", proxy.url)); }
        if matches!(flow.client_protocol.as_deref(), Some("HTTP/3")) { flow.notes.push(crate::multiplex::HEADER_NOTE.into()); }
        flow.duration_ms = start.elapsed().as_millis() as u64;
        self.store.insert(&flow)?;
        pending.flow = None;
        let _ = self.events.send(flow.clone());
        Ok(flow)
    }
    async fn apply_script(&self, flow:&mut Flow, script:&mut scripts::Scripts, revision:u64, after:bool)->Result<()> {
        let stage=if after {"响应后"} else {"请求前"};
        let result=self.scripts.execute(if after {script.after.clone()} else {script.before.clone()},flow,script.variables.clone(),script.modules.clone(),after).await.map_err(|e|anyhow::anyhow!("{stage}脚本：{e:#}"))?;
        for log in result.logs { flow.notes.push(format!("[{stage}脚本] {log}")); }
        if let Some(error)=result.error { anyhow::bail!("{stage}脚本：{error}"); }
        if after { if flow.original_response.is_none(){flow.original_response=flow.response.clone();} flow.response=result.response; }
        else { flow.original_request.get_or_insert_with(||flow.request.clone()); flow.request=result.request; }
        if flow.source=="capture" {self.scripts.commit_variables(revision,&script.variables,&result.variables)?;}
        else {flow.request.scripts.variables=result.variables.clone();}
        script.variables=result.variables;
        flow.notes.push(format!("{stage}脚本执行完成")); Ok(())
    }
    pub fn record_failure(&self, request: RequestDraft, raw_head: String, error: String) -> Result<()> {
        let flow = Flow { original_request: None, original_response: None, client_tls: None, id: uuid::Uuid::new_v4().to_string(), parent_id: None,
            started_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
            duration_ms: 0, source: "capture".into(), request, response: None, error: Some(error),
            raw_request_head_base64: Some(raw_head), notes: vec!["请求未转发；正文可能未完整接收。".into()], client_protocol: None };
        self.store.insert(&flow)?;
        let _ = self.events.send(flow);
        Ok(())
    }
}

mod httpcloak;
