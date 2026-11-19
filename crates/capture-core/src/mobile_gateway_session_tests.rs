//! Isolated persistent-HTTP session regressions; bodies remain unrecorded.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rustls::pki_types::ServerName;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;

const LIMIT: Duration = Duration::from_secs(10);
type Client = tokio_rustls::client::TlsStream<TcpStream>;
type CopyTask = JoinHandle<anyhow::Result<(u64, u64)>>;

struct SessionFixture {
    engine: Arc<Engine>,
    gateway: Option<MobileGateway>,
    session: [u8; PAIRING_TOKEN_BYTES],
    _directory: tempfile::TempDir,
}
impl SessionFixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::open(directory.path()).unwrap();
        let gateway =
            MobileGateway::start_inner(Ipv4Addr::LOCALHOST, 0, true, Some(engine.clone()))
                .await
                .unwrap();
        let token = [0x59; PAIRING_TOKEN_BYTES];
        gateway.state.lock().unwrap().pending.replace(PairingCode {
            host: Ipv4Addr::LOCALHOST,
            port: gateway.address().port(),
            token,
            fingerprint: gateway.fingerprint.clone(),
            expires_at: unix_seconds().unwrap() + 300,
        });
        let mut pairing = connect(&gateway).await;
        pairing.write_all(&[1]).await.unwrap();
        pairing.write_all(&token).await.unwrap();
        assert_eq!(timeout(LIMIT, pairing.read_u8()).await.unwrap().unwrap(), 0);
        let mut session = [0; PAIRING_TOKEN_BYTES];
        pairing.read_exact(&mut session).await.unwrap();
        Self {
            engine,
            gateway: Some(gateway),
            session,
            _directory: directory,
        }
    }
    async fn tunnel(&self, port: u16) -> Client {
        let mut client = connect(self.gateway.as_ref().unwrap()).await;
        client.write_all(&[2]).await.unwrap();
        client.write_all(&self.session).await.unwrap();
        client.write_u16(9).await.unwrap();
        client.write_all(b"127.0.0.1").await.unwrap();
        client.write_u16(port).await.unwrap();
        assert_eq!(timeout(LIMIT, client.read_u8()).await.unwrap().unwrap(), 0);
        client
    }
}
impl Drop for SessionFixture {
    fn drop(&mut self) {
        if let Some(gateway) = self.gateway.take() {
            gateway.stop();
        }
    }
}
async fn connect(gateway: &MobileGateway) -> Client {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(gateway.certificate_der.clone()))
        .unwrap();
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let socket = TcpStream::connect(gateway.address()).await.unwrap();
    timeout(
        LIMIT,
        TlsConnector::from(Arc::new(config))
            .connect(ServerName::IpAddress(gateway.address().ip().into()), socket),
    )
    .await
    .unwrap()
    .unwrap()
}
async fn wait_flow(engine: &Engine, predicate: impl Fn(&Flow) -> bool) -> Flow {
    timeout(LIMIT, async {
        loop {
            if let Some(flow) = engine.store.list().unwrap().into_iter().find(&predicate) {
                return flow;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expected persistent-session observation was not saved")
}
fn memory_tunnel(engine: Arc<Engine>) -> (DuplexStream, DuplexStream, CopyTask, String) {
    let flow = mobile_flow(
        "127.0.0.1:42001".parse().unwrap(),
        "CONNECT".into(),
        "tcp://example.test:8080".into(),
        None,
    )
    .unwrap();
    let id = flow.id.clone();
    let (client, inward) = tokio::io::duplex(64);
    let (outward, server) = tokio::io::duplex(64);
    let (generation, mut changes) = watch::channel(0);
    let task = tokio::spawn(async move {
        // A closed watch channel is a real stop signal, so retain its sender.
        let _generation = generation;
        copy_observed_tunnel(
            inward,
            outward,
            Some((engine, flow)),
            &mut changes,
            0,
            tokio::time::Instant::now() + Duration::from_secs(30),
        )
        .await
    });
    (client, server, task, id)
}
fn assert_unrecorded(engine: &Engine, flow: &Flow) {
    for variant in crate::body_capture::VARIANTS {
        assert_eq!(
            flow.body_capture_state(variant),
            crate::body_capture::BodyCaptureState::NotRecorded
        );
        assert!(flow.ensure_body_recorded(variant).is_err());
    }
    assert!(flow.request.body_base64.is_empty());
    if let Some(response) = &flow.response {
        assert!(response.body_base64.is_empty());
    }
    assert!(engine
        .store
        .metadata(&flow.id, None)
        .unwrap()
        .unwrap()
        .body_references
        .is_empty());
}
fn assert_exchange(
    engine: &Engine,
    flow: &Flow,
    method: &str,
    url: &str,
    status: u16,
    request_head: &[u8],
    response_head: &[u8],
) {
    assert_eq!(flow.request.method, method);
    assert_eq!(flow.request.url, url);
    assert_eq!(flow.client_protocol.as_deref(), Some("HTTP/1.1"));
    assert_eq!(
        STANDARD
            .decode(flow.raw_request_head_base64.as_ref().unwrap())
            .unwrap(),
        request_head
    );
    let response = flow.response.as_ref().unwrap();
    assert_eq!(response.status, status);
    assert_eq!(
        STANDARD
            .decode(response.raw_head_base64.as_ref().unwrap())
            .unwrap(),
        response_head
    );
    assert_eq!(flow.state, "completed");
    assert_unrecorded(engine, flow);
}
fn chunked(body: &[u8]) -> Vec<u8> {
    [
        format!("{:x}\r\n", body.len()).as_bytes(),
        body,
        b"\r\n0\r\nX-Trailer: ignored\r\n\r\n",
    ]
    .concat()
}
async fn half_closed_exchange(engine: Arc<Engine>, request: Vec<u8>, response: Vec<u8>) -> String {
    let (mut client, mut server, forwarding, first_id) = memory_tunnel(engine);
    let expected_request = request.clone();
    let expected_response = response.clone();
    let upstream = tokio::spawn(async move {
        let mut observed = Vec::new();
        timeout(LIMIT, server.read_to_end(&mut observed))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(observed, expected_request);
        for part in response.chunks(11) {
            server.write_all(part).await.unwrap();
            tokio::task::yield_now().await;
        }
        server.shutdown().await.unwrap();
    });
    client.write_all(&request).await.unwrap();
    // Duplex half-close is intentional and strict. Real TLS below uses normal
    // length framing; its older Windows early-close_notify reset is not hidden.
    client.shutdown().await.unwrap();
    let mut received = Vec::new();
    timeout(LIMIT, client.read_to_end(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, expected_response);
    timeout(LIMIT, upstream).await.unwrap().unwrap();
    let counts = timeout(LIMIT, forwarding).await.unwrap().unwrap().unwrap();
    assert_eq!(
        counts,
        (request.len() as u64, expected_response.len() as u64)
    );
    first_id
}

#[tokio::test]
async fn authenticated_tls_reuses_one_tunnel_for_independent_completed_requests() {
    let fixture = SessionFixture::new().await;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = b"GET /body-only HTTP/1.1\r\nHost: fake.test\r\n\r\n\0\xff";
    let head1 = format!("POST /first HTTP/1.1\r\nHost: irrelevant.test\r\nX-Order: one\r\nx-order: two\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    let request1 = [head1.as_slice(), body].concat();
    let head2 = b"GET /second?q=2 HTTP/1.1\r\nHost: irrelevant.test\r\nX-Order: three\r\nx-order: four\r\n\r\n";
    let response_body = b"HTTP/1.1 599 Body-Only\r\n\r\n\0\x80";
    let reply_head1 = format!(
        "HTTP/1.1 201 Created\r\nX-Reply: one\r\nx-reply: two\r\nContent-Length: {}\r\n\r\n",
        response_body.len()
    )
    .into_bytes();
    let reply1 = [reply_head1.as_slice(), response_body].concat();
    let reply_head2 =
        b"HTTP/1.1 200 OK\r\nX-Reply: three\r\nx-reply: four\r\nContent-Length: 2\r\n\r\n";
    let reply2 = [reply_head2.as_slice(), b"ok"].concat();
    let expected1 = request1.clone();
    let server_reply1 = reply1.clone();
    let server_reply2 = reply2.clone();
    let upstream = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut received = vec![0; expected1.len()];
        timeout(LIMIT, socket.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, expected1);
        socket.write_all(&server_reply1).await.unwrap();
        let mut received = vec![0; head2.len()];
        timeout(LIMIT, socket.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, head2);
        socket.write_all(&server_reply2).await.unwrap();
        socket.shutdown().await.unwrap();
    });
    let mut client = fixture.tunnel(port).await;
    let placeholder = wait_flow(&fixture.engine, |flow| flow.request.method == "CONNECT").await;
    client.write_all(&request1).await.unwrap();
    let mut received = vec![0; reply1.len()];
    timeout(LIMIT, client.read_exact(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, reply1);
    let first = wait_flow(&fixture.engine, |flow| {
        flow.request.method == "POST" && flow.state == "completed"
    })
    .await;
    assert_eq!(first.id, placeholder.id);
    // Completion belongs to this HTTP exchange, even while the tunnel is open.
    assert_exchange(
        &fixture.engine,
        &first,
        "POST",
        &format!("http://127.0.0.1:{port}/first"),
        201,
        &head1,
        &reply_head1,
    );
    client.write_all(head2).await.unwrap();
    let mut received = Vec::new();
    timeout(LIMIT, client.read_to_end(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, reply2);
    client.shutdown().await.unwrap();
    timeout(LIMIT, upstream).await.unwrap().unwrap();
    let second = wait_flow(&fixture.engine, |flow| {
        flow.request.method == "GET" && flow.state == "completed"
    })
    .await;
    assert_ne!(first.id, second.id);
    assert_exchange(
        &fixture.engine,
        &second,
        "GET",
        &format!("http://127.0.0.1:{port}/second?q=2"),
        200,
        head2,
        reply_head2,
    );
    assert_eq!(fixture.engine.store.list().unwrap().len(), 2);
    let fields: Vec<_> = first
        .request
        .headers
        .iter()
        .map(|header| (header.name.as_str(), header.value.as_str()))
        .collect();
    assert_eq!(&fields[1..3], &[("X-Order", "one"), ("x-order", "two")]);
    let fields: Vec<_> = second
        .response
        .as_ref()
        .unwrap()
        .headers
        .iter()
        .map(|header| (header.name.as_str(), header.value.as_str()))
        .collect();
    assert_eq!(&fields[..2], &[("X-Reply", "three"), ("x-reply", "four")]);
}

#[tokio::test]
async fn pipelined_fifo_skips_cl_and_chunked_entities_and_pairs_interim_responses() {
    let directory = tempfile::tempdir().unwrap();
    let engine = Engine::open(directory.path()).unwrap();
    let fake_request = b"GET /not-a-request HTTP/1.1\r\nHost: fake.test\r\n\r\n";
    let request_head1 = format!("POST /one HTTP/1.1\r\nHost: example.test\r\nX-Dup: a\r\nx-dup: b\r\nContent-Length: {}\r\n\r\n", fake_request.len()).into_bytes();
    let request_head2 = b"PATCH /two HTTP/1.1\r\nHost: example.test\r\nTransfer-Encoding: chunked\r\nX-Dup: c\r\nx-dup: d\r\n\r\n";
    let request_head3 = b"GET /three HTTP/1.1\r\nHost: example.test\r\n\r\n";
    let request = [
        request_head1.as_slice(),
        fake_request,
        request_head2.as_slice(),
        chunked(fake_request).as_slice(),
        request_head3.as_slice(),
    ]
    .concat();
    let fake_response = b"HTTP/1.1 599 Not-A-Response\r\n\r\n";
    let response_head1 =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nX-Dup: r1\r\nx-dup: r2\r\n\r\n";
    let response_head2 = b"HTTP/1.1 204 No Content\r\nX-Result: two\r\n\r\n";
    let response_head3 =
        b"HTTP/1.1 304 Not Modified\r\nContent-Length: 9999\r\nX-Result: three\r\n\r\n";
    let response = [
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Early Hints\r\nLink: </hint>\r\n\r\n"
            .as_slice(),
        response_head1.as_slice(),
        chunked(fake_response).as_slice(),
        response_head2.as_slice(),
        response_head3.as_slice(),
    ]
    .concat();
    let first_id = half_closed_exchange(engine.clone(), request, response).await;
    let flows = engine.store.list().unwrap();
    assert_eq!(flows.len(), 3);
    let one = flows
        .iter()
        .find(|flow| flow.request.method == "POST")
        .unwrap();
    let two = flows
        .iter()
        .find(|flow| flow.request.method == "PATCH")
        .unwrap();
    let three = flows
        .iter()
        .find(|flow| flow.request.method == "GET")
        .unwrap();
    assert_eq!(one.id, first_id);
    assert_eq!(
        [&one.id, &two.id, &three.id]
            .into_iter()
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    assert_exchange(
        &engine,
        one,
        "POST",
        "http://example.test:8080/one",
        200,
        &request_head1,
        response_head1,
    );
    assert_exchange(
        &engine,
        two,
        "PATCH",
        "http://example.test:8080/two",
        204,
        request_head2,
        response_head2,
    );
    assert_exchange(
        &engine,
        three,
        "GET",
        "http://example.test:8080/three",
        304,
        request_head3,
        response_head3,
    );
    let fields: Vec<_> = two
        .request
        .headers
        .iter()
        .map(|header| (header.name.as_str(), header.value.as_str()))
        .collect();
    assert_eq!(&fields[2..], &[("X-Dup", "c"), ("x-dup", "d")]);
    let fields: Vec<_> = one
        .response
        .as_ref()
        .unwrap()
        .headers
        .iter()
        .map(|header| (header.name.as_str(), header.value.as_str()))
        .collect();
    assert_eq!(&fields[1..], &[("X-Dup", "r1"), ("x-dup", "r2")]);
}

#[tokio::test]
async fn head_with_nonzero_content_length_cannot_consume_the_next_response() {
    let directory = tempfile::tempdir().unwrap();
    let engine = Engine::open(directory.path()).unwrap();
    let request1 = b"HEAD /metadata HTTP/1.1\r\nHost: example.test\r\n\r\n";
    let request2 = b"GET /entity HTTP/1.1\r\nHost: example.test\r\n\r\n";
    let response1 = b"HTTP/1.1 200 OK\r\nContent-Length: 123456\r\n\r\n";
    let response2 = b"HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\n\r\n";
    half_closed_exchange(
        engine.clone(),
        [request1.as_slice(), request2.as_slice()].concat(),
        [response1.as_slice(), response2.as_slice(), b"ok"].concat(),
    )
    .await;
    let flows = engine.store.list().unwrap();
    assert_eq!(flows.len(), 2);
    let first = flows
        .iter()
        .find(|flow| flow.request.method == "HEAD")
        .unwrap();
    let second = flows
        .iter()
        .find(|flow| flow.request.method == "GET")
        .unwrap();
    assert_exchange(
        &engine,
        first,
        "HEAD",
        "http://example.test:8080/metadata",
        200,
        request1,
        response1,
    );
    assert_exchange(
        &engine,
        second,
        "GET",
        "http://example.test:8080/entity",
        202,
        request2,
        response2,
    );
}

#[tokio::test]
async fn upgrades_and_successful_connect_leave_following_payload_opaque() {
    for connect in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::open(directory.path()).unwrap();
        let (request, response) = if connect {
            (
                b"CONNECT other.test:443 HTTP/1.1\r\nHost: other.test:443\r\n\r\n".as_slice(),
                b"HTTP/1.1 200 Connection Established\r\n\r\n".as_slice(),
            )
        } else {
            (b"GET /upgrade HTTP/1.1\r\nHost: example.test\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n".as_slice(), b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n".as_slice())
        };
        let opaque_request = b"GET /opaque-fake HTTP/1.1\r\nHost: fake.test\r\n\r\n\0\xff";
        let opaque_response = b"HTTP/1.1 599 Opaque-Fake\r\nContent-Length: 0\r\n\r\n\0\x80";
        let (mut client, mut server, forwarding, first_id) = memory_tunnel(engine.clone());
        let upstream = tokio::spawn(async move {
            let mut received = vec![0; request.len()];
            timeout(LIMIT, server.read_exact(&mut received))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(received, request);
            server.write_all(response).await.unwrap();
            let mut received = Vec::new();
            timeout(LIMIT, server.read_to_end(&mut received))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(received, opaque_request);
            server.write_all(opaque_response).await.unwrap();
            server.shutdown().await.unwrap();
        });
        client.write_all(request).await.unwrap();
        let mut received = vec![0; response.len()];
        timeout(LIMIT, client.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, response);
        client.write_all(opaque_request).await.unwrap();
        client.shutdown().await.unwrap();
        let mut received = Vec::new();
        timeout(LIMIT, client.read_to_end(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, opaque_response);
        timeout(LIMIT, upstream).await.unwrap().unwrap();
        let counts = timeout(LIMIT, forwarding).await.unwrap().unwrap().unwrap();
        assert_eq!(
            counts,
            (
                (request.len() + opaque_request.len()) as u64,
                (response.len() + opaque_response.len()) as u64
            )
        );
        let flows = engine.store.list().unwrap();
        assert_eq!(flows.len(), 1);
        assert_eq!(flows[0].id, first_id);
        assert_exchange(
            &engine,
            &flows[0],
            if connect { "CONNECT" } else { "GET" },
            if connect {
                "tcp://example.test:8080"
            } else {
                "http://example.test:8080/upgrade"
            },
            if connect { 200 } else { 101 },
            request,
            response,
        );
    }
}

#[tokio::test]
async fn eof_finishes_close_delimited_response_but_not_truncated_framing() {
    let complete_request = b"GET /eof HTTP/1.1\r\nHost: example.test\r\n\r\n";
    let complete_response =
        b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nHTTP/1.1 599 Body-Only\r\n\r\n";
    let incomplete_response = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort";
    let incomplete_request =
        b"POST /eof HTTP/1.1\r\nHost: example.test\r\nContent-Length: 10\r\n\r\nshort";
    for (request, response, completed) in [
        (
            complete_request.as_slice(),
            complete_response.as_slice(),
            true,
        ),
        (
            complete_request.as_slice(),
            incomplete_response.as_slice(),
            false,
        ),
        (
            incomplete_request.as_slice(),
            b"HTTP/1.1 413 Content Too Large\r\nContent-Length: 0\r\n\r\n".as_slice(),
            false,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::open(directory.path()).unwrap();
        let first_id =
            half_closed_exchange(engine.clone(), request.to_vec(), response.to_vec()).await;
        let flows = engine.store.list().unwrap();
        assert_eq!(flows.len(), 1);
        let flow = &flows[0];
        assert_eq!(flow.id, first_id);
        assert_eq!(
            flow.state,
            if completed {
                "completed"
            } else {
                "interrupted"
            }
        );
        assert!(flow.raw_request_head_base64.is_some());
        if request.starts_with(b"GET ") {
            assert!(flow.response.is_some());
        }
        assert_unrecorded(&engine, flow);
    }
}

#[tokio::test]
async fn deleting_or_clearing_during_second_exchange_cannot_resurrect_saved_records() {
    for clear in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::open(directory.path()).unwrap();
        let (mut client, mut server, forwarding, first_id) = memory_tunnel(engine.clone());
        let request1 = b"GET /retained HTTP/1.1\r\nHost: example.test\r\n\r\n";
        let request2 = b"GET /deleted HTTP/1.1\r\nHost: example.test\r\n\r\n";
        let response1 = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        let response2 = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n";
        let (finish, finish_allowed) = oneshot::channel();
        let upstream = tokio::spawn(async move {
            let mut received = vec![0; request1.len()];
            timeout(LIMIT, server.read_exact(&mut received))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(received, request1);
            server.write_all(response1).await.unwrap();
            let mut received = vec![0; request2.len()];
            timeout(LIMIT, server.read_exact(&mut received))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(received, request2);
            server.write_all(response2).await.unwrap();
            server.write_all(b"a").await.unwrap();
            timeout(LIMIT, finish_allowed).await.unwrap().unwrap();
            server.write_all(b"bcde").await.unwrap();
            server.shutdown().await.unwrap();
        });
        client.write_all(request1).await.unwrap();
        let mut received = vec![0; response1.len()];
        timeout(LIMIT, client.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, response1);
        let first = wait_flow(&engine, |flow| {
            flow.id == first_id && flow.state == "completed"
        })
        .await;
        let first_snapshot = serde_json::to_value(&first).unwrap();
        client.write_all(request2).await.unwrap();
        let mut received = vec![0; response2.len() + 1];
        timeout(LIMIT, client.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, [response2.as_slice(), b"a"].concat());
        let second = wait_flow(&engine, |flow| {
            flow.id != first_id && flow.response.is_some()
        })
        .await;
        assert_eq!(second.request.url, "http://example.test:8080/deleted");
        assert_ne!(second.state, "completed");
        assert_unrecorded(&engine, &second);
        if clear {
            assert_eq!(engine.store.clear().unwrap(), 2);
        } else {
            engine.store.delete_flows(vec![second.id.clone()]).unwrap();
        }
        finish.send(()).unwrap();
        let mut rest = Vec::new();
        timeout(LIMIT, client.read_to_end(&mut rest))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rest, b"bcde");
        client.shutdown().await.unwrap();
        timeout(LIMIT, upstream).await.unwrap().unwrap();
        let counts = timeout(LIMIT, forwarding).await.unwrap().unwrap().unwrap();
        assert_eq!(
            counts,
            (
                (request1.len() + request2.len()) as u64,
                (response1.len() + response2.len() + 5) as u64
            )
        );
        // Completion joins the sole writer, so this assertion has no timing race.
        assert!(engine.store.get(&second.id).unwrap().is_none());
        if clear {
            assert!(engine.store.list().unwrap().is_empty());
        } else {
            assert_eq!(engine.store.list().unwrap().len(), 1);
            assert_eq!(
                serde_json::to_value(engine.store.get(&first_id).unwrap().unwrap()).unwrap(),
                first_snapshot
            );
        }
    }
}

#[tokio::test]
async fn sql_failure_on_later_request_stops_recording_without_stopping_the_session() {
    let directory = tempfile::tempdir().unwrap();
    let engine = Engine::open(directory.path()).unwrap();
    let database = rusqlite::Connection::open(directory.path().join("sessions.db")).unwrap();
    let mut events = engine.events.subscribe();
    let (mut client, mut server, forwarding, first_id) = memory_tunnel(engine.clone());
    let request1 = b"GET /saved HTTP/1.1\r\nHost: example.test\r\n\r\n";
    let request2 =
        b"POST /sql-error HTTP/1.1\r\nHost: example.test\r\nContent-Length: 3\r\n\r\n\0\xffa";
    let request3 = b"GET /network-still-works HTTP/1.1\r\nHost: example.test\r\n\r\n";
    let response1 = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
    let response2 = b"HTTP/1.1 201 Created\r\nContent-Length: 3\r\n\r\n\0\x80b";
    let response3 = b"HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\n\r\nok";
    let upstream = tokio::spawn(async move {
        let mut received = vec![0; request1.len()];
        timeout(LIMIT, server.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, request1);
        server.write_all(response1).await.unwrap();
        let mut received = Vec::new();
        timeout(LIMIT, server.read_to_end(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            received,
            [request2.as_slice(), request3.as_slice()].concat()
        );
        server.write_all(response2).await.unwrap();
        server.write_all(response3).await.unwrap();
        server.shutdown().await.unwrap();
    });
    client.write_all(request1).await.unwrap();
    let mut received = vec![0; response1.len()];
    timeout(LIMIT, client.read_exact(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, response1);
    let first = timeout(LIMIT, async {
        loop {
            let flow = events.recv().await.unwrap();
            if flow.id == first_id && flow.state == "completed" {
                break flow;
            }
        }
    })
    .await
    .unwrap();
    let snapshot = serde_json::to_value(first).unwrap();
    // This SQLite trigger affects only this fresh temporary database. Existing
    // first-exchange data remains readable while later inserts actually fail.
    database.execute_batch("CREATE TRIGGER fail_later_observation BEFORE INSERT ON flows BEGIN SELECT RAISE(ABORT, 'isolated session SQL failure'); END;").unwrap();
    let probe = mobile_flow(
        "127.0.0.1:42002".parse().unwrap(),
        "CONNECT".into(),
        "tcp://example.test:8080".into(),
        None,
    )
    .unwrap();
    assert!(engine
        .store
        .insert_observation(&probe, true)
        .unwrap_err()
        .to_string()
        .contains("isolated session SQL failure"));
    client
        .write_all(&[request2.as_slice(), request3.as_slice()].concat())
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let mut received = Vec::new();
    timeout(LIMIT, client.read_to_end(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        received,
        [response2.as_slice(), response3.as_slice()].concat()
    );
    timeout(LIMIT, upstream).await.unwrap().unwrap();
    let counts = timeout(LIMIT, forwarding).await.unwrap().unwrap().unwrap();
    assert_eq!(
        counts,
        (
            (request1.len() + request2.len() + request3.len()) as u64,
            (response1.len() + response2.len() + response3.len()) as u64
        )
    );
    let flows = engine.store.list().unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(serde_json::to_value(&flows[0]).unwrap(), snapshot);
    assert_unrecorded(&engine, &flows[0]);
    assert_eq!(
        events.try_recv().unwrap_err(),
        tokio::sync::broadcast::error::TryRecvError::Empty
    );
}

// Register a separate wakeup while the underlying read is pending, so an IO
// failure can be injected after exact bytes have already reached both peers.
struct ReadResetOnSignal<T> {
    inner: T,
    reset: Option<oneshot::Receiver<()>>,
    failed: bool,
}
impl<T: AsyncRead + Unpin> AsyncRead for ReadResetOnSignal<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if let Some(signal) = this.reset.as_mut() {
            match Pin::new(signal).poll(cx) {
                Poll::Ready(result) => {
                    this.failed = result.is_ok();
                    this.reset = None;
                }
                Poll::Pending => {}
            }
        }
        if this.failed {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "isolated upstream read reset",
            )));
        }
        Pin::new(&mut this.inner).poll_read(cx, buffer)
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for ReadResetOnSignal<T> {
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

#[tokio::test]
async fn a_real_io_reset_fails_only_the_open_exchange_and_preserves_completed_rows() {
    let directory = tempfile::tempdir().unwrap();
    let engine = Engine::open(directory.path()).unwrap();
    let flow = mobile_flow(
        "127.0.0.1:42003".parse().unwrap(),
        "CONNECT".into(),
        "tcp://example.test:8080".into(),
        None,
    )
    .unwrap();
    let first_id = flow.id.clone();
    let (mut client, inward) = tokio::io::duplex(64);
    let (outward, mut server) = tokio::io::duplex(64);
    let (reset, reset_received) = oneshot::channel();
    let upstream = ReadResetOnSignal {
        inner: outward,
        reset: Some(reset_received),
        failed: false,
    };
    let (generation, mut changes) = watch::channel(0);
    let owner = engine.clone();
    let forwarding = tokio::spawn(async move {
        let _generation = generation;
        copy_observed_tunnel(
            inward,
            upstream,
            Some((owner, flow)),
            &mut changes,
            0,
            tokio::time::Instant::now() + Duration::from_secs(30),
        )
        .await
    });
    let request1 = b"GET /completed-before-reset HTTP/1.1\r\nHost: example.test\r\n\r\n";
    let request2 = b"GET /interrupted-by-reset HTTP/1.1\r\nHost: example.test\r\n\r\n";
    let response1 = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    let response_head2 = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nX-Partial: observed\r\n\r\n";
    let server_task = tokio::spawn(async move {
        let mut received = vec![0; request1.len()];
        timeout(LIMIT, server.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, request1);
        server.write_all(response1).await.unwrap();
        let mut received = vec![0; request2.len()];
        timeout(LIMIT, server.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, request2);
        server.write_all(response_head2).await.unwrap();
        server.write_all(b"ab").await.unwrap();
        // Keep the peer open: its EOF is a consequence of the copy error, not
        // the event that should classify the unfinished HTTP flow as failed.
        let mut unexpected = Vec::new();
        timeout(LIMIT, server.read_to_end(&mut unexpected))
            .await
            .unwrap()
            .unwrap();
        assert!(unexpected.is_empty());
    });
    client.write_all(request1).await.unwrap();
    let mut received = vec![0; response1.len()];
    timeout(LIMIT, client.read_exact(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, response1);
    let completed = wait_flow(&engine, |flow| {
        flow.id == first_id && flow.state == "completed"
    })
    .await;
    assert_unrecorded(&engine, &completed);
    let completed_snapshot = serde_json::to_value(completed).unwrap();

    client.write_all(request2).await.unwrap();
    let mut received = vec![0; response_head2.len() + 2];
    timeout(LIMIT, client.read_exact(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, [response_head2.as_slice(), b"ab"].concat());
    let unfinished = wait_flow(&engine, |flow| {
        flow.id != first_id && flow.response.is_some()
    })
    .await;
    assert_ne!(unfinished.state, "completed");
    assert_eq!(
        unfinished.request.url,
        "http://example.test:8080/interrupted-by-reset"
    );
    assert_unrecorded(&engine, &unfinished);

    reset.send(()).unwrap();
    let error = timeout(LIMIT, forwarding)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    let io_error = error
        .downcast_ref::<std::io::Error>()
        .expect("copy must preserve its real IO error");
    assert_eq!(io_error.kind(), std::io::ErrorKind::ConnectionReset);
    assert!(error.to_string().contains("isolated upstream read reset"));
    let mut extra = Vec::new();
    timeout(LIMIT, client.read_to_end(&mut extra))
        .await
        .unwrap()
        .unwrap();
    assert!(extra.is_empty());
    timeout(LIMIT, server_task).await.unwrap().unwrap();

    // The helper joins persistence before returning the error: this is a final
    // state assertion rather than an eventual interrupted/failed timing race.
    let failed = engine.store.get(&unfinished.id).unwrap().unwrap();
    assert_eq!(failed.state, "failed");
    assert!(failed
        .error
        .as_deref()
        .unwrap()
        .contains("isolated upstream read reset"));
    assert_eq!(
        STANDARD
            .decode(
                failed
                    .response
                    .as_ref()
                    .unwrap()
                    .raw_head_base64
                    .as_ref()
                    .unwrap()
            )
            .unwrap(),
        response_head2
    );
    assert_unrecorded(&engine, &failed);
    assert_eq!(engine.store.list().unwrap().len(), 2);
    assert_eq!(
        serde_json::to_value(engine.store.get(&first_id).unwrap().unwrap()).unwrap(),
        completed_snapshot
    );
}
