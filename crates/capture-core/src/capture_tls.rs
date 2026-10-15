//! Eager upstream TLS negotiation for capture connections. The first HTTP
//! request reuses this verified socket; later requests keep the selected ALPN.
use crate::{http1::*, model::*, replay::Stream, upstream::UpstreamProxy};
use anyhow::{ensure, Context, Result};
use std::{future::Future, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tokio_rustls::{client::TlsStream, TlsConnector};

pub(crate) struct CaptureTls {
    origin: url::Url,
    route: Option<UpstreamProxy>,
    alpn: Option<Vec<u8>>,
    first: Mutex<Option<TlsStream<Stream>>>,
    created: Instant,
}
tokio::task_local! { static CURRENT: Arc<CaptureTls>; }

struct PreparedConnectionGuard(Arc<CaptureTls>);
impl Drop for PreparedConnectionGuard {
    fn drop(&mut self) { self.0.discard_first(); }
}

impl CaptureTls {
    pub(crate) async fn prepare(engine: &crate::Engine, origin: &url::Url, offers: Vec<Vec<u8>>) -> Result<Arc<Self>> {
        let route = engine.upstream.snapshot();
        let mut config = crate::tls_config::config(&TlsProfile::default(), &engine.upstream_roots, "auto")?;
        config.alpn_protocols = offers;
        let tls = tokio::time::timeout(Duration::from_secs(10), dial(origin, route.as_ref(), config))
            .await.context("Upstream TLS negotiation timed out")??;
        let alpn = tls.get_ref().1.alpn_protocol().map(Vec::from);
        ensure!(alpn.as_deref().is_none_or(|p| p == b"h2" || p == b"http/1.1"), "Unsupported upstream ALPN");
        Ok(Arc::new(Self { origin: origin.clone(), route, alpn, first: Mutex::new(Some(tls)), created: Instant::now() }))
    }
    pub(crate) fn downstream_alpn(&self) -> Vec<Vec<u8>> { self.alpn.iter().cloned().collect() }
    fn matches(&self, request: &RequestDraft, url: &url::Url, route: Option<&UpstreamProxy>) -> bool {
        let tls = &request.tls;
        let same_route = match (self.route.as_ref(), route) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_route(b),
            _ => false,
        };
        let protocol_compatible = request.engine == "auto" || (request.engine == "native" && self.alpn.as_deref() != Some(b"h2"));
        same_route && protocol_compatible && url.scheme() == self.origin.scheme()
            && url.host() == self.origin.host() && url.port_or_known_default() == self.origin.port_or_known_default()
            && tls.preset == "native" && matches!(tls.version.as_deref(), None | Some("auto"))
            && tls.cipher_list.is_none() && tls.curves_list.is_none() && tls.sigalgs_list.is_none()
            && tls.grease.is_none() && tls.permute_extensions.is_none() && tls.client_hello_hex.is_none()
            && tls.browser_version.is_none()
    }
    fn discard_first(&self) { self.first.lock().unwrap().take(); }
}

pub(crate) fn scope<T>(context: Option<Arc<CaptureTls>>, work: impl Future<Output = T>) -> impl Future<Output = T> {
    // Keep the transport future off the Windows thread stack, including while
    // moving it into the task-local scope.
    let work = Box::pin(work);
    async move {
        if let Some(context) = context {
            // Replacements, scripts or cancelled streams may never consume it.
            let _cleanup = PreparedConnectionGuard(context.clone());
            CURRENT.scope(context, work).await
        } else { work.await }
    }
}

pub(crate) async fn connect(request: &RequestDraft, url: &url::Url, route: Option<&UpstreamProxy>, mut config: rustls::ClientConfig) -> Result<TlsStream<Stream>> {
    let context = CURRENT.try_with(Clone::clone).ok();
    if let Some(context) = context {
        if context.matches(request, url, route) {
            config.alpn_protocols = context.downstream_alpn();
            let first = context.first.lock().unwrap().take();
            // A long breakpoint may outlive the origin's idle timeout.
            if context.created.elapsed() < Duration::from_secs(5) {
                if let Some(first) = first { return Ok(first); }
            }
        } else { context.discard_first(); }
    }
    Box::pin(dial(url, route, config)).await
}

async fn dial(url: &url::Url, route: Option<&UpstreamProxy>, config: rustls::ClientConfig) -> Result<TlsStream<Stream>> {
    let connection = crate::upstream::connect(url, route).await?;
    let name = rustls::pki_types::ServerName::try_from(hostname(url))?;
    TlsConnector::from(Arc::new(config)).connect(name, connection.stream).await.map_err(Into::into)
}
