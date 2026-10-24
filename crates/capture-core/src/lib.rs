pub mod grpc_client;
pub mod runtime;
pub mod config_migration;
pub use runtime::CaptureRuntime;
pub mod body_export;
pub mod body_read;
pub mod audit;
pub mod fault_rules;
pub mod response_rewrite;
mod body_spool;
mod body_base64;
mod capture_tls;
mod traffic_origin;
pub use traffic_origin::TunOriginRegistry;
mod traffic_process;
pub mod browser_profiles;
pub mod tls_config;
pub mod tun;
pub mod network;
pub mod decryption;
pub mod executions;
pub mod script_tools;
pub mod scripts;
pub mod plugins;
pub mod intercept;
pub mod blocking;
pub mod tls_details;
pub mod upstream;
mod socks_udp;
pub mod ca;
pub mod certificates;
pub mod system_proxy;
pub mod ca_management;
pub mod http1;
pub mod model;
pub mod proxy;
pub mod replay;
pub mod sse;
pub mod websocket;
pub mod websocket_client;
pub mod websocket_messages;
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
    pub certificates: Arc<certificates::Certificates>,
    pub grpc_clients: grpc_client::Sessions,
    pub system_proxy: system_proxy::SystemProxy,
    pub websocket_clients: websocket_client::Clients,
    pub tun: tun::TunState,
    pub executions: executions::Executions,
    pub scripts: scripts::ScriptState,
    pub plugins: plugins::Plugins,
    pub intercept: intercept::Interceptor,
    pub blocking: blocking::Blocking,
    pub fault_rules: fault_rules::FaultRules,
    pub response_rewrite: response_rewrite::ResponseRewrite,
    pub upstream: upstream::UpstreamState,
    pub store: Arc<store::Store>,
    pub ca: ca::CertificateAuthority,
    pub events: broadcast::Sender<Flow>,
    pub upstream_roots: rustls::RootCertStore,
    pub send_engines: transport::SendEngines,
}

struct PendingFlow<'a> { engine: &'a Engine, id: Option<String> }
impl Drop for PendingFlow<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let mut flow=match self.engine.store.get(&id) {
                Ok(Some(flow))=>flow,
                Ok(None)=>return, // Deleted captures must not be resurrected.
                Err(error)=>{tracing::error!(%id,%error,"Unable to finish cancelled capture");return;}
            };
            flow.notes.retain(|n| n != "SSE 接收中");
            flow.error = Some("请求已取消：代理停止或连接超时".into());
            flow.state = "cancelled".into();
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
        let certificates=certificates::Certificates::open(data_dir)?;
        let ca=certificates.authority(&data_dir.join("certificates"))?;
        Ok(Arc::new(Self { certificates,response_rewrite:response_rewrite::ResponseRewrite::open(store.clone())?, fault_rules:fault_rules::FaultRules::open(store.clone())?, blocking:blocking::Blocking::open(store.clone())?, plugins:plugins::Plugins::open(store.clone())?, grpc_clients:Default::default(), system_proxy: Default::default(), websocket_clients:Default::default(), tun: Default::default(), executions: Default::default(), scripts: scripts::ScriptState::open(store.clone())?, intercept: intercept::Interceptor::open(store.clone())?, upstream: upstream::UpstreamState::open_with_store(data_dir,store.clone())?, store,
            ca, events,
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
        let execution = self.executions.begin(self.executions.prepare()?)?;
        self.execute_inner(request, parent_id, source, raw_request_head_base64, client_protocol, client_tls, Some(execution)).await
    }
    pub async fn replay(&self, request: RequestDraft, parent_id: Option<String>, execution_id: Option<String>) -> Result<Flow> {
        let id = match execution_id { Some(id) => id, None => self.executions.prepare()? };
        let execution = self.executions.begin(id)?;
        self.execute_inner(request, parent_id, "replay", None, None, None, Some(execution)).await
    }
    async fn execute_inner(&self, mut request: RequestDraft, parent_id: Option<String>, source: &str,
        raw_request_head_base64: Option<String>, client_protocol: Option<String>, client_tls: Option<TlsDetails>,
        mut execution: Option<executions::Execution<'_>>) -> Result<Flow> {
        let started_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
        let start = Instant::now();
        let original_request = request.has_disabled_headers().then(|| request.clone());
        request.retain_enabled_headers();
        let mut flow = Flow { state: "sending".into(), websocket: None, original_request, original_response: None, client_tls, id: execution.as_ref().map(|e|e.id.clone()).unwrap_or_else(||uuid::Uuid::new_v4().to_string()), parent_id, started_at, duration_ms: 0,
            source: source.into(), request, response: None, error: None, raw_request_head_base64, notes: vec![], client_protocol, traffic_origin: crate::traffic_origin::current(source) };
        self.store.insert(&flow)?;
        if self.events.receiver_count()>0 { let _ = self.events.send(flow.clone()); }
        let mut pending = PendingFlow { engine: self, id: Some(flow.id.clone()) };
        let upstream = if source == "capture" { Ok(self.upstream.snapshot()) } else { flow.request.upstream_profile_id.as_deref().map(|id|self.upstream.profile_snapshot(id)).transpose() };
        let (mut script, revision) = if source == "capture" { self.scripts.snapshot() } else { (flow.request.scripts.clone(),0) };
        let rules = self.intercept.snapshot().config.rules;
        let plugins=plugins::matching(&self.plugins.list(),&flow);
        let operation = async {
            if self.blocking.apply(&mut flow) { return Ok(()); }
            let route=upstream.as_ref().map_err(|e|anyhow::anyhow!("{e:#}"))?.as_ref();
            script.validate()?;
            plugins::run(self,&plugins,&mut flow,false).await;
            if script.enabled && !script.before.trim().is_empty() && (source != "capture" || crate::intercept::matches_rules(&rules,&flow)) {
                self.apply_script(&mut flow, &mut script, revision, false).await?;
            }
            if let Some(d) = self.intercept.pause(&flow, "request").await? {
                if d.action == "modify" { flow.original_request.get_or_insert_with(||flow.request.clone()); flow.request = d.request.unwrap(); flow.notes.push("请求已通过拦截编辑；原始请求另行保留。".into()); }
                if d.action == "replace" { flow.response = d.response; flow.notes.push("请求未发往上游，已返回自定义响应。".into()); return Ok(()); }
            }
            flow.request.retain_enabled_headers();
            if self.blocking.apply(&mut flow) { return Ok(()); }
            if self.fault_rules.apply(&mut flow).await? { return Ok(()); }
            let (response, notes) = self.send_observed(&mut flow, route, (script.enabled && !script.after.trim().is_empty())||plugins.iter().any(|p|!p.after.trim().is_empty())).await?;
            flow.response = Some(response); flow.notes.extend(notes);
            plugins::run(self,&plugins,&mut flow,true).await;
            if script.enabled && !script.after.trim().is_empty() && (source != "capture" || crate::intercept::matches_rules(&rules,&flow)) {
                self.apply_script(&mut flow, &mut script, revision, true).await?;
            }
            // Saved response rewrites run automatically; manual response
            // breakpoints remain a separate final review/override step.
            self.response_rewrite.apply(&mut flow);
            if let Some(d) = if flow.notes.iter().any(|n| n == "SSE 接收中") { None } else { self.intercept.pause(&flow, "response").await? } {
                if d.action == "modify" || d.action == "replace" { if flow.original_response.is_none() { flow.original_response = flow.response.clone(); } flow.response = d.response; flow.notes.push("响应已通过拦截修改或替换；原始响应另行保留。".into()); }
            }
            Ok(())
        };
        let mut cancelled = false;
        let outcome: Result<()> = tokio::select! {
            biased;
            _ = async { match execution.as_mut() { Some(e) => e.cancelled().await, None => std::future::pending::<()>().await } } => { cancelled=true; Err(anyhow::anyhow!("请求已取消：用户取消发送")) },
            result = operation => result,
        };
        flow.request.retain_enabled_headers();
        if let Some(response) = &mut flow.response { response.retain_enabled_headers(); }
        if let Err(err) = outcome { flow.error = Some(format!("{err:#}")); if !flow.response.as_ref().is_some_and(|r|sse::is_sse(&r.headers)) { flow.response = None; } }
        if flow.state != "blocked" { if let Ok(Some(proxy)) = upstream { flow.notes.push(format!("上游代理：{}；失败不回退直连。", proxy.url)); } }
        if matches!(flow.client_protocol.as_deref(), Some("HTTP/3")) { flow.notes.push(crate::multiplex::HEADER_NOTE.into()); }
        flow.notes.retain(|n| n != "SSE 接收中");
        flow.duration_ms = start.elapsed().as_millis() as u64;
        flow.state = if cancelled { "cancelled" } else if flow.error.is_some() { "failed" } else if flow.state == "blocked" { "blocked" } else { "completed" }.into();
        self.store.insert(&flow)?;
        pending.id = None;
        if self.events.receiver_count()>0 { let _ = self.events.send(flow.clone()); }
        Ok(flow)
    }
    async fn send_observed(&self, flow: &mut Flow, route: Option<&upstream::UpstreamProxy>, after_script: bool) -> transport::SendResult {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let context = sse::Context { tx, started: Arc::new(std::sync::atomic::AtomicBool::new(false)) };
        let streaming_started = context.started.clone();
        let security=self.certificates.security(&url::Url::parse(&flow.request.url)?)?;
        let send = certificates::CURRENT.scope(security,sse::UPDATES.scope(context, self.send_engines.send_via(&flow.request, route)));
        tokio::pin!(send);
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
        let mut dirty = false;
        let mut completed: Option<transport::SendResult> = None;
        loop {
            if completed.is_some() && rx.is_empty() {
                let result=completed.take().unwrap();
                return result.and_then(|(mut response,notes)| {
                    if streaming_started.load(std::sync::atomic::Ordering::Relaxed) {
                        let live=flow.response.as_mut().ok_or_else(||anyhow::anyhow!("SSE response headers were not recorded"))?;
                        response.body_base64=std::mem::take(&mut live.body_base64);
                    }
                    Ok((response,notes))
                });
            }
            tokio::select! {
                biased;
                Some(event) = rx.recv() => {
                    match &event {
                        sse::Event::Head(response) => {
                            let mut check = flow.clone(); check.response = Some(response.clone());
                            let config = self.intercept.snapshot().config;
                            anyhow::ensure!(!after_script && !(config.response && (config.scope == "all" || config.scope == flow.source) && crate::intercept::matches_rules(&config.rules,&check)), "SSE streaming cannot use response scripts or response interception; disable the matching response rule/script and retry");
                            flow.response = Some(response.clone());
                            flow.state = "receiving".into();
                            flow.notes.push("SSE 接收中".into());
                            flow.notes.push("SSE 实时流：最多 300 秒 / 8 MiB；停止或超限保留已接收正文。".into());
                        }
                        sse::Event::Data(data) => {
                            let response=flow.response.as_mut().ok_or_else(||anyhow::anyhow!("SSE data arrived before response headers"))?;
                            body_base64::append_complete(&mut response.body_base64,data)?;
                        },
                    }
                    dirty = true;
                    sse::downstream(event).await?;
                }
                result = &mut send, if completed.is_none() => completed = Some(result),
                _ = tick.tick(), if dirty => {
                    self.store.insert_streaming(flow,"response")?;
                    if self.events.receiver_count()>0 { let _ = self.events.send(flow.clone()); }
                    dirty = false;
                }
            }
        }
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
        let flow = Flow { state: "failed".into(), websocket: None, original_request: None, original_response: None, client_tls: None, id: uuid::Uuid::new_v4().to_string(), parent_id: None,
            started_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
            duration_ms: 0, source: "capture".into(), request, response: None, error: Some(error),
            raw_request_head_base64: Some(raw_head), notes: vec!["请求未转发；正文可能未完整接收。".into()], client_protocol: None, traffic_origin: crate::traffic_origin::current("capture") };
        self.store.insert(&flow)?;
        let _ = self.events.send(flow);
        Ok(())
    }
}

mod httpcloak;
