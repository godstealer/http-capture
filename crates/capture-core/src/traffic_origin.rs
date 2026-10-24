//! Capture connection metadata is scoped per connection and copied into spawned streams.
use crate::model::TrafficOrigin;
use std::{collections::HashMap, future::Future, net::SocketAddr, sync::{Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use tokio::sync::{Notify, Semaphore};

tokio::task_local! { static CURRENT: Option<TrafficOrigin>; }
static CLASSIFIERS: Semaphore = Semaphore::const_new(4);

/// Relates the TUN helper's outgoing loopback socket to one original VPN flow.
/// Each bridge owns its registry, so stopping/restarting the VPN cannot reuse a
/// prior flow's identity when the OS later reuses the same ephemeral port.
#[derive(Default)]
pub struct TunOriginRegistry {
    enabled: AtomicBool,
    pending: Mutex<HashMap<SocketAddr, (TrafficOrigin, Instant)>>,
    changed: Notify,
}

impl TunOriginRegistry {
    pub fn enable(&self) { self.enabled.store(true, Ordering::Release); }

    pub fn register(&self, peer: SocketAddr, origin: TrafficOrigin) {
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|_, (_, when)| when.elapsed() < Duration::from_secs(30));
        if pending.len() >= 2048 { pending.clear(); }
        pending.insert(peer, (origin, Instant::now()));
        drop(pending);
        self.changed.notify_waiters();
    }

    pub(crate) async fn take(&self, peer: SocketAddr) -> Option<TrafficOrigin> {
        if !self.enabled.load(Ordering::Acquire) { return None; }
        tokio::time::timeout(Duration::from_millis(250), async {
            loop {
                let notified = self.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if let Some((origin, _)) = self.pending.lock().unwrap().remove(&peer) {
                    return origin;
                }
                notified.await;
            }
        }).await.ok()
    }
}

pub(crate) fn current(source: &str) -> Option<TrafficOrigin> {
    if source != "capture" { return None; }
    CURRENT.try_with(Clone::clone).ok().flatten()
}

pub(crate) fn scope<F: Future>(origin: Option<TrafficOrigin>, work: F) -> impl Future<Output = F::Output> {
    // Protocol futures are large in debug builds; keep the context wrapper small
    // rather than multiplying their inline size through TLS and stream scopes.
    CURRENT.scope(origin, Box::pin(work))
}

pub(crate) async fn tcp(peer: SocketAddr, local: SocketAddr, transparent: bool) -> TrafficOrigin {
    // The TUN helper's loopback socket identifies the helper, not the original application.
    if transparent { return tun(); }
    let mut origin = TrafficOrigin {
        entry: "explicit-proxy".into(), client_address: Some(peer.to_string()),
        process: None, application: None, process_status: "unknown".into(),
    };
    let local_peer = if peer.ip().to_canonical().is_loopback() { true } else {
        let ip = peer.ip();
        let Some(value) = crate::traffic_process::run_bounded(&CLASSIFIERS, std::time::Duration::from_secs(1),
            move || Some(crate::traffic_process::is_local(ip))).await else { return origin; };
        value
    };
    if !local_peer { origin.process_status = "remote".into(); }
    else if !crate::traffic_process::SUPPORTED { origin.process_status = "unsupported".into(); }
    else if let Some(process) = crate::traffic_process::lookup(peer, local).await {
        origin.process = Some(process); origin.process_status = "identified".into();
    }
    origin
}

pub(crate) fn tun() -> TrafficOrigin {
    TrafficOrigin { entry: "tun".into(), client_address: None, process: None, application: None, process_status: "tun-unavailable".into() }
}

pub(crate) fn quic(peer: SocketAddr, redirected: bool) -> TrafficOrigin {
    if redirected { return tun(); }
    TrafficOrigin { entry: "quic-reverse".into(), client_address: Some(peer.to_string()), process: None, application: None, process_status: "transport-unsupported".into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Flow;
    use std::sync::Arc;

    #[tokio::test]
    async fn tun_registry_matches_only_the_helper_socket_and_consumes_once() {
        let registry = Arc::new(TunOriginRegistry::default());
        registry.enable();
        let peer: SocketAddr = "127.0.0.1:42001".parse().unwrap();
        let other: SocketAddr = "127.0.0.1:42002".parse().unwrap();
        let pending = registry.clone();
        let waiter = tokio::spawn(async move { pending.take(peer).await });
        tokio::task::yield_now().await;
        let mut origin = tun();
        origin.client_address = Some("10.111.0.2:50000".into());
        origin.application = Some(crate::model::ApplicationInfo {
            uid: 12345, packages: vec!["example.app".into()], label: Some("Example".into()),
        });
        registry.register(other, tun());
        registry.register(peer, origin.clone());
        assert_eq!(waiter.await.unwrap(), Some(origin));
        assert!(registry.pending.lock().unwrap().contains_key(&other));
        assert!(!registry.pending.lock().unwrap().contains_key(&peer));
    }

    #[tokio::test]
    async fn task_context_is_isolated_and_never_attached_to_replay() {
        let a = tun();
        let b = quic("127.0.0.1:41234".parse().unwrap(), false);
        let (left, right) = tokio::join!(scope(Some(a.clone()), async {
            tokio::task::yield_now().await;
            assert!(current("replay").is_none());
            let child = current("capture");
            tokio::spawn(scope(child, async { current("capture") })).await.unwrap()
        }), scope(Some(b.clone()), async {
            tokio::task::yield_now().await; current("capture")
        }));
        assert_eq!(left, Some(a)); assert_eq!(right, Some(b));
        assert!(current("capture").is_none());
    }

    #[test]
    fn old_records_and_optional_metadata_round_trip() {
        let mut flow: Flow = serde_json::from_value(serde_json::json!({
            "id":"old", "source":"capture", "startedAt":0, "durationMs":0,
            "request":{"method":"GET","url":"http://example.invalid/","headers":[],"bodyBase64":""},
            "response":null,"error":null,"notes":[]
        })).unwrap();
        assert!(flow.traffic_origin.is_none());
        assert!(serde_json::to_value(&flow).unwrap().get("trafficOrigin").is_none());
        flow.traffic_origin = Some(tun());
        let loaded: Flow = serde_json::from_value(serde_json::to_value(&flow).unwrap()).unwrap();
        assert_eq!(loaded.traffic_origin, flow.traffic_origin);
        let old = serde_json::json!({"entry":"tun","clientAddress":null,"process":null,
            "processStatus":"tun-unavailable"});
        let origin: TrafficOrigin = serde_json::from_value(old).unwrap();
        assert!(origin.application.is_none());
    }
}
