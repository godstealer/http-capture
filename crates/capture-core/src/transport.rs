//! Outbound transport boundary. Capture, persistence and UI do not depend on a
//! particular TLS client. Future process adapters implement this same contract.
use crate::model::RequestDraft;
use anyhow::{bail, Result};
use std::sync::Arc;

pub use transport_api::{SendFuture, SendResult};
pub use transport_api::SendEngine as EngineContract;
/// Core binds the transport contract to its script-bearing draft and upstream settings.
pub trait SendEngine: EngineContract<RequestDraft,crate::upstream::UpstreamProxy> {}
impl<T:EngineContract<RequestDraft,crate::upstream::UpstreamProxy>> SendEngine for T {}

pub struct NativeEngine { roots: rustls::RootCertStore }
impl NativeEngine {
    pub fn new(roots: rustls::RootCertStore) -> Self { Self { roots } }
}
impl EngineContract<RequestDraft,crate::upstream::UpstreamProxy> for NativeEngine {
    fn send_via<'a>(&'a self, draft: &'a RequestDraft, proxy: Option<&'a crate::upstream::UpstreamProxy>) -> SendFuture<'a> {
        if proxy.is_none() { return self.send(draft); }
        Box::pin(async move { crate::replay::native_via(draft, &self.roots, proxy).await })
    }
    fn id(&self) -> &'static str { "native" }
    fn profiles(&self) -> Vec<String> { vec!["native".into()] }
    fn send<'a>(&'a self, draft: &'a RequestDraft) -> SendFuture<'a> {
        Box::pin(async move {
            if draft.tls.preset != "native" { bail!("native engine requires the native TLS profile"); }
            crate::replay::native(draft, &self.roots).await
        })
    }
}

pub struct WreqEngine;
pub struct AutoEngine { roots: rustls::RootCertStore }
impl EngineContract<RequestDraft,crate::upstream::UpstreamProxy> for AutoEngine {
    fn send_via<'a>(&'a self, draft: &'a RequestDraft, proxy: Option<&'a crate::upstream::UpstreamProxy>) -> SendFuture<'a> {
        if proxy.is_none() { return self.send(draft); }
        Box::pin(async move { crate::replay::native_via(draft, &self.roots, proxy).await })
    }
    fn id(&self) -> &'static str { "auto" }
    fn profiles(&self) -> Vec<String> { vec!["native".into()] }
    fn send<'a>(&'a self, draft: &'a RequestDraft) -> SendFuture<'a> {
        Box::pin(async move { crate::replay::native(draft, &self.roots).await })
    }
}
pub struct MultiplexEngine { pub protocol: &'static str, pub roots: rustls::RootCertStore }
impl EngineContract<RequestDraft,crate::upstream::UpstreamProxy> for MultiplexEngine {
    fn send_via<'a>(&'a self, draft: &'a RequestDraft, proxy: Option<&'a crate::upstream::UpstreamProxy>) -> SendFuture<'a> {
        if proxy.is_none() { return self.send(draft); }
        if crate::sse::active() && self.protocol == "h2" { return Box::pin(crate::multiplex::h2_send_via(draft, &self.roots, proxy)); }
        Box::pin(async move { tokio::time::timeout(std::time::Duration::from_secs(45), async {
            if self.protocol == "h2" { crate::multiplex::h2_send_via(draft, &self.roots, proxy).await }
            else { crate::multiplex::h3_send_via(draft, &self.roots, proxy).await }
        }).await.map_err(|_| anyhow::anyhow!("Protocol request timed out"))? })
    }
    fn id(&self) -> &'static str { self.protocol }
    fn profiles(&self) -> Vec<String> { vec!["native".into()] }
    fn send<'a>(&'a self, draft: &'a RequestDraft) -> SendFuture<'a> {
        Box::pin(async move {
            let work = async { if self.protocol == "h2" { crate::multiplex::h2_send(draft, &self.roots).await } else { crate::multiplex::h3_send(draft, &self.roots).await } };
            if crate::sse::active() && self.protocol == "h2" { return work.await; }
            tokio::time::timeout(std::time::Duration::from_secs(45), work).await.map_err(|_| anyhow::anyhow!("Protocol request timed out"))?
        })
    }
}
impl EngineContract<RequestDraft,crate::upstream::UpstreamProxy> for WreqEngine {
    fn send_via<'a>(&'a self,draft:&'a RequestDraft,proxy:Option<&'a crate::upstream::UpstreamProxy>)->SendFuture<'a>{Box::pin(async move {crate::replay::browser_via(draft,proxy).await})}
    fn browser_versions(&self) -> std::collections::BTreeMap<String,Vec<u16>> { crate::browser_profiles::versions() }
    fn id(&self) -> &'static str { "wreq" }
    fn profiles(&self) -> Vec<String> { vec!["chrome".into(), "firefox".into()] }
    fn send<'a>(&'a self, draft: &'a RequestDraft) -> SendFuture<'a> {
        Box::pin(async move {
            if !matches!(draft.tls.preset.as_str(), "chrome" | "firefox") {
                bail!("wreq engine requires a Chrome or Firefox TLS profile");
            }
            crate::replay::browser_via(draft,None).await
        })
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    #[serde(default)]
    pub browser_versions: std::collections::BTreeMap<String, Vec<u16>>,
    pub id: String,
    pub available: bool,
    pub profiles: Vec<String>,
    pub reason: Option<String>,
}

/// Per-request routing by engine ID, independent of TLS profile. Registered
/// implementations coexist; unavailable engines never silently fall back.
pub struct SendEngines {
    entries: std::collections::BTreeMap<String, (EngineInfo, Option<Arc<dyn SendEngine>>)>,
}
impl SendEngines {
    pub fn empty() -> Self { Self { entries: Default::default() } }
    pub fn register(&mut self, engine: Arc<dyn SendEngine>) -> Result<()> {
        let id = engine.id().to_owned();
        if self.entries.get(&id).is_some_and(|(_, e)| e.is_some()) { bail!("Engine already registered: {id}"); }
        let info = EngineInfo { browser_versions: engine.browser_versions(), id: id.clone(), available: true, profiles: engine.profiles(), reason: None };
        self.entries.insert(id, (info, Some(engine)));
        Ok(())
    }
    pub fn with_roots(roots: rustls::RootCertStore) -> Self {
        let mut engines = Self::empty();
        engines.register(Arc::new(AutoEngine { roots: roots.clone() })).expect("unique auto engine");
        engines.register(Arc::new(NativeEngine::new(roots.clone()))).expect("unique native engine");
        for protocol in ["h2", "h3"] { engines.register(Arc::new(MultiplexEngine { protocol, roots: roots.clone() })).expect("unique protocol engine"); }
        if cfg!(feature = "browser-replay") {
            engines.register(Arc::new(WreqEngine)).expect("unique wreq engine");
        } else {
            engines.unavailable("wreq", &["chrome", "firefox"], "当前构建未启用 browser-replay");
        }
        match crate::httpcloak::HttpcloakEngine::discover() {
            Ok(engine)=>engines.register(Arc::new(engine)).expect("unique httpcloak engine"),
            Err(_)=>engines.unavailable("httpcloak", &["chrome", "firefox"], "Build or install the httpcloak helper and capability manifest"),
        }
        engines
    }
    fn unavailable(&mut self, id: &str, profiles: &[&str], reason: &str) {
        self.entries.insert(id.into(), (EngineInfo { browser_versions: Default::default(), id: id.into(), available: false,
            profiles: profiles.iter().map(|p| (*p).into()).collect(), reason: Some(reason.into()) }, None));
    }
    pub fn list(&self) -> Vec<EngineInfo> {
        self.entries.values().map(|(info, _)| info.clone()).collect()
    }
    pub async fn send(&self, draft: &RequestDraft) -> SendResult { self.send_via(draft, None).await }
    pub async fn send_via(&self, draft: &RequestDraft, proxy: Option<&crate::upstream::UpstreamProxy>) -> SendResult {
        use anyhow::Context;
        let (info, engine) = self.entries.get(&draft.engine)
            .with_context(|| format!("Unknown send engine: {}", draft.engine))?;
        let engine = engine.as_ref().with_context(|| format!("{} unavailable: {}", info.id, info.reason.as_deref().unwrap_or("not registered")))?;
        if !info.profiles.contains(&draft.tls.preset) {
            bail!("{} does not support TLS profile {}", info.id, draft.tls.preset);
        }
        let send = engine.send_via(draft, proxy);
        tokio::pin!(send);
        let result = tokio::select! {
            result = &mut send => result,
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                anyhow::ensure!(crate::sse::started(), "Request timed out after 30 seconds");
                tokio::time::timeout(std::time::Duration::from_secs(270), &mut send).await.context("SSE session reached 300 second limit")?
            }
        };
        result.with_context(|| format!("{} send engine", engine.id()))
    }
}
