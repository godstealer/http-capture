//! Capture connection metadata is scoped per connection and copied into spawned streams.
use crate::model::TrafficOrigin;
use std::{future::Future, net::SocketAddr};
use tokio::sync::Semaphore;

tokio::task_local! { static CURRENT: Option<TrafficOrigin>; }
static CLASSIFIERS: Semaphore = Semaphore::const_new(4);

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
        process: None, process_status: "unknown".into(),
    };
    let local_peer = if peer.ip().to_canonical().is_loopback() { true } else {
        let Ok(permit) = CLASSIFIERS.try_acquire() else { return origin; };
        let ip = peer.ip();
        let work = tokio::task::spawn_blocking(move || { let _permit = permit; crate::traffic_process::is_local(ip) });
        let Ok(Ok(value)) = tokio::time::timeout(std::time::Duration::from_secs(1), work).await else { return origin; };
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
    TrafficOrigin { entry: "tun".into(), client_address: None, process: None, process_status: "tun-unavailable".into() }
}

pub(crate) fn quic(peer: SocketAddr, redirected: bool) -> TrafficOrigin {
    if redirected { return tun(); }
    TrafficOrigin { entry: "quic-reverse".into(), client_address: Some(peer.to_string()), process: None, process_status: "transport-unsupported".into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Flow;

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
    }
}
