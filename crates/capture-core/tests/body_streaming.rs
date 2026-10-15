use bytes::Bytes;
use capture_core::{model::*, multiplex, transport::*, Engine};
use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};
use std::time::Duration;

struct Origin(Arc<AtomicUsize>);
impl EngineContract<RequestDraft, capture_core::upstream::UpstreamProxy> for Origin {
    fn id(&self) -> &'static str { "auto" }
    fn profiles(&self) -> Vec<String> { vec!["native".into()] }
    fn send<'a>(&'a self, _: &'a RequestDraft) -> SendFuture<'a> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            let response = serde_json::from_value(serde_json::json!({
                "status":200,"version":"HTTP/2","headers":[],"bodyBase64":"b2s="
            }))?;
            Ok((response, vec![]))
        })
    }
}

#[tokio::test]
async fn cancelled_spooled_upload_never_reaches_origin_and_another_stream_survives() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut transports = SendEngines::empty();
        transports.register(Arc::new(Origin(calls.clone()))).unwrap();
        let engine = Engine::open_with_engines(directory.path(), rustls::RootCertStore::empty(), transports).unwrap();
        let (client, server) = tokio::io::duplex(32768);
        let target = "https://stream.test/".parse().unwrap();
        let capture_engine = engine.clone();
        let capture = tokio::spawn(async move { multiplex::h2_capture(server, capture_engine, target).await });
        let (mut sender, connection) = h2::client::handshake(client).await.unwrap();
        let driver = tokio::spawn(connection);
        let request = http::Request::builder().method("POST").uri("https://stream.test/cancel")
            .header("content-length", "2097152").body(()).unwrap();
        let (response, mut upload) = sender.send_request(request, false).unwrap();
        let mut remaining = 128 * 1024;
        while remaining > 0 {
            upload.reserve_capacity(16384);
            let capacity = std::future::poll_fn(|cx| upload.poll_capacity(cx)).await.unwrap().unwrap();
            if capacity == 0 { continue; }
            let count = capacity.min(remaining).min(16384);
            upload.send_data(Bytes::from(vec![7; count]), false).unwrap();
            remaining -= count;
        }
        // Exceeding the initial 65,535-byte receive window proves the capture
        // task has accepted bytes beyond the spool's in-memory threshold.
        upload.send_reset(h2::Reason::CANCEL);
        assert!(response.await.is_err());
        let mut sender = sender.ready().await.unwrap();
        let (response, _) = sender.send_request(http::Request::builder()
            .uri("https://stream.test/complete").body(()).unwrap(), true).unwrap();
        let response = response.await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(multiplex::recv_h2_body(&mut response.into_body()).await.unwrap(), b"ok");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let flows = engine.store.list().unwrap();
        assert_eq!(flows.len(), 1);
        assert!(flows[0].request.url.ends_with("/complete"));
        assert_eq!(flows[0].state, "completed");
        driver.abort(); capture.abort();
        let _ = driver.await; let _ = capture.await;
    }).await.unwrap();
}

#[tokio::test]
async fn h3_rejects_mismatched_response_lengths_but_accepts_head_representation_length() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let directory = tempfile::tempdir().unwrap();
        let ca = capture_core::ca::CertificateAuthority::load_or_create(directory.path()).unwrap();
        let pem = std::fs::read(&ca.cert_path).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        for certificate in rustls_pemfile::certs(&mut &pem[..]) { roots.add(certificate.unwrap()).unwrap(); }
        for (method, declared, bytes, accepted) in [
            ("GET", 9, b"short".as_slice(), false),
            ("GET", 2, b"long".as_slice(), false),
            ("HEAD", 9000, b"".as_slice(), true),
        ] {
            let mut tls = (*ca.server_config("127.0.0.1").unwrap()).clone();
            tls.alpn_protocols = vec![b"h3".to_vec()];
            let endpoint = quinn::Endpoint::server(quinn::ServerConfig::with_crypto(Arc::new(
                quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap()
            )), "127.0.0.1:0".parse().unwrap()).unwrap();
            let address = endpoint.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let connection = endpoint.accept().await.unwrap().await.unwrap();
                let mut server = h3::server::builder().build(h3_quinn::Connection::new(connection)).await.unwrap();
                let resolver = server.accept().await.unwrap().unwrap();
                let worker = tokio::spawn(async move {
                    let (_, mut stream) = resolver.resolve_request().await.unwrap();
                    while stream.recv_data().await.unwrap().is_some() {}
                    stream.send_response(http::Response::builder().status(200)
                        .header("content-length", declared).body(()).unwrap()).await.unwrap();
                    if !bytes.is_empty() { stream.send_data(Bytes::from_static(bytes)).await.unwrap(); }
                    stream.finish().await.unwrap();
                });
                let _ = server.accept().await;
                worker.await.unwrap();
            });
            let request: RequestDraft = serde_json::from_value(serde_json::json!({
                "engine":"h3","method":method,"url":format!("https://{address}/"),"headers":[],"bodyBase64":""
            })).unwrap();
            let result = multiplex::h3_send(&request, &roots).await;
            if accepted {
                let (response, _) = result.unwrap();
                assert!(response.body_base64.is_empty());
                assert_eq!(response.headers.iter().find(|h| h.name == "content-length").unwrap().value, "9000");
            } else {
                assert!(result.is_err(), "mismatched response length must fail");
            }
            server.abort(); let _ = server.await;
        }
    }).await.unwrap();
}
