//! Best-effort HTTP metadata observation never changes forwarding bytes or IO outcomes.
use crate::{
    mobile_http_observer::RequestHead,
    mobile_http_session::{SessionEvent, SessionObserver, MAX_PENDING as MAX_ACTIVE_FLOWS},
    model::{CapturedResponse, Flow},
    Engine,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{
    collections::BTreeMap,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
    task::{Context, Poll},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{mpsc, oneshot, watch, OwnedSemaphorePermit, Semaphore},
    time::Instant as Deadline,
};

// Detached cancellation cleanup cannot accumulate unbounded database writers.
// When this budget is busy, the authenticated tunnel continues without observation.
static WRITERS: OnceLock<Arc<Semaphore>> = OnceLock::new();
const MAX_WRITERS: usize = 64;
const MAX_EVENTS: usize = 64;
const MAX_QUEUED_HEAD_BYTES: usize = 256 * 1024;
const IO_ERROR_REASON: &str = "Observation ended after IO error";
type StopReason = Arc<Mutex<Option<(&'static str, Instant)>>>;

#[derive(Clone, Copy)]
enum Direction {
    Request,
    Response,
}
struct QueuedEvent {
    event: SessionEvent,
    at: Instant,
    at_ms: u64,
    _budget: Option<OwnedSemaphorePermit>,
}
struct Observation {
    parser: Option<SessionObserver>,
    // There is exactly one sender. Dropping it closes the queue immediately,
    // even when the other direction never polls again after observation stops.
    events: Option<mpsc::Sender<QueuedEvent>>,
    budget: Arc<Semaphore>,
    stopped: StopReason,
}
impl Observation {
    fn disable(&mut self, reason: &'static str) {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner()) = Some((reason, Instant::now()));
        self.parser = None;
        self.events.take();
    }
    fn observe(&mut self, direction: Direction, bytes: &[u8], eof: bool) {
        let Some(events) = self.events.as_ref() else {
            return;
        };
        if events.is_closed() {
            self.disable("Observation writer stopped");
            return;
        }
        let Some(parser) = self.parser.as_mut() else {
            return;
        };
        let mut refused = false;
        let stopped = self.stopped.clone();
        let budget = self.budget.clone();
        let mut emit = |event: SessionEvent| {
            if let SessionEvent::Stopped { reason } = &event {
                *stopped.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some((*reason, Instant::now()));
            }
            let bytes = match &event {
                SessionEvent::Request { head, .. } => head.raw.len(),
                SessionEvent::Response { head, .. } => head.raw.len(),
                _ => 0,
            };
            let permit = if bytes == 0 {
                None
            } else {
                match budget.clone().try_acquire_many_owned(bytes as u32) {
                    Ok(permit) => Some(permit),
                    Err(_) => {
                        refused = true;
                        return false;
                    }
                }
            };
            let queued = QueuedEvent {
                event,
                at: Instant::now(),
                at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                _budget: permit,
            };
            if events.try_send(queued).is_err() {
                refused = true;
                return false;
            }
            true
        };
        match (direction, eof) {
            (Direction::Request, false) => parser.observe_request(bytes, &mut emit),
            (Direction::Response, false) => parser.observe_response(bytes, &mut emit),
            (Direction::Request, true) => parser.eof_request(&mut emit),
            (Direction::Response, true) => parser.eof_response(&mut emit),
        }
        if refused {
            self.disable("Observation event or byte budget exhausted");
        } else if parser.request_finished() && parser.response_finished() {
            self.parser = None;
            self.events.take();
        }
    }
    fn direction_finished(&self, direction: Direction) -> bool {
        self.parser.as_ref().is_none_or(|parser| match direction {
            Direction::Request => parser.request_finished(),
            Direction::Response => parser.response_finished(),
        })
    }
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
                let finished = this.capture.as_ref().is_some_and(|capture| {
                    if let Ok(mut state) = capture.lock() {
                        state.observe(this.direction, &buf.filled()[before..], false);
                        state.direction_finished(this.direction)
                    } else {
                        true
                    }
                });
                if finished {
                    this.capture = None;
                }
            }
            Poll::Ready(Ok(())) if space > 0 => {
                if let Some(capture) = &this.capture {
                    if let Ok(mut state) = capture.lock() {
                        state.observe(this.direction, &[], true);
                    }
                }
                this.capture = None;
            }
            Poll::Ready(Err(_)) => {
                if let Some(capture) = &this.capture {
                    if let Ok(mut state) = capture.lock() {
                        state.disable(IO_ERROR_REASON);
                    }
                }
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
    at: Instant,
    duration_ms: u64,
    outcome: Result<(u64, u64), String>,
}

// Some(false) is a user deletion/clear, not a storage failure. It suppresses
// that sequence without retrying creation or aborting unrelated future flows.
async fn save(engine: &Arc<Engine>, flow: &Flow, initial: bool) -> Option<bool> {
    let owner = engine.clone();
    let snapshot = flow.clone();
    match tokio::task::spawn_blocking(move || owner.store.insert_observation(&snapshot, initial))
        .await
    {
        Ok(Ok(true)) => {
            let _ = engine.events.send(flow.clone());
            Some(true)
        }
        Ok(Ok(false)) => Some(false),
        result => {
            tracing::debug!(?result, "Mobile tunnel observation persistence stopped");
            None
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

struct ActiveFlow {
    flow: Flow,
    started: Instant,
    request_ended: bool,
    response_ended: bool,
}
async fn update_active(
    engine: &Arc<Engine>,
    active: &mut BTreeMap<u64, ActiveFlow>,
    sequence: u64,
    at: Instant,
) -> bool {
    let Some(entry) = active.get_mut(&sequence) else {
        return true;
    };
    if entry.request_ended && entry.response_ended {
        let mut entry = active.remove(&sequence).unwrap();
        entry.flow.state = "completed".into();
        entry.flow.duration_ms = at.saturating_duration_since(entry.started).as_millis() as u64;
        return save(engine, &entry.flow, false).await.is_some();
    }
    true
}
async fn record(
    engine: Arc<Engine>,
    flow: Flow,
    mut heads: mpsc::Receiver<QueuedEvent>,
    finished: oneshot::Receiver<Finished>,
    stopped: StopReason,
    _permit: OwnedSemaphorePermit,
) {
    if save(&engine, &flow, true).await != Some(true) {
        return;
    }
    let seed = flow.clone();
    let mut connection = Some(flow);
    let mut active = BTreeMap::<u64, ActiveFlow>::new();
    // Only incomplete sequences are retained. Completion is saved immediately,
    // removed from this map and never rewritten at the eventual tunnel finish.
    while let Some(queued) = heads.recv().await {
        let at = queued.at;
        match queued.event {
            SessionEvent::Request { sequence, head } => {
                if active.len() >= MAX_ACTIVE_FLOWS {
                    *stopped.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(("HTTP active-flow budget exhausted", at));
                    heads.close();
                    break;
                }
                let initial = sequence != 0;
                let mut flow = if !initial {
                    let Some(flow) = connection.take() else {
                        return;
                    };
                    flow
                } else {
                    let mut next = seed.clone();
                    next.id = uuid::Uuid::new_v4().to_string();
                    next
                };
                flow.started_at = queued.at_ms;
                apply_request(&mut flow, head);
                match save(&engine, &flow, initial).await {
                    Some(true) => {
                        active.insert(
                            sequence,
                            ActiveFlow {
                                flow,
                                started: at,
                                request_ended: false,
                                response_ended: false,
                            },
                        );
                    }
                    Some(false) => {} // Removed rows cannot be recreated by later response events.
                    None => return,
                }
            }
            SessionEvent::RequestEnd { sequence } => {
                if let Some(entry) = active.get_mut(&sequence) {
                    entry.request_ended = true;
                }
                if !update_active(&engine, &mut active, sequence, at).await {
                    return;
                }
            }
            SessionEvent::Response {
                sequence,
                head: response,
            } => {
                if let Some(entry) = active.get_mut(&sequence) {
                    entry.flow.response = Some(CapturedResponse {
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
                    match save(&engine, &entry.flow, false).await {
                        Some(true) => {}
                        Some(false) => {
                            active.remove(&sequence);
                        }
                        None => return,
                    }
                }
            }
            SessionEvent::ResponseEnd { sequence } => {
                if let Some(entry) = active.get_mut(&sequence) {
                    entry.response_ended = true;
                }
                if !update_active(&engine, &mut active, sequence, at).await {
                    return;
                }
            }
            SessionEvent::Stopped { .. } => {}
        }
        // The queued raw-head byte permit is held through its persistence await.
    }
    let reason = *stopped.lock().unwrap_or_else(|e| e.into_inner());
    // A real read error is immediately followed by the copy result. Preserve
    // that concrete failure; parser/budget stops must not wait for idle IO.
    if let Some((reason, stopped_at)) = reason.filter(|(reason, _)| *reason != IO_ERROR_REASON) {
        // A queue overflow has no room for a Stopped event. The separate reason
        // plus closed single sender still finalizes open metadata without waiting
        // for an idle peer or the one-hour forwarding deadline.
        for (_, mut entry) in std::mem::take(&mut active) {
            entry.flow.state = "interrupted".into();
            entry.flow.duration_ms = stopped_at
                .saturating_duration_since(entry.started)
                .as_millis() as u64;
            entry.flow.notes.push(format!(
                "HTTP metadata observation stopped: {reason}. Body bytes were not recorded."
            ));
            if save(&engine, &entry.flow, false).await.is_none() {
                return;
            }
        }
        if connection.is_none() {
            return;
        }
    }
    match finished.await {
        Ok(finished) => {
            // The seed is only a TCP connection placeholder until a request head
            // is recognized. Tunnel-wide counts never overwrite completed HTTPs.
            if let Some(mut flow) = connection {
                flow.duration_ms = finished.duration_ms;
                match &finished.outcome {
                    Ok((sent, received)) => {
                        flow.state = "completed".into();
                        flow.notes
                            .push(format!("Tunnel bytes: client={sent}, server={received}"));
                    }
                    Err(error) => {
                        flow.state = "failed".into();
                        flow.error = Some(error.clone());
                    }
                }
                if let Some((reason, _)) = reason {
                    flow.notes
                        .push(format!("HTTP observation unavailable: {reason}"));
                }
                let _ = save(&engine, &flow, false).await;
            }
            for (_, mut entry) in active {
                entry.flow.state = if finished.outcome.is_err() {
                    "failed"
                } else {
                    "interrupted"
                }
                .into();
                entry.flow.duration_ms = finished
                    .at
                    .saturating_duration_since(entry.started)
                    .as_millis() as u64;
                entry.flow.notes.push("HTTP framing did not finish before the TCP tunnel ended; body bytes were not recorded.".into());
                if let Err(error) = &finished.outcome {
                    entry.flow.error = Some(error.clone());
                }
                if save(&engine, &entry.flow, false).await.is_none() {
                    return;
                }
            }
        }
        Err(_) => {
            if let Some(flow) = connection {
                active.insert(
                    0,
                    ActiveFlow {
                        flow,
                        started: Instant::now(),
                        request_ended: false,
                        response_ended: false,
                    },
                );
            }
            for (_, mut entry) in active {
                entry.flow.state = "interrupted".into();
                entry
                    .flow
                    .notes
                    .push("Tunnel task ended before its final IO result was recorded.".into());
                if save(&engine, &entry.flow, false).await.is_none() {
                    return;
                }
            }
        }
    }
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
            let (events, heads) = mpsc::channel(MAX_EVENTS);
            let (finish, finished) = oneshot::channel();
            let stopped = Arc::new(Mutex::new(None));
            let capture = Arc::new(Mutex::new(Observation {
                parser: Some(SessionObserver::default()),
                events: Some(events),
                budget: Arc::new(Semaphore::new(MAX_QUEUED_HEAD_BYTES)),
                stopped: stopped.clone(),
            }));
            let writer = tokio::spawn(record(engine, flow, heads, finished, stopped, permit));
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
    drop(client);
    drop(upstream);
    if let Some(finish) = finish {
        let outcome = transferred
            .as_ref()
            .map(|counts| *counts)
            .map_err(ToString::to_string);
        let _ = finish.send(Finished {
            at: Instant::now(),
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
    use crate::mobile_http_observer::HeadObserver;
    use std::future::poll_fn;

    #[tokio::test]
    async fn tap_observes_only_new_readbuf_bytes_and_zero_capacity_is_not_eof() {
        let request = b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n";
        let (events, mut heads) = mpsc::channel(2);
        let state = Arc::new(Mutex::new(Observation {
            parser: Some(SessionObserver::default()),
            events: Some(events),
            budget: Arc::new(Semaphore::new(MAX_QUEUED_HEAD_BYTES)),
            stopped: Arc::new(Mutex::new(None)),
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
        let Some(QueuedEvent {
            event: SessionEvent::Request { head, .. },
            ..
        }) = heads.recv().await
        else {
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

    #[tokio::test]
    async fn exhausted_event_queue_closes_without_polling_the_idle_peer() {
        let (events, mut heads) = mpsc::channel(1);
        let stopped = Arc::new(Mutex::new(None));
        let shared = Arc::new(Mutex::new(Observation {
            parser: Some(SessionObserver::default()),
            events: Some(events),
            budget: Arc::new(Semaphore::new(MAX_QUEUED_HEAD_BYTES)),
            stopped: stopped.clone(),
        }));
        // This idle peer retains the shared observation. Queue termination must
        // not depend on it performing another poll or releasing its Arc.
        let _idle_peer = shared.clone();
        shared.lock().unwrap().observe(
            Direction::Request,
            b"GET / HTTP/1.1\r\nHost: test\r\n\r\n",
            false,
        );
        assert!(stopped.lock().unwrap().is_some());
        assert!(heads.recv().await.is_some());
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), heads.recv())
                .await
                .unwrap()
                .is_none()
        );
        assert!(shared.lock().unwrap().events.is_none());
    }

    #[tokio::test]
    async fn head_byte_budget_stops_recording_without_changing_successful_read() {
        use tokio::io::AsyncReadExt;
        let request = b"GET / HTTP/1.1\r\nHost: test\r\n\r\n";
        let (events, mut heads) = mpsc::channel(64);
        let budget = Arc::new(Semaphore::new(request.len() - 1));
        let stopped = Arc::new(Mutex::new(None));
        let mut tap = ObservedIo {
            inner: request.as_slice(),
            direction: Direction::Request,
            capture: Some(Arc::new(Mutex::new(Observation {
                parser: Some(SessionObserver::default()),
                events: Some(events),
                budget: budget.clone(),
                stopped: stopped.clone(),
            }))),
        };
        let mut result = Vec::new();
        assert_eq!(tap.read_to_end(&mut result).await.unwrap(), request.len());
        assert_eq!(result, request);
        assert!(heads.recv().await.is_none());
        assert!(stopped.lock().unwrap().is_some());
        assert_eq!(budget.available_permits(), request.len() - 1);
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
                parser: Some(SessionObserver::default()),
                events: Some(events),
                budget: Arc::new(Semaphore::new(MAX_QUEUED_HEAD_BYTES)),
                stopped: Arc::new(Mutex::new(None)),
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
