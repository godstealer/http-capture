use capture_core::{Engine, model::*, proxy, http1};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, io::{AsyncReadExt, AsyncWriteExt, BufReader}};

const FIRST: &[u8] = b"id: 1\ndata: first\n\n";
fn draft(url: String) -> RequestDraft {
    RequestDraft { method: "GET".into(), url, engine: "auto".into(), headers: vec![], pseudo_headers: vec![], body_base64: String::new(), tls: Default::default(), scripts: Default::default(), upstream_profile_id: None }
}
async fn origin(chunked: bool) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/events", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = BufReader::new(socket);
        http1::read_head(&mut socket).await.unwrap();
        socket.write_all(if chunked { b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n" } else { b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n" }).await.unwrap();
        // Leave one huge chunk unfinished: the first event must not wait for the entire chunk.
        if chunked { socket.write_all(b"10000\r\n").await.unwrap(); }
        socket.write_all(FIRST).await.unwrap(); socket.flush().await.unwrap();
        let mut end = [0]; let _ = socket.read(&mut end).await;
    });
    (url, task)
}
async fn received(engine: &Engine) -> Flow {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some(flow) = engine.store.list().unwrap().into_iter().find(|f| f.response.as_ref().is_some_and(|r| STANDARD.decode(&r.body_base64).unwrap() == FIRST)) { return flow; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("SSE event must be visible before upstream closes")
}
#[tokio::test]
async fn manual_sse_is_live_and_cancel_preserves_partial_body() {
    for chunked in [false, true] {
        let dir = tempfile::tempdir().unwrap(); let engine = Engine::open(dir.path()).unwrap();
        let (url, server) = origin(chunked).await;
        let sender = engine.clone();
        let task = tokio::spawn(async move { sender.replay(draft(url), None, None).await.unwrap() });
        let live = received(&engine).await;
        assert!(!task.is_finished());
        assert!(engine.executions.cancel(&live.id));
        let flow = tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
        assert!(flow.error.unwrap().contains("取消"));
        assert_eq!(STANDARD.decode(flow.response.unwrap().body_base64).unwrap(), FIRST);
        assert!(!flow.notes.iter().any(|n|n == "SSE 接收中"));
        server.abort();
    }
}
#[tokio::test]
async fn h1_capture_forwards_before_eof_and_can_stop_one_stream() {
    let dir = tempfile::tempdir().unwrap(); let engine = Engine::open(dir.path()).unwrap();
    let (url, server) = origin(true).await;
    let handle = proxy::start(engine.clone(), 0).await.unwrap();
    let mut io = BufReader::new(tokio::net::TcpStream::connect(handle.address).await.unwrap());
    io.write_all(format!("GET {url} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        let head = http1::read_head(&mut io).await.unwrap();
        assert!(head.starts_with(b"HTTP/1.1 200"));
        let mut data = vec![0; FIRST.len()]; io.read_exact(&mut data).await.unwrap(); assert_eq!(data, FIRST);
    }).await.expect("downstream must receive first event before EOF");
    let live = received(&engine).await; assert!(engine.executions.cancel(&live.id));
    let mut rest = Vec::new(); tokio::time::timeout(Duration::from_secs(2), io.read_to_end(&mut rest)).await.unwrap().unwrap();
    assert!(rest.is_empty()); handle.stop().await; server.abort();
}
#[tokio::test]
async fn h2_sse_capture_streams_and_preserves_completed_body() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let dir = tempfile::tempdir().unwrap(); let _ = rustls::crypto::ring::default_provider().install_default();
        let ca = capture_core::ca::CertificateAuthority::load_or_create(&dir.path().join("origin")).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        let pem = std::fs::read(&ca.cert_path).unwrap();
        for cert in rustls_pemfile::certs(&mut &pem[..]) { roots.add(cert.unwrap()).unwrap(); }
        let engine = Engine::open_with_roots(&dir.path().join("capture"), roots).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://localhost:{}/events", listener.local_addr().unwrap().port());
        let mut config = (*ca.server_config("localhost").unwrap()).clone(); config.alpn_protocols = vec![b"h2".to_vec()];
        let (release, wait) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (io, _) = listener.accept().await.unwrap();
            let io = tokio_rustls::TlsAcceptor::from(Arc::new(config)).accept(io).await.unwrap();
            let mut h2 = h2::server::handshake(io).await.unwrap();
            let (_, mut respond) = h2.accept().await.unwrap().unwrap();
            let mut stream = respond.send_response(http::Response::builder().header("content-type", "text/event-stream").body(()).unwrap(), false).unwrap();
            stream.send_data(bytes::Bytes::from_static(FIRST), false).unwrap();
            tokio::select! { _ = h2.accept() => panic!("connection ended early"), _ = wait => {} }
            stream.send_data(bytes::Bytes::from_static(b"data: last\n\n"), true).unwrap();
            let _ = h2.accept().await;
        });
        let (client, proxy_io) = tokio::io::duplex(65536); let e = engine.clone(); let target = url.parse().unwrap();
        let capture = tokio::spawn(async move { capture_core::multiplex::h2_capture(proxy_io, e, target).await });
        let (mut sender, connection) = h2::client::handshake(client).await.unwrap(); let driver = tokio::spawn(connection);
        let (response, _) = sender.send_request(http::Request::builder().uri(url).body(()).unwrap(), true).unwrap();
        let mut body = response.await.unwrap().into_body();
        let chunk = body.data().await.unwrap().unwrap(); assert_eq!(chunk.as_ref(), FIRST); body.flow_control().release_capacity(chunk.len()).unwrap();
        let live = received(&engine).await; assert_eq!(live.client_protocol.as_deref(), Some("HTTP/2"));
        release.send(()).unwrap(); assert_eq!(capture_core::multiplex::recv_h2_body(&mut body).await.unwrap(), b"data: last\n\n");
        driver.abort(); capture.abort(); server.abort();
    }).await.unwrap();
}
