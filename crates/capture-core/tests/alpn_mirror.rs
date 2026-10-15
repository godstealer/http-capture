use capture_core::{
    ca::CertificateAuthority,
    http1::{parse_request, parse_response, read_body, read_head},
    intercept::{Config as InterceptConfig, Decision, Item, Rule},
    model::{CapturedResponse, Flow},
    multiplex, proxy,
    upstream::UpstreamInput,
    Engine,
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::{LazyConfigAcceptor, TlsConnector};

fn roots(ca: &CertificateAuthority) -> rustls::RootCertStore {
    let pem = std::fs::read(&ca.cert_path).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &pem[..]) {
        roots.add(cert.unwrap()).unwrap();
    }
    roots
}

async fn finish_flow(engine: &Engine) -> Flow {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(flow) = engine.store.list().unwrap().into_iter()
                .find(|flow| flow.response.is_some() || flow.error.is_some()) {
                return flow;
            }
            tokio::task::yield_now().await;
        }
    }).await.unwrap()
}

#[tokio::test]
async fn browser_http2_offer_follows_http1_origin() {
    roundtrip(&[b"h2", b"http/1.1"], &[b"http/1.1"], Some(b"http/1.1"), false).await;
}

#[tokio::test]
async fn browser_http2_offer_follows_http2_origin() {
    roundtrip(&[b"h2", b"http/1.1"], &[b"h2"], Some(b"h2"), false).await;
}

#[tokio::test]
async fn browser_http2_offer_follows_origin_without_alpn() {
    roundtrip(&[b"h2", b"http/1.1"], &[], None, false).await;
}

#[tokio::test]
async fn http1_client_does_not_offer_http2_to_dual_protocol_origin() {
    roundtrip(&[b"http/1.1"], &[b"h2", b"http/1.1"], Some(b"http/1.1"), false).await;
}

#[tokio::test]
async fn browser_http2_offer_follows_http1_origin_through_http_connect_upstream() {
    roundtrip(&[b"h2", b"http/1.1"], &[b"http/1.1"], Some(b"http/1.1"), true).await;
}

async fn waiting_request(engine: &Engine) -> Item {
    loop {
        if let Some(item) = engine.intercept.snapshot().items.into_iter().next() {
            assert_eq!(item.stage, "request");
            return item;
        }
        tokio::task::yield_now().await;
    }
}

fn intercept_localhost(engine: &Engine) {
    engine.intercept.configure(InterceptConfig {
        request: true,
        response: false,
        scope: "capture".into(),
        rules: vec![Rule { host: "localhost".into(), ..Default::default() }],
    }).unwrap();
}

async fn connect_http1_client(engine: &Engine, proxy: std::net::SocketAddr, port: u16)
    -> BufReader<tokio_rustls::client::TlsStream<BufReader<TcpStream>>> {
    let mut tunnel = BufReader::new(TcpStream::connect(proxy).await.unwrap());
    tunnel.write_all(format!("CONNECT localhost:{port} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n")
        .as_bytes()).await.unwrap();
    assert!(read_head(&mut tunnel).await.unwrap().starts_with(b"HTTP/1.1 200"));
    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(roots(&engine.ca)).with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let client = TlsConnector::from(Arc::new(config))
        .connect("localhost".try_into().unwrap(), tunnel).await.unwrap();
    assert_ne!(client.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
    BufReader::new(client)
}

#[tokio::test]
async fn unreachable_eager_origin_can_be_replaced_locally() {
    Box::pin(tokio::time::timeout(Duration::from_secs(15), async {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open_with_roots(dir.path(), rustls::RootCertStore::empty()).unwrap();
        intercept_localhost(&engine);
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = dead.local_addr().unwrap().port();
        drop(dead);
        let proxy = proxy::start(engine.clone(), 0).await.unwrap();
        let resolver_engine = engine.clone();
        let resolver = tokio::spawn(async move {
            let item = waiting_request(&resolver_engine).await;
            assert_eq!(item.flow.request.url, format!("https://localhost:{port}/local"));
            resolver_engine.intercept.resolve(Decision {
                id: item.id,
                action: "replace".into(),
                request: None,
                response: Some(CapturedResponse {
                    status: 201,
                    version: "HTTP/1.1".into(),
                    headers: vec![],
                    body_base64: "bG9jYWw=".into(),
                    raw_head_base64: None,
                    upstream_tls: None,
                    tls_version: None,
                    sent_request_headers: None,
                    trailers: vec![],
                }),
            }).unwrap();
        });
        let mut client = connect_http1_client(&engine, proxy.address, port).await;
        client.write_all(format!("GET /local HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n")
            .as_bytes()).await.unwrap();
        let head = read_head(&mut client).await.unwrap();
        let (status, _, headers) = parse_response(&head).unwrap();
        assert_eq!(status, 201, "{}", String::from_utf8_lossy(&head));
        assert_eq!(read_body(&mut client, &headers, false).await.unwrap().0, b"local");
        resolver.await.unwrap();
        let flow = finish_flow(&engine).await;
        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert!(flow.client_tls.is_some());
        let response = flow.response.unwrap();
        assert!(response.upstream_tls.is_none());
        assert!(response.sent_request_headers.is_none());
        proxy.stop().await;
    })).await.unwrap();
}

#[tokio::test]
async fn modified_target_discards_eager_connection_to_original_origin() {
    Box::pin(tokio::time::timeout(Duration::from_secs(15), async {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = tempfile::tempdir().unwrap();
        let ca = CertificateAuthority::load_or_create(&dir.path().join("origin")).unwrap();
        let engine = Engine::open_with_roots(&dir.path().join("capture"), roots(&ca)).unwrap();
        intercept_localhost(&engine);
        let original = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let original_port = original.local_addr().unwrap().port();
        let replacement = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let replacement_port = replacement.local_addr().unwrap().port();
        let mut config = (*ca.server_config("localhost").unwrap()).clone();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let original_acceptor = acceptor.clone();
        let original_server = tokio::spawn(async move {
            let (tcp, _) = original.accept().await.unwrap();
            drop(original);
            let mut tls = original_acceptor.accept(tcp).await.unwrap();
            let mut byte = [0; 1];
            match tls.read(&mut byte).await {
                Ok(0) => {},
                Err(error) if matches!(error.kind(), std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset) => {},
                result => panic!("original origin must not receive application bytes: {result:?}"),
            }
        });
        let replacement_server = tokio::spawn(async move {
            let (tcp, _) = replacement.accept().await.unwrap();
            drop(replacement);
            let mut tls = BufReader::new(acceptor.accept(tcp).await.unwrap());
            let head = read_head(&mut tls).await.unwrap();
            let (_, target, headers) = parse_request(&head).unwrap();
            assert_eq!(target, "/rewritten");
            assert_eq!(headers.iter().find(|header| header.name.eq_ignore_ascii_case("host")).unwrap().value,
                format!("localhost:{replacement_port}"));
            tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nnew").await.unwrap();
            tls.flush().await.unwrap();
            tls.shutdown().await.unwrap();
        });
        let proxy = proxy::start(engine.clone(), 0).await.unwrap();
        let resolver_engine = engine.clone();
        let resolver = tokio::spawn(async move {
            let item = waiting_request(&resolver_engine).await;
            let mut request = item.flow.request;
            request.url = format!("https://localhost:{replacement_port}/rewritten");
            request.headers.iter_mut().find(|header| header.name.eq_ignore_ascii_case("host"))
                .unwrap().value = format!("localhost:{replacement_port}");
            resolver_engine.intercept.resolve(Decision {
                id: item.id,
                action: "modify".into(),
                request: Some(request),
                response: None,
            }).unwrap();
        });
        let mut client = connect_http1_client(&engine, proxy.address, original_port).await;
        client.write_all(format!("GET /original HTTP/1.1\r\nHost: localhost:{original_port}\r\n\r\n")
            .as_bytes()).await.unwrap();
        let head = read_head(&mut client).await.unwrap();
        let (status, _, headers) = parse_response(&head).unwrap();
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&head));
        assert_eq!(read_body(&mut client, &headers, false).await.unwrap().0, b"new");
        resolver.await.unwrap();
        original_server.await.unwrap();
        replacement_server.await.unwrap();
        let flow = finish_flow(&engine).await;
        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert_eq!(flow.request.url, format!("https://localhost:{replacement_port}/rewritten"));
        assert_eq!(flow.original_request.unwrap().url, format!("https://localhost:{original_port}/original"));
        proxy.stop().await;
    })).await.unwrap();
}

async fn roundtrip(client_alpn: &[&[u8]], origin_alpn: &[&[u8]], selected: Option<&[u8]>, via_http_proxy: bool) {
    Box::pin(tokio::time::timeout(Duration::from_secs(15), async {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = tempfile::tempdir().unwrap();
        let ca = CertificateAuthority::load_or_create(&dir.path().join("origin")).unwrap();
        let engine = Engine::open_with_roots(&dir.path().join("capture"), roots(&ca)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let url = format!("https://localhost:{}/tomorrow?route=kkj-tpe", address.port());
        let mut config = (*ca.server_config("localhost").unwrap()).clone();
        config.alpn_protocols = origin_alpn.iter().map(|protocol| protocol.to_vec()).collect();
        let offered: Vec<Vec<u8>> = client_alpn.iter().map(|protocol| protocol.to_vec()).collect();
        let expected = selected.map(<[u8]>::to_vec);
        let expected_origin = expected.clone();
        let origin = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            // Close the listening socket before TLS: a separate probe followed by a
            // second request connection cannot satisfy this round trip.
            drop(listener);
            let start = LazyConfigAcceptor::new(rustls::server::Acceptor::default(), tcp).await.unwrap();
            let upstream_offered: Vec<Vec<u8>> = start.client_hello().alpn().into_iter().flatten()
                .map(<[u8]>::to_vec).collect();
            assert_eq!(upstream_offered, offered, "upstream ALPN must mirror the browser's offer");
            let tls = start.into_stream(Arc::new(config)).await.unwrap();
            assert_eq!(tls.get_ref().1.alpn_protocol(), expected_origin.as_deref());
            if expected_origin.as_deref() == Some(b"h2") {
                let mut connection = h2::server::handshake(tls).await.unwrap();
                let (request, mut respond) = connection.accept().await.unwrap().unwrap();
                assert_eq!(request.method(), http::Method::GET);
                assert_eq!(request.uri().path_and_query().unwrap().as_str(), "/tomorrow?route=kkj-tpe");
                assert!(multiplex::recv_h2_body(&mut request.into_body()).await.unwrap().is_empty());
                let mut body = respond.send_response(http::Response::builder().status(200)
                    .header("content-length", "2").body(()).unwrap(), false).unwrap();
                body.send_data(bytes::Bytes::from_static(b"ok"), true).unwrap();
                connection.graceful_shutdown();
                // The one-shot upstream client closes its driver after reading the
                // complete response. That can race our final GOAWAY writes; the
                // browser below must still receive 200 and the complete body.
                while let Some(request) = connection.accept().await {
                    match request {
                        Ok(_) => panic!("unexpected additional request"),
                        Err(error) if error.get_io().is_some_and(|io| matches!(io.kind(),
                            std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::ConnectionAborted
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::UnexpectedEof)) => break,
                        Err(error) => panic!("unexpected HTTP/2 shutdown failure: {error:?}"),
                    }
                }
            } else {
                let mut tls = BufReader::new(tls);
                let head = read_head(&mut tls).await.unwrap();
                let (method, target, headers) = parse_request(&head).unwrap();
                assert_eq!(method, "GET");
                assert_eq!(target, "/tomorrow?route=kkj-tpe");
                assert!(read_body(&mut tls, &headers, false).await.unwrap().0.is_empty());
                tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
                tls.flush().await.unwrap();
                tls.shutdown().await.unwrap();
            }
        });

        let relay = if via_http_proxy {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            engine.upstream.update(UpstreamInput {
                enabled: true,
                url: format!("http://{}", listener.local_addr().unwrap()),
                username: String::new(),
                password: None,
                auth_enabled: false,
            }).unwrap();
            Some(tokio::spawn(async move {
                let (client, _) = listener.accept().await.unwrap();
                drop(listener);
                let mut client = BufReader::new(client);
                let head = read_head(&mut client).await.unwrap();
                let (method, target, _) = parse_request(&head).unwrap();
                assert_eq!(method, "CONNECT");
                assert_eq!(target, format!("localhost:{}", address.port()));
                let mut upstream = TcpStream::connect(address).await.unwrap();
                client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.unwrap();
                client.flush().await.unwrap();
                if let Err(error) = tokio::io::copy_bidirectional(&mut client, &mut upstream).await {
                    assert!(matches!(error.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe),
                        "unexpected relay failure: {error}");
                }
            }))
        } else {
            None
        };

        let proxy = proxy::start(engine.clone(), 0).await.unwrap();
        let mut tunnel = BufReader::new(TcpStream::connect(proxy.address).await.unwrap());
        tunnel.write_all(format!("CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
            address.port(), address.port()).as_bytes()).await.unwrap();
        assert!(read_head(&mut tunnel).await.unwrap().starts_with(b"HTTP/1.1 200"));
        // Verify the generated MITM leaf with its CA; the origin was separately
        // verified by Engine using the injected origin CA above.
        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(roots(&engine.ca)).with_no_client_auth();
        config.alpn_protocols = client_alpn.iter().map(|protocol| protocol.to_vec()).collect();
        let client = TlsConnector::from(Arc::new(config))
            .connect("localhost".try_into().unwrap(), tunnel).await.unwrap();
        assert_eq!(client.get_ref().1.alpn_protocol(), expected.as_deref(),
            "browser-facing TLS must follow the origin's selected ALPN");
        if expected.as_deref() == Some(b"h2") {
            let (mut sender, connection) = h2::client::handshake(client).await.unwrap();
            let driver = tokio::spawn(connection);
            let request = http::Request::builder().uri(&url).body(()).unwrap();
            let (response, _) = sender.send_request(request, true).unwrap();
            let response = response.await.unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(multiplex::recv_h2_body(&mut response.into_body()).await.unwrap(), b"ok");
            drop(sender);
            driver.abort();
            let _ = driver.await;
        } else {
            let mut client = BufReader::new(client);
            client.write_all(format!("GET /tomorrow?route=kkj-tpe HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                address.port()).as_bytes()).await.unwrap();
            let head = read_head(&mut client).await.unwrap();
            let (status, _, headers) = parse_response(&head).unwrap();
            assert_eq!(status, 200, "{}", String::from_utf8_lossy(&head));
            assert_eq!(read_body(&mut client, &headers, false).await.unwrap().0, b"ok");
        }
        origin.await.unwrap();
        let flow = finish_flow(&engine).await;
        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert_eq!(flow.request.url, url);
        let protocol = if expected.as_deref() == Some(b"h2") { "HTTP/2" } else { "HTTP/1.1" };
        assert_eq!(flow.client_protocol.as_deref(), Some(protocol));
        let alpn = expected.as_deref().map(|value| std::str::from_utf8(value).unwrap());
        assert_eq!(flow.client_tls.as_ref().unwrap().alpn.as_deref(), alpn);
        let response = flow.response.as_ref().unwrap();
        assert_eq!(response.version, protocol);
        assert_eq!(response.upstream_tls.as_ref().unwrap().alpn.as_deref(), alpn);
        proxy.stop().await;
        if let Some(relay) = relay {
            relay.await.unwrap();
        }
    })).await.unwrap();
}
