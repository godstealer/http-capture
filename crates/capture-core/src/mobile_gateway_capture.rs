//! Best-effort observation never changes forwarding bytes or IO outcomes.
use crate::{
    mobile_http_observer::{HeadObserver, RequestHead, ResponseHead},
    model::{CapturedResponse, Flow},
    Engine,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
    task::{Context, Poll},
    time::Instant,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{mpsc, oneshot, watch, Semaphore},
    time::Instant as Deadline,
};

// Detached cancellation cleanup cannot accumulate unbounded database writers.
// When this budget is busy, the authenticated tunnel continues without observation.
static WRITERS: OnceLock<Arc<Semaphore>> = OnceLock::new();
const MAX_WRITERS: usize = 64;

#[derive(Clone, Copy)]
enum Direction {
    Request,
    Response,
}
enum Head {
    Request(RequestHead),
    Response(ResponseHead),
}
struct Observation {
    parser: Option<HeadObserver>,
    events: mpsc::Sender<Head>,
}
struct ObservedIo<T> {
    inner: T,
    direction: Direction,
    capture: Option<Arc<Mutex<Observation>>>,
}

impl<T: AsyncRead + Unpin> AsyncRead for ObservedIo<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let space = buf.remaining();
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        match &result {
            Poll::Ready(Ok(())) if buf.filled().len() > before => {
                let mut finished = false;
                if let Some(capture) = &this.capture {
                    if let Ok(mut state) = capture.lock() {
                        if state.events.is_closed() {
                            state.parser = None;
                        }
                        let event = state
                            .parser
                            .as_mut()
                            .and_then(|parser| match this.direction {
                                Direction::Request => parser
                                    .observe_request(&buf.filled()[before..])
                                    .map(Head::Request),
                                Direction::Response => parser
                                    .observe_response(&buf.filled()[before..])
                                    .map(Head::Response),
                            });
                        if let Some(event) = event {
                            if state.events.try_send(event).is_err() {
                                state.parser = None;
                            }
                        }
                        finished =
                            state
                                .parser
                                .as_ref()
                                .is_none_or(|parser| match this.direction {
                                    Direction::Request => parser.request_finished(),
                                    Direction::Response => parser.response_finished(),
                                });
                    } else {
                        finished = true;
                    }
                }
                if finished {
                    this.capture = None;
                }
            }
            // A zero-capacity successful read is not an EOF observation.
            Poll::Ready(Ok(())) if space > 0 => {
                this.capture = None;
            }
            Poll::Ready(Err(_)) => {
                this.capture = None;
            }
            _ => {}
        }
        result
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for ObservedIo<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bytes)
    }
}

struct Finished {
    duration_ms: u64,
    outcome: Result<(u64, u64), String>,
}

async fn save(engine: &Arc<Engine>, flow: &Flow, initial: bool) -> bool {
    let owner = engine.clone();
    let snapshot = flow.clone();
    match tokio::task::spawn_blocking(move || owner.store.insert_observation(&snapshot, initial))
        .await
    {
        Ok(Ok(true)) => {
            let _ = engine.events.send(flow.clone());
            true
        }
        Ok(Ok(false)) => false,
        result => {
            tracing::debug!(?result, "Mobile tunnel observation persistence stopped");
            false
        }
    }
}

fn apply_request(flow: &mut Flow, request: RequestHead) {
    // The route is the already authenticated tunnel destination. Host is observed
    // as a header and never controls forwarding or replaces that destination.
    let candidate = if request.target.starts_with('/') {
        flow.request
            .url
            .strip_prefix("tcp://")
            .map(|authority| format!("http://{authority}{}", request.target))
    } else {
        Some(request.target)
    };
    if let Some(url) = candidate.filter(|url| crate::http1::validate_url(url).is_ok()) {
        flow.request.url = url;
    }
    flow.request.method = request.method;
    flow.client_protocol = Some(request.version);
    flow.request.headers = request.headers;
    flow.raw_request_head_base64 = Some(STANDARD.encode(request.raw));
}

async fn record(
    engine: Arc<Engine>,
    mut flow: Flow,
    mut heads: mpsc::Receiver<Head>,
    finished: oneshot::Receiver<Finished>,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    if !save(&engine, &flow, true).await {
        return;
    }
    // There are at most two head events. Drain them before the final lifecycle;
    // a separately selected finish message must never discard already read heads.
    while let Some(head) = heads.recv().await {
        match head {
            Head::Request(request) => apply_request(&mut flow, request),
            Head::Response(response) => {
                flow.response = Some(CapturedResponse {
                    status: response.status,
                    version: response.version,
                    headers: response.headers,
                    raw_head_base64: Some(STANDARD.encode(response.raw)),
                    body_base64: Default::default(),
                    trailers: vec![],
                    upstream_tls: None,
                    tls_version: None,
                    sent_request_headers: None,
                });
            }
        }
        if !save(&engine, &flow, false).await {
            return;
        }
    }
    match finished.await {
        Ok(finished) => {
            flow.duration_ms = finished.duration_ms;
            match finished.outcome {
                Ok((sent, received)) => {
                    flow.state = "completed".into();
                    flow.notes
                        .push(format!("Tunnel bytes: client={sent}, server={received}"));
                }
                Err(error) => {
                    flow.state = "failed".into();
                    flow.error = Some(error);
                }
            }
        }
        Err(_) => {
            flow.state = "interrupted".into();
            flow.notes
                .push("Tunnel task ended before its final IO result was recorded.".into());
        }
    }
    let _ = save(&engine, &flow, false).await;
}

pub(super) async fn copy_observed_tunnel<C, U>(
    client: C,
    upstream: U,
    recording: Option<(Arc<Engine>, Flow)>,
    generation: &mut watch::Receiver<u64>,
    generation_at_start: u64,
    expires: Deadline,
) -> anyhow::Result<(u64, u64)>
where
    C: AsyncRead + AsyncWrite + Unpin,
    U: AsyncRead + AsyncWrite + Unpin,
{
    let started = Instant::now();
    let (capture, finish, writer) = recording
        .and_then(|(engine, flow)| {
            let permit = WRITERS
                .get_or_init(|| Arc::new(Semaphore::new(MAX_WRITERS)))
                .clone()
                .try_acquire_owned()
                .ok()?;
            let (events, heads) = mpsc::channel(2);
            let (finish, finished) = oneshot::channel();
            let capture = Arc::new(Mutex::new(Observation {
                parser: Some(HeadObserver::default()),
                events,
            }));
            let writer = tokio::spawn(record(engine, flow, heads, finished, permit));
            Some((Some(capture), Some(finish), Some(writer)))
        })
        .unwrap_or((None, None, None));
    let mut client = ObservedIo {
        inner: client,
        direction: Direction::Request,
        capture: capture.clone(),
    };
    let mut upstream = ObservedIo {
        inner: upstream,
        direction: Direction::Response,
        capture,
    };
    let transferred = tokio::select! {
        biased;
        _ = generation.changed() => Err(anyhow::anyhow!(if *generation.borrow() != generation_at_start { "Mobile session was revoked" } else { "Mobile gateway stopped" })),
        _ = tokio::time::sleep_until(expires) => Err(anyhow::anyhow!("Mobile session expired")),
        result = tokio::io::copy_bidirectional(&mut client, &mut upstream) => result.map_err(anyhow::Error::from),
    };
    // Close revoked/completed sockets before waiting for observation persistence.
    drop(client);
    drop(upstream);
    if let Some(finish) = finish {
        let outcome = transferred
            .as_ref()
            .map(|counts| *counts)
            .map_err(ToString::to_string);
        let _ = finish.send(Finished {
            duration_ms: started.elapsed().as_millis() as u64,
            outcome,
        });
    }
    if let Some(writer) = writer {
        if let Err(error) = writer.await {
            tracing::debug!(%error, "Mobile tunnel observation task ended");
        }
    }
    transferred
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::poll_fn;

    #[tokio::test]
    async fn tap_observes_only_new_readbuf_bytes_and_zero_capacity_is_not_eof() {
        let request = b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n";
        let (events, mut heads) = mpsc::channel(2);
        let state = Arc::new(Mutex::new(Observation {
            parser: Some(HeadObserver::default()),
            events,
        }));
        let mut tap = ObservedIo {
            inner: request.as_slice(),
            direction: Direction::Request,
            capture: Some(state),
        };
        let mut empty = [];
        poll_fn(|cx| Pin::new(&mut tap).poll_read(cx, &mut ReadBuf::new(&mut empty)))
            .await
            .unwrap();
        assert!(tap.capture.is_some());
        let mut storage = [0; 256];
        let mut buffer = ReadBuf::new(&mut storage);
        buffer.put_slice(b"not part of the stream");
        poll_fn(|cx| Pin::new(&mut tap).poll_read(cx, &mut buffer))
            .await
            .unwrap();
        let Some(Head::Request(head)) = heads.recv().await else {
            panic!("request head missing")
        };
        assert_eq!(head.raw, request);
        assert_eq!(&buffer.filled()[b"not part of the stream".len()..], request);
    }

    #[test]
    fn observed_targets_preserve_tunnel_authority_and_reject_invalid_display_urls() {
        for (authority, target, expected) in [
            (
                "example.test:80",
                "/path?q=1",
                "http://example.test:80/path?q=1",
            ),
            ("[::1]:8080", "/ipv6", "http://[::1]:8080/ipv6"),
            (
                "example.test:80",
                "//other.test/path",
                "http://example.test:80//other.test/path",
            ),
            (
                "example.test:80",
                "/path%23part",
                "http://example.test:80/path%23part",
            ),
            ("example.test:80", "/path#fragment", "tcp://example.test:80"),
            ("example.test:80", "*", "tcp://example.test:80"),
            (
                "example.test:80",
                "https://other.test/path",
                "https://other.test/path",
            ),
            (
                "example.test:80",
                "http://user:secret@other.test/",
                "tcp://example.test:80",
            ),
        ] {
            let mut flow = super::super::mobile_flow(
                "127.0.0.1:1234".parse().unwrap(),
                "CONNECT".into(),
                format!("tcp://{authority}"),
                None,
            )
            .unwrap();
            let raw = format!("GET {target} HTTP/1.1\r\nHost: unrelated.test\r\n\r\n").into_bytes();
            let request = HeadObserver::default().observe_request(&raw).unwrap();
            apply_request(&mut flow, request);
            assert_eq!(flow.request.url, expected, "target {target}");
            assert_eq!(flow.request.headers[0].value, "unrelated.test");
            assert_eq!(
                STANDARD
                    .decode(flow.raw_request_head_base64.as_ref().unwrap())
                    .unwrap(),
                raw
            );
            assert!(flow.ensure_body_recorded("request").is_err());
        }
    }

    #[test]
    fn observation_updates_cannot_recreate_cleared_or_deleted_records() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).unwrap();
        for deleted in [false, true] {
            let flow = super::super::mobile_flow(
                "127.0.0.1:1234".parse().unwrap(),
                "CONNECT".into(),
                "tcp://example.test:80".into(),
                None,
            )
            .unwrap();
            assert!(!engine.store.insert_observation(&flow, false).unwrap());
            assert!(engine.store.insert_observation(&flow, true).unwrap());
            assert!(!engine.store.insert_observation(&flow, true).unwrap());
            if deleted {
                engine.store.delete_flows(vec![flow.id.clone()]).unwrap();
            } else {
                engine.store.clear().unwrap();
            }
            assert!(!engine.store.insert_observation(&flow, false).unwrap());
            assert!(engine.store.get(&flow.id).unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn aborted_copy_drops_channels_and_finishes_observation_as_interrupted() {
        use tokio::io::AsyncReadExt;
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).unwrap();
        let flow = super::super::mobile_flow(
            "127.0.0.1:1234".parse().unwrap(),
            "CONNECT".into(),
            "tcp://example.test:80".into(),
            None,
        )
        .unwrap();
        let id = flow.id.clone();
        let (mut client, inward) = tokio::io::duplex(1024);
        let (mut server, outward) = tokio::io::duplex(1024);
        let (_generation, mut changed) = watch::channel(0);
        let owner = engine.clone();
        let task = tokio::spawn(async move {
            copy_observed_tunnel(
                inward,
                outward,
                Some((owner, flow)),
                &mut changed,
                0,
                Deadline::now() + std::time::Duration::from_secs(60),
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while engine.store.get(&id).unwrap().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        task.abort();
        let _ = task.await;
        let mut byte = [0];
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), server.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if engine.store.get(&id).unwrap().unwrap().state == "interrupted" {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn tap_preserves_pending_and_error_without_observing_failed_bytes() {
        struct PendingThenError(bool);
        impl AsyncRead for PendingThenError {
            fn poll_read(
                mut self: Pin<&mut Self>,
                _: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                if !self.0 {
                    self.0 = true;
                    Poll::Pending
                } else {
                    buf.put_slice(b"GET / HTTP/1.1\r\n\r\n");
                    Poll::Ready(Err(std::io::Error::from(
                        std::io::ErrorKind::ConnectionReset,
                    )))
                }
            }
        }
        let (events, mut heads) = mpsc::channel(2);
        let mut tap = ObservedIo {
            inner: PendingThenError(false),
            direction: Direction::Request,
            capture: Some(Arc::new(Mutex::new(Observation {
                parser: Some(HeadObserver::default()),
                events,
            }))),
        };
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut storage = [0; 64];
        let mut buffer = ReadBuf::new(&mut storage);
        assert!(Pin::new(&mut tap)
            .poll_read(&mut cx, &mut buffer)
            .is_pending());
        assert!(buffer.filled().is_empty());
        let Poll::Ready(Err(error)) = Pin::new(&mut tap).poll_read(&mut cx, &mut buffer) else {
            panic!("expected IO error")
        };
        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
        assert_eq!(
            heads.try_recv().err().unwrap(),
            mpsc::error::TryRecvError::Disconnected
        );
    }
}
