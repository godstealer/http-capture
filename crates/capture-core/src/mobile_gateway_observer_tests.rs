//! Authenticated loopback integration: observation must not change forwarding.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);
type Client = tokio_rustls::client::TlsStream<TcpStream>;

struct Fixture {
    engine: Arc<Engine>,
    gateway: Option<MobileGateway>,
    session: [u8; PAIRING_TOKEN_BYTES],
    _directory: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::open(directory.path()).unwrap();
        let gateway =
            MobileGateway::start_inner(Ipv4Addr::LOCALHOST, 0, true, Some(engine.clone()))
                .await
                .unwrap();
        let token = [0x73; PAIRING_TOKEN_BYTES];
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
        assert_eq!(pairing.read_u8().await.unwrap(), 0);
        let mut session = [0; PAIRING_TOKEN_BYTES];
        pairing.read_exact(&mut session).await.unwrap();
        Self {
            _directory: directory,
            engine,
            gateway: Some(gateway),
            session,
        }
    }
    fn gateway(&self) -> &MobileGateway {
        self.gateway.as_ref().unwrap()
    }
    async fn tunnel(&self, port: u16) -> Client {
        let mut client = connect(self.gateway()).await;
        client.write_all(&[2]).await.unwrap();
        client.write_all(&self.session).await.unwrap();
        client.write_u16(9).await.unwrap();
        client.write_all(b"127.0.0.1").await.unwrap();
        client.write_u16(port).await.unwrap();
        assert_eq!(
            timeout(TEST_TIMEOUT, client.read_u8())
                .await
                .unwrap()
                .unwrap(),
            0
        );
        client
    }
}
impl Drop for Fixture {
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
        TEST_TIMEOUT,
        TlsConnector::from(Arc::new(config))
            .connect(ServerName::IpAddress(gateway.address().ip().into()), socket),
    )
    .await
    .unwrap()
    .unwrap()
}
async fn wait_flow(engine: &Engine, expected: impl Fn(&Flow) -> bool) -> Flow {
    timeout(TEST_TIMEOUT, async {
        loop {
            if let Some(flow) = engine.store.list().unwrap().into_iter().find(&expected) {
                return flow;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected gateway observation did not reach storage")
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

#[tokio::test]
async fn slow_fragmented_http_heads_and_interim_responses_preserve_every_wire_byte() {
    let fixture = Fixture::new().await;
    let upstream = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = upstream.local_addr().unwrap().port();
    // Include complete fake heads inside both entities. Only the first genuine
    // request/final response may be observed; entity content must not be parsed.
    let request_body = b"\0\xffGET /not-a-second-request HTTP/1.1\r\nHost: fake\r\n\r\n";
    let mut request_head = format!("POST /slow?q=1 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Order: first\r\nx-order: second\r\nX-Latin1: ").into_bytes();
    request_head.extend_from_slice(b"\xff\r\n");
    request_head
        .extend_from_slice(format!("Content-Length: {}\r\n\r\n", request_body.len()).as_bytes());
    let request = [request_head.as_slice(), request_body].concat();
    let response_body = b"\0\xffHTTP/1.1 599 fake\r\nContent-Length: 0\r\n\r\n";
    let mut response_head =
        b"HTTP/1.1 201 Created\r\nX-Reply: first\r\nx-reply: second\r\nX-Latin1: \x80\r\n".to_vec();
    response_head
        .extend_from_slice(format!("Content-Length: {}\r\n\r\n", response_body.len()).as_bytes());
    let response = [
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Early Hints\r\nLink: </x>\r\n\r\n".as_slice(),
        response_head.as_slice(),
        response_body,
    ]
    .concat();
    let expected_request = request.clone();
    let server_response = response.clone();
    let (first_request_forwarded, first_request_received) = tokio::sync::oneshot::channel();
    let (partial_response_received, partial_response_read) = tokio::sync::oneshot::channel();
    let response_split = response
        .windows(12)
        .position(|part| part == b"HTTP/1.1 201")
        .unwrap()
        + 10;
    let server = tokio::spawn(async move {
        let (mut socket, _) = upstream.accept().await.unwrap();
        let mut observed = vec![0; 13];
        timeout(TEST_TIMEOUT, socket.read_exact(&mut observed))
            .await
            .unwrap()
            .unwrap();
        first_request_forwarded.send(()).unwrap();
        // Consume the known request frame. The prefix barrier proves a partial
        // head was forwarded before completion without early TLS close_notify.
        observed.resize(expected_request.len(), 0);
        timeout(TEST_TIMEOUT, socket.read_exact(&mut observed[13..]))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(observed, expected_request);
        socket
            .write_all(&server_response[..response_split])
            .await
            .unwrap();
        timeout(TEST_TIMEOUT, partial_response_read)
            .await
            .unwrap()
            .unwrap();
        for chunk in server_response[response_split..].chunks(7) {
            socket.write_all(chunk).await.unwrap();
            tokio::task::yield_now().await;
        }
        socket.shutdown().await.unwrap();
    });
    let mut client = fixture.tunnel(port).await;
    tokio::time::sleep(Duration::from_millis(275)).await;
    client.write_all(&request[..13]).await.unwrap();
    timeout(TEST_TIMEOUT, first_request_received)
        .await
        .unwrap()
        .unwrap();
    for chunk in request[13..].chunks(3) {
        client.write_all(chunk).await.unwrap();
        tokio::task::yield_now().await;
    }
    let mut received = vec![0; response_split];
    timeout(TEST_TIMEOUT, client.read_exact(&mut received))
        .await
        .unwrap()
        .unwrap();
    partial_response_received.send(()).unwrap();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, response);
    client.shutdown().await.unwrap();
    timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
    let flow = wait_flow(&fixture.engine, |flow| flow.state == "completed").await;
    assert_eq!(flow.request.method, "POST");
    assert_eq!(
        flow.request.url,
        format!("http://127.0.0.1:{port}/slow?q=1")
    );
    assert_eq!(flow.client_protocol.as_deref(), Some("HTTP/1.1"));
    let headers: Vec<_> = flow
        .request
        .headers
        .iter()
        .map(|header| (header.name.as_str(), header.value.as_str()))
        .collect();
    assert_eq!(
        headers[1..4],
        [
            ("X-Order", "first"),
            ("x-order", "second"),
            ("X-Latin1", "ÿ")
        ]
    );
    assert_eq!(
        STANDARD
            .decode(flow.raw_request_head_base64.as_ref().unwrap())
            .unwrap(),
        request_head
    );
    let response = flow.response.as_ref().unwrap();
    assert_eq!(response.status, 201);
    assert_eq!(response.version, "HTTP/1.1");
    let headers: Vec<_> = response
        .headers
        .iter()
        .map(|header| (header.name.as_str(), header.value.as_str()))
        .collect();
    assert_eq!(
        headers[..3],
        [
            ("X-Reply", "first"),
            ("x-reply", "second"),
            ("X-Latin1", "\u{80}")
        ]
    );
    assert_eq!(
        STANDARD
            .decode(response.raw_head_base64.as_ref().unwrap())
            .unwrap(),
        response_head
    );
    assert!(flow.error.is_none());
    assert_unrecorded(&fixture.engine, &flow);
}

#[tokio::test]
async fn rejected_http_observations_do_not_reject_or_rewrite_the_tcp_stream() {
    let fixture = Fixture::new().await;
    let mut oversized = b"GET /large HTTP/1.1\r\nX-Large: ".to_vec();
    oversized.extend(std::iter::repeat_n(b'a', 70 * 1024));
    oversized.extend_from_slice(b"\r\n\r\n");
    let cases = [
        b"POST /bad HTTP/1.1\r\nBad Header: invalid\r\n\r\nbody".to_vec(),
        oversized,
        vec![0x16, 3, 3, 0, 8, 1, 0, 0, 4, 0, 1, 2, 3],
        b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec(),
    ];
    let mut previous = HashSet::new();
    for request in cases {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected = request.clone();
        let answer = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut observed = vec![0; expected.len()];
            timeout(TEST_TIMEOUT, socket.read_exact(&mut observed))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(observed, expected);
            socket.write_all(answer).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let mut client = fixture.tunnel(port).await;
        client.write_all(&request).await.unwrap();
        let mut response = Vec::new();
        timeout(TEST_TIMEOUT, client.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response, answer);
        client.shutdown().await.unwrap();
        timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
        let flow = wait_flow(&fixture.engine, |flow| {
            !previous.contains(&flow.id) && flow.state == "completed"
        })
        .await;
        assert!(flow.raw_request_head_base64.is_none());
        assert!(flow.request.headers.is_empty());
        assert!(
            flow.response.is_none(),
            "an unrecognized request cannot acquire an unrelated response"
        );
        assert_unrecorded(&fixture.engine, &flow);
        previous.insert(flow.id);
    }
}

#[tokio::test]
async fn server_first_banners_remain_transparent_and_cannot_become_paired_responses() {
    let fixture = Fixture::new().await;
    let cases = [
        (
            b"220 example.test service ready\r\n".as_slice(),
            b"EHLO client\r\n".as_slice(),
            b"250 done\r\n".as_slice(),
        ),
        (
            b"HTTP/1.1 200 Unsolicited\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"GET /later HTTP/1.1\r\nHost: example.test\r\n\r\n".as_slice(),
            b"HTTP/1.1 202 Later\r\nContent-Length: 0\r\n\r\n".as_slice(),
        ),
    ];
    let mut previous = HashSet::new();
    for (greeting, request, answer) in cases {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.write_all(greeting).await.unwrap();
            let mut observed = vec![0; request.len()];
            timeout(TEST_TIMEOUT, socket.read_exact(&mut observed))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(observed, request);
            socket.write_all(answer).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let mut client = fixture.tunnel(port).await;
        let mut banner = vec![0; greeting.len()];
        timeout(TEST_TIMEOUT, client.read_exact(&mut banner))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(banner, greeting);
        client.write_all(request).await.unwrap();
        let mut response = Vec::new();
        timeout(TEST_TIMEOUT, client.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response, answer);
        client.shutdown().await.unwrap();
        timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
        let flow = wait_flow(&fixture.engine, |flow| {
            !previous.contains(&flow.id) && flow.state == "completed"
        })
        .await;
        assert!(
            flow.response.is_none(),
            "server-first bytes must not be paired with a later request"
        );
        if request.starts_with(b"GET ") {
            assert_eq!(flow.request.method, "GET");
        }
        assert_unrecorded(&fixture.engine, &flow);
        previous.insert(flow.id);
    }
}

#[tokio::test]
async fn revoking_a_live_tunnel_closes_forwarding_without_losing_observed_request_headers() {
    let fixture = Fixture::new().await;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let request = b"GET /revoked HTTP/1.1\r\nHost: example.test\r\nX-Keep: observed\r\n\r\n";
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut head = vec![0; request.len()];
        socket.read_exact(&mut head).await.unwrap();
        assert_eq!(head, request);
        let mut byte = [0];
        timeout(TEST_TIMEOUT, socket.read(&mut byte)).await.unwrap()
    });
    let mut client = fixture.tunnel(port).await;
    client.write_all(request).await.unwrap();
    let observed = wait_flow(&fixture.engine, |flow| {
        flow.request
            .headers
            .iter()
            .any(|header| header.name == "X-Keep")
    })
    .await;
    fixture.gateway().disconnect();
    let mut byte = [0];
    let closed = timeout(TEST_TIMEOUT, client.read(&mut byte)).await.unwrap();
    assert!(closed.is_err() || closed.unwrap() == 0);
    let upstream_closed = timeout(TEST_TIMEOUT, server).await.unwrap().unwrap();
    assert!(upstream_closed.is_err() || upstream_closed.unwrap() == 0);
    let flow = wait_flow(&fixture.engine, |flow| {
        flow.id == observed.id && flow.state == "failed"
    })
    .await;
    assert!(flow.error.as_deref().unwrap().contains("revoked"));
    assert_eq!(
        STANDARD
            .decode(flow.raw_request_head_base64.as_ref().unwrap())
            .unwrap(),
        request
    );
    assert!(flow
        .request
        .headers
        .iter()
        .any(|header| header.name == "X-Keep" && header.value == "observed"));
    assert_unrecorded(&fixture.engine, &flow);
}

#[tokio::test]
async fn clearing_or_deleting_observed_history_cannot_resurrect_it_when_forwarding_finishes() {
    for clear in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::open(directory.path()).unwrap();
        let flow = mobile_flow(
            "127.0.0.1:41001".parse().unwrap(),
            "CONNECT".into(),
            "tcp://127.0.0.1:8081".into(),
            None,
        )
        .unwrap();
        let id = flow.id.clone();
        let (mut client, client_forwarder) = tokio::io::duplex(64);
        let (upstream_forwarder, mut server) = tokio::io::duplex(64);
        let (_generation, mut changes) = watch::channel(0u64);
        let forwarding_engine = engine.clone();
        let forwarding = tokio::spawn(async move {
            copy_observed_tunnel(
                client_forwarder,
                upstream_forwarder,
                Some((forwarding_engine, flow)),
                &mut changes,
                0,
                tokio::time::Instant::now() + Duration::from_secs(30),
            )
            .await
        });
        let request = b"GET /deleted HTTP/1.1\r\nHost: example.test\r\nX-Request: retained-until-deleted\r\n\r\n";
        let head = b"HTTP/1.1 200 OK\r\nX-Response: observed\r\nContent-Length: 5\r\n\r\n";
        let (finish, finish_received) = tokio::sync::oneshot::channel();
        let upstream = tokio::spawn(async move {
            let mut read = vec![0; request.len()];
            server.read_exact(&mut read).await.unwrap();
            assert_eq!(read, request);
            server.write_all(head).await.unwrap();
            server.write_all(b"a").await.unwrap();
            finish_received.await.unwrap();
            server.write_all(b"bcde").await.unwrap();
            server.shutdown().await.unwrap();
        });
        client.write_all(request).await.unwrap();
        client.shutdown().await.unwrap();
        let mut first = vec![0; head.len() + 1];
        timeout(TEST_TIMEOUT, client.read_exact(&mut first))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first, [head.as_slice(), b"a"].concat());
        let observed = wait_flow(&engine, |flow| {
            flow.id == id
                && flow
                    .response
                    .as_ref()
                    .is_some_and(|response| response.status == 200)
        })
        .await;
        assert!(observed.raw_request_head_base64.is_some());
        assert_unrecorded(&engine, &observed);
        if clear {
            assert_eq!(engine.store.clear().unwrap(), 1);
        } else {
            engine.store.delete_flows(vec![id.clone()]).unwrap();
        }
        assert!(engine.store.get(&id).unwrap().is_none());
        finish.send(()).unwrap();
        let mut rest = Vec::new();
        timeout(TEST_TIMEOUT, client.read_to_end(&mut rest))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rest, b"bcde");
        timeout(TEST_TIMEOUT, upstream).await.unwrap().unwrap();
        let (sent, received) = timeout(TEST_TIMEOUT, forwarding)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(sent, request.len() as u64);
        assert_eq!(received, (head.len() + 5) as u64);
        // This join is an observer-actor completion barrier, not a timing guess:
        // even final persistence has completed, and neither delete nor clear may
        // recreate the row from an old in-memory Flow clone.
        assert!(engine.store.get(&id).unwrap().is_none());
        assert!(engine.store.metadata_changes(None).unwrap().rows.is_empty());
    }
}

#[tokio::test]
async fn idle_expiry_closes_both_peers_with_and_without_an_observer_record() {
    for record in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::open(directory.path()).unwrap();
        let flow = mobile_flow(
            "127.0.0.1:41002".parse().unwrap(),
            "CONNECT".into(),
            "tcp://127.0.0.1:8082".into(),
            None,
        )
        .unwrap();
        let id = flow.id.clone();
        let entry = record.then(|| (engine.clone(), flow));
        let (mut client, client_forwarder) = tokio::io::duplex(16);
        let (upstream_forwarder, mut server) = tokio::io::duplex(16);
        let (_generation, mut changes) = watch::channel(0u64);
        let result = timeout(
            TEST_TIMEOUT,
            copy_observed_tunnel(
                client_forwarder,
                upstream_forwarder,
                entry,
                &mut changes,
                0,
                tokio::time::Instant::now() + Duration::from_millis(30),
            ),
        )
        .await
        .unwrap();
        assert!(result.err().unwrap().to_string().contains("expired"));
        let mut byte = [0];
        assert_eq!(
            timeout(TEST_TIMEOUT, client.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert_eq!(
            timeout(TEST_TIMEOUT, server.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        if record {
            let flow = engine.store.get(&id).unwrap().unwrap();
            assert_eq!(flow.state, "failed");
            assert!(flow.error.as_deref().unwrap().contains("expired"));
            assert_unrecorded(&engine, &flow);
        } else {
            assert!(engine.store.list().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn revocation_closes_an_incomplete_authenticated_destination_setup_promptly() {
    let fixture = Fixture::new().await;
    let mut client = connect(fixture.gateway()).await;
    client.write_all(&[2]).await.unwrap();
    client.write_all(&fixture.session).await.unwrap();
    // Begin the two-byte destination length, then withhold the second byte.
    // No upstream or Flow should be created while setup remains incomplete.
    client.write_all(&[0]).await.unwrap();
    client.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    fixture.gateway().disconnect();
    let mut response = [0];
    let first = timeout(Duration::from_secs(2), client.read(&mut response))
        .await
        .expect("revocation waited for the eight-second setup read timeout");
    if let Ok(size) = first {
        if size != 0 {
            // If revocation wins before the handler reads its credential, the
            // ordinary unauthorized reply is also valid, followed by closure.
            assert_eq!(response[0], 1);
            let end = timeout(Duration::from_secs(2), client.read(&mut response))
                .await
                .unwrap();
            assert!(end.is_err() || end.unwrap() == 0);
        }
    }
    assert!(fixture.engine.store.list().unwrap().is_empty());
}

#[tokio::test]
async fn transparent_stream_half_close_preserves_the_reply_with_and_without_recording() {
    for record in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::open(directory.path()).unwrap();
        let flow = mobile_flow(
            "127.0.0.1:41003".parse().unwrap(),
            "CONNECT".into(),
            "tcp://127.0.0.1:8083".into(),
            None,
        )
        .unwrap();
        let id = flow.id.clone();
        let entry = record.then(|| (engine.clone(), flow));
        let (mut client, inward) = tokio::io::duplex(32);
        let (outward, mut server) = tokio::io::duplex(32);
        let (_generation, mut changed) = watch::channel(0);
        let forwarding = tokio::spawn(async move {
            copy_observed_tunnel(
                inward,
                outward,
                entry,
                &mut changed,
                0,
                tokio::time::Instant::now() + Duration::from_secs(30),
            )
            .await
        });
        let request =
            b"POST /half-close HTTP/1.1\r\nHost: example.test\r\nContent-Length: 1\r\n\r\na";
        let response = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n";
        let upstream = tokio::spawn(async move {
            let mut bytes = Vec::new();
            server.read_to_end(&mut bytes).await.unwrap();
            assert_eq!(bytes, request);
            server.write_all(response).await.unwrap();
            server.shutdown().await.unwrap();
        });
        client.write_all(request).await.unwrap();
        client.shutdown().await.unwrap();
        let mut bytes = Vec::new();
        timeout(TEST_TIMEOUT, client.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bytes, response);
        timeout(TEST_TIMEOUT, upstream).await.unwrap().unwrap();
        let counts = timeout(TEST_TIMEOUT, forwarding)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(counts, (request.len() as u64, response.len() as u64));
        if record {
            let flow = engine.store.get(&id).unwrap().unwrap();
            assert_eq!(flow.state, "completed");
            assert_eq!(flow.request.method, "POST");
            assert_eq!(flow.response.as_ref().unwrap().status, 204);
            assert_unrecorded(&engine, &flow);
        } else {
            assert!(engine.store.list().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn persistence_failure_keeps_forwarding_bytes_counts_and_half_close_intact() {
    // Every database touched here belongs to this temporary Engine. A real SQLite
    // constraint failure exercises both initial creation and later observation
    // updates without changing a running service or adding a production hook.
    for fail_initial in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::open(directory.path()).unwrap();
        let fault_database =
            rusqlite::Connection::open(directory.path().join("sessions.db")).unwrap();
        let flow = mobile_flow(
            "127.0.0.1:41004".parse().unwrap(),
            "CONNECT".into(),
            "tcp://127.0.0.1:8084".into(),
            None,
        )
        .unwrap();
        let id = flow.id.clone();
        let mut events = engine.events.subscribe();
        if fail_initial {
            fault_database
                .execute_batch(
                    "CREATE TRIGGER fail_mobile_observation BEFORE INSERT ON flows
                 BEGIN SELECT RAISE(ABORT, 'isolated observer persistence failure'); END;",
                )
                .unwrap();
            let error = engine.store.insert_observation(&flow, true).unwrap_err();
            assert!(error
                .to_string()
                .contains("isolated observer persistence failure"));
        }

        let (mut client, inward) = tokio::io::duplex(32);
        let (outward, mut server) = tokio::io::duplex(32);
        let (_generation, mut changes) = watch::channel(0);
        let owner = engine.clone();
        let forwarding = tokio::spawn(async move {
            copy_observed_tunnel(
                inward,
                outward,
                Some((owner, flow)),
                &mut changes,
                0,
                tokio::time::Instant::now() + Duration::from_secs(30),
            )
            .await
        });
        let request = b"POST /storage-error HTTP/1.1\r\nHost: example.test\r\nContent-Length: 4\r\n\r\n\0\xffok";
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n\0\x80ok!";
        let (request_eof, request_eof_received) = tokio::sync::oneshot::channel();
        let (send_response, response_allowed) = tokio::sync::oneshot::channel();
        let upstream = tokio::spawn(async move {
            let mut received = Vec::new();
            timeout(TEST_TIMEOUT, server.read_to_end(&mut received))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(received, request);
            request_eof.send(()).unwrap();
            timeout(TEST_TIMEOUT, response_allowed)
                .await
                .unwrap()
                .unwrap();
            server.write_all(response).await.unwrap();
            server.shutdown().await.unwrap();
        });
        client.write_all(request).await.unwrap();
        // The upstream must see EOF before replying: a failed observer cannot
        // prevent this client-first half-close or truncate the eventual reply.
        client.shutdown().await.unwrap();
        timeout(TEST_TIMEOUT, request_eof_received)
            .await
            .unwrap()
            .unwrap();

        let saved_before_error = if fail_initial {
            assert!(engine.store.get(&id).unwrap().is_none());
            None
        } else {
            // Wait for an actual persisted and published request head before
            // breaking future writes, while the upstream response is withheld.
            let observed = timeout(TEST_TIMEOUT, async {
                loop {
                    let event = events.recv().await.unwrap();
                    assert_eq!(event.id, id);
                    if event.raw_request_head_base64.is_some() {
                        break event;
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(observed.request.method, "POST");
            assert!(observed.response.is_none());
            assert_unrecorded(&engine, &observed);
            fault_database
                .execute_batch(
                    "CREATE TRIGGER fail_mobile_observation BEFORE UPDATE ON flows
                 BEGIN SELECT RAISE(ABORT, 'isolated observer persistence failure'); END;",
                )
                .unwrap();
            let error = engine
                .store
                .insert_observation(&observed, false)
                .unwrap_err();
            assert!(error
                .to_string()
                .contains("isolated observer persistence failure"));
            Some(serde_json::to_value(engine.store.get(&id).unwrap().unwrap()).unwrap())
        };

        send_response.send(()).unwrap();
        let mut received = Vec::new();
        timeout(TEST_TIMEOUT, client.read_to_end(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, response);
        timeout(TEST_TIMEOUT, upstream).await.unwrap().unwrap();
        let counts = timeout(TEST_TIMEOUT, forwarding)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(counts, (request.len() as u64, response.len() as u64));
        // Joining also waits for the writer. No successful lifecycle or response
        // event may claim a failed SQL write reached storage.
        assert_eq!(
            events.try_recv().unwrap_err(),
            tokio::sync::broadcast::error::TryRecvError::Empty
        );
        let saved = engine.store.get(&id).unwrap();
        if let Some(expected) = saved_before_error {
            assert_eq!(serde_json::to_value(saved.unwrap()).unwrap(), expected);
        } else {
            assert!(saved.is_none());
            assert!(engine.store.list().unwrap().is_empty());
        }
    }
}
