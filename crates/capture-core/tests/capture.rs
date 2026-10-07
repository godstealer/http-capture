use base64::{engine::general_purpose::STANDARD, Engine as _};
use capture_core::{ca::CertificateAuthority, http1::{parse_request, read_body, read_head}, proxy, Engine};
use std::{sync::Arc, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}};
use tokio_rustls::{rustls, TlsAcceptor, TlsConnector};

async fn finish_flow(engine: &Engine) -> capture_core::model::Flow {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(flow) = engine.store.list().unwrap().into_iter().find(|f| f.response.is_some() || f.error.is_some()) { return flow; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap()
}

#[tokio::test]
async fn http_proxy_preserves_order_duplicates_binary_body_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    let mut events = engine.events.subscribe();
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr = origin.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = origin.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let head = read_head(&mut reader).await.unwrap();
        let (method, target, headers) = parse_request(&head).unwrap();
        assert_eq!(method, "POST"); assert_eq!(target, "/echo?q=1");
        assert_eq!(headers.iter().filter(|h| h.name.to_lowercase().starts_with("x-")).map(|h| (h.name.as_str(), h.value.as_str())).collect::<Vec<_>>(), [("X-A", "first"), ("X-B", "middle"), ("x-a", "last")]);
        assert!(!headers.iter().any(|h| h.name.eq_ignore_ascii_case("proxy-authorization")));
        assert_eq!(read_body(&mut reader, &headers, false).await.unwrap().0, [0, 255, 128, 42]);
        reader.write_all(b"HTTP/1.1 200 OK\r\nX-Origin: first\r\nX-Origin: second\r\nContent-Length: 4\r\nConnection: close\r\n\r\n\x00\xff\x80\x2a").await.unwrap();
    });
    let handle = proxy::start(engine.clone(), 0).await.unwrap();
    let mut client = TcpStream::connect(handle.address).await.unwrap();
    let request = format!("POST http://{origin_addr}/echo?q=1 HTTP/1.1\r\nHost: {origin_addr}\r\nX-A: first\r\nX-B: middle\r\nx-a: last\r\nProxy-Authorization: Basic demo\r\nContent-Length: 4\r\n\r\n");
    client.write_all(request.as_bytes()).await.unwrap(); client.write_all(&[0, 255, 128, 42]).await.unwrap();
    let mut response = Vec::new(); client.read_to_end(&mut response).await.unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200")); assert!(response.ends_with(&[0, 255, 128, 42]));
    server.await.unwrap();
    let flow = finish_flow(&engine).await;
    assert_eq!(flow.source, "capture");
    assert_eq!(STANDARD.decode(flow.raw_request_head_base64.as_ref().unwrap()).unwrap(), request.as_bytes());
    assert_eq!(flow.request.headers[1].name, "X-A");
    assert_eq!(flow.request.headers[3].name, "x-a");
    assert_eq!(flow.response.as_ref().unwrap().headers[0].name, "X-Origin");
    assert_eq!(flow.response.as_ref().unwrap().headers[1].value, "second");
    assert!(events.recv().await.unwrap().response.is_none());
    assert!(events.recv().await.unwrap().response.is_some());
    handle.stop().await;
    drop(engine);
    let reopened = Engine::open(dir.path()).unwrap();
    assert_eq!(reopened.store.list().unwrap()[0].id, flow.id);
}

#[tokio::test]
async fn https_connect_is_decrypted_with_trusted_ca_and_upstream_verified() {
    let dir = tempfile::tempdir().unwrap();
    let upstream_ca = CertificateAuthority::load_or_create(&dir.path().join("upstream")).unwrap();
    let pem = std::fs::read(&upstream_ca.cert_path).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &pem[..]) { roots.add(cert.unwrap()).unwrap(); }
    let engine = Engine::open_with_roots(&dir.path().join("capture"), roots).unwrap();
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr = origin.local_addr().unwrap();
    let mut upstream_config = (*upstream_ca.server_config("localhost").unwrap()).clone();
    upstream_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(upstream_config));
    let server = tokio::spawn(async move {
        let (tcp, _) = origin.accept().await.unwrap();
        let tls = acceptor.accept(tcp).await.unwrap();
        assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"http/1.1".as_slice()));
        let mut reader = BufReader::new(tls);
        let head = read_head(&mut reader).await.unwrap();
        assert_eq!(parse_request(&head).unwrap().1, "/secret");
        reader.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\ndecrypted").await.unwrap();
        reader.flush().await.unwrap();
        reader.shutdown().await.unwrap();
    });
    let proxy = proxy::start(engine.clone(), 0).await.unwrap();
    let tcp = TcpStream::connect(proxy.address).await.unwrap();
    let mut tunnel = BufReader::new(tcp);
    tunnel.write_all(format!("CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n", origin_addr.port(), origin_addr.port()).as_bytes()).await.unwrap();
    assert!(read_head(&mut tunnel).await.unwrap().starts_with(b"HTTP/1.1 200"));
    let pem = std::fs::read(&engine.ca.cert_path).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &pem[..]) { roots.add(cert.unwrap()).unwrap(); }
    let config = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    let mut client = TlsConnector::from(Arc::new(config)).connect("localhost".try_into().unwrap(), tunnel).await.unwrap();
    client.write_all(format!("GET /secret HTTP/1.1\r\nHost: localhost:{}\r\nX-Order: one\r\nx-order: two\r\n\r\n", origin_addr.port()).as_bytes()).await.unwrap();
    let mut client = BufReader::new(client);
    let head = read_head(&mut client).await.unwrap();
    let (status, _, headers) = capture_core::http1::parse_response(&head).unwrap();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&head));
    assert_eq!(read_body(&mut client, &headers, false).await.unwrap().0, b"decrypted");
    let flow = finish_flow(&engine).await;
    assert!(flow.request.url.starts_with("https://localhost:"));
    assert_eq!(flow.request.headers[2].name, "x-order");
    let downstream = flow.client_tls.as_ref().unwrap();
    let upstream = flow.response.as_ref().unwrap().upstream_tls.as_ref().unwrap();
    assert_eq!(downstream.server_name.as_deref(), Some("localhost"));
    assert!(!downstream.offered_cipher_suites.is_empty());
    assert!(!downstream.signature_schemes.is_empty());
    for details in [downstream, upstream] {
        assert_eq!(details.version.as_deref(), Some("TLS 1.3"));
        assert!(details.cipher_suite.as_ref().unwrap().starts_with("TLS13_"));
        let cert = &details.certificates[0];
        assert!(cert.parse_error.is_none());
        assert!(!cert.subject.is_empty() && !cert.issuer.is_empty());
        assert!(!cert.not_before.is_empty() && !cert.not_after.is_empty());
        assert_eq!(cert.sha256.split(':').count(), 32);
        assert!(!STANDARD.decode(&cert.der_base64).unwrap().is_empty());
    }
    assert_ne!(downstream.certificates[0].sha256, upstream.certificates[0].sha256);
    assert!(engine.store.list().unwrap()[0].client_tls.is_some());
    assert_eq!(STANDARD.decode(flow.response.unwrap().body_base64).unwrap(), b"decrypted");
    proxy.stop().await; server.await.unwrap();
}

#[tokio::test]
async fn upstream_failure_is_visible_and_stop_releases_port() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = listener.local_addr().unwrap(); drop(listener);
    let handle = proxy::start(engine.clone(), 0).await.unwrap();
    let address = handle.address;
    let mut client = TcpStream::connect(address).await.unwrap();
    client.write_all(format!("GET http://{dead}/ HTTP/1.1\r\nHost: {dead}\r\n\r\n").as_bytes()).await.unwrap();
    let mut response = Vec::new(); client.read_to_end(&mut response).await.unwrap();
    assert!(response.starts_with(b"HTTP/1.1 502"));
    assert!(finish_flow(&engine).await.error.is_some());
    handle.stop().await;
    let restarted = proxy::start(engine, address.port()).await.unwrap();
    restarted.stop().await;
}

#[tokio::test]
async fn ca_survives_restart_and_validates_new_leaf() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    let before = std::fs::read(&engine.ca.cert_path).unwrap();
    drop(engine);
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(std::fs::read(&engine.ca.cert_path).unwrap(), before);
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &before[..]) { roots.add(cert.unwrap()).unwrap(); }
    let config = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    let acceptor = TlsAcceptor::from(engine.ca.server_config("localhost").unwrap());
    let (a, b) = tokio::io::duplex(65536);
    let server = tokio::spawn(async move { acceptor.accept(a).await.unwrap() });
    let _client = TlsConnector::from(Arc::new(config)).connect("localhost".try_into().unwrap(), b).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn expect_continue_and_chunked_upload_are_forwarded_without_changing_entity() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = origin.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        let head = read_head(&mut stream).await.unwrap();
        let (_, _, headers) = parse_request(&head).unwrap();
        assert!(!headers.iter().any(|h| h.name.eq_ignore_ascii_case("expect") || h.name.eq_ignore_ascii_case("transfer-encoding")));
        assert_eq!(read_body(&mut stream, &headers, false).await.unwrap().0, b"hello");
        stream.write_all(b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nok\r\n0\r\n\r\n").await.unwrap();
    });
    let proxy = proxy::start(engine.clone(), 0).await.unwrap();
    let mut client = BufReader::new(TcpStream::connect(proxy.address).await.unwrap());
    client.write_all(format!("POST http://{destination}/upload HTTP/1.1\r\nHost: {destination}\r\nTransfer-Encoding: chunked\r\nExpect: 100-continue\r\n\r\n").as_bytes()).await.unwrap();
    assert_eq!(read_head(&mut client).await.unwrap(), b"HTTP/1.1 100 Continue\r\n\r\n");
    client.write_all(b"5\r\nhello\r\n0\r\n\r\n").await.unwrap();
    let head = read_head(&mut client).await.unwrap();
    let (status, _, headers) = capture_core::http1::parse_response(&head).unwrap();
    assert_eq!(status, 201);
    assert_eq!(read_body(&mut client, &headers, false).await.unwrap().0, b"ok");
    assert_eq!(finish_flow(&engine).await.request.headers[2].name, "Expect");
    server.await.unwrap(); proxy.stop().await;
}

#[tokio::test]
async fn malformed_framing_is_rejected_and_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    let proxy = proxy::start(engine.clone(), 0).await.unwrap();
    let mut client = TcpStream::connect(proxy.address).await.unwrap();
    client.write_all(b"POST http://127.0.0.1:9/ HTTP/1.1\r\nHost: 127.0.0.1:9\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
    let mut response = Vec::new(); client.read_to_end(&mut response).await.unwrap();
    assert!(response.starts_with(b"HTTP/1.1 400"));
    assert!(finish_flow(&engine).await.error.unwrap().contains("Ambiguous"));
    proxy.stop().await;
}

#[tokio::test]
async fn stopping_cancels_inflight_and_finishes_its_record() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    let mut events = engine.events.subscribe();
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let proxy = proxy::start(engine.clone(), 0).await.unwrap();
    let mut client = TcpStream::connect(proxy.address).await.unwrap();
    client.write_all(format!("GET http://{destination}/slow HTTP/1.1\r\nHost: {destination}\r\n\r\n").as_bytes()).await.unwrap();
    let pending = tokio::time::timeout(Duration::from_secs(3), events.recv()).await.unwrap().unwrap();
    assert!(pending.response.is_none() && pending.error.is_none());
    proxy.stop().await;
    assert!(finish_flow(&engine).await.error.unwrap().contains("取消"));
}

#[tokio::test]
async fn invalid_upstream_certificate_is_never_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    let untrusted_ca = CertificateAuthority::load_or_create(&dir.path().join("untrusted")).unwrap();
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(untrusted_ca.server_config("localhost").unwrap());
    let server = tokio::spawn(async move {
        let (tcp, _) = origin.accept().await.unwrap();
        assert!(acceptor.accept(tcp).await.is_err());
    });
    let request = capture_core::model::RequestDraft { upstream_profile_id: None, scripts: Default::default(), pseudo_headers: vec![], engine: "native".into(), method: "GET".into(), url: format!("https://localhost:{}/", destination.port()), headers: vec![], body_base64: String::new(), tls: Default::default() };
    let flow = engine.execute(request, None, "replay", None).await.unwrap();
    assert!(flow.error.unwrap().contains("certificate"));
    assert!(flow.response.is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn configurable_listener_captures_and_releases_port() {
    for host in ["127.0.0.1", "0.0.0.0"] {
        let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
        let origin=TcpListener::bind("127.0.0.1:0").await.unwrap();let target=origin.local_addr().unwrap();
        let server=tokio::spawn(async move {
            let (stream,_)=origin.accept().await.unwrap();let mut reader=BufReader::new(stream);
            read_head(&mut reader).await.unwrap();reader.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
        });
        let handle=proxy::start_on(engine.clone(),host.parse().unwrap(),0).await.unwrap();
        let bound=handle.address;assert_eq!(bound.ip().to_string(),host);
        assert!(proxy::start_on(engine.clone(),host.parse().unwrap(),bound.port()).await.is_err());
        let mut client=TcpStream::connect(("127.0.0.1",bound.port())).await.unwrap();
        client.write_all(format!("GET http://{target}/ HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes()).await.unwrap();
        let mut response=Vec::new();tokio::time::timeout(Duration::from_secs(3),client.read_to_end(&mut response)).await.unwrap().unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));server.await.unwrap();
        // A wildcard listener must reject requests targeting its own port as well.
        let mut client=TcpStream::connect(("127.0.0.1",bound.port())).await.unwrap();
        client.write_all(format!("GET http://127.0.0.1:{}/ HTTP/1.1\r\nHost: localhost\r\n\r\n",bound.port()).as_bytes()).await.unwrap();
        let mut response=Vec::new();tokio::time::timeout(Duration::from_secs(3),client.read_to_end(&mut response)).await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&response).contains("Refusing a proxy loop"));
        handle.stop().await;
        assert!(engine.upstream.listener.read().unwrap().is_none());
        let rebound=TcpListener::bind(bound).await.unwrap();drop(rebound);
    }
}

#[tokio::test]
async fn tun_connect_ip_uses_sni_for_certificate_and_upstream() {
    let dir = tempfile::tempdir().unwrap();
    let upstream_ca = CertificateAuthority::load_or_create(&dir.path().join("upstream")).unwrap();
    let pem = std::fs::read(&upstream_ca.cert_path).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &pem[..]) { roots.add(cert.unwrap()).unwrap(); }
    let engine = Engine::open_with_roots(&dir.path().join("capture"), roots).unwrap();
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr = origin.local_addr().unwrap();
    let mut upstream_config = (*upstream_ca.server_config("localhost").unwrap()).clone();
    upstream_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(upstream_config));
    let server = tokio::spawn(async move {
        let (tcp, _) = origin.accept().await.unwrap();
        let tls = acceptor.accept(tcp).await.unwrap();
        assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"http/1.1".as_slice()));
        let mut reader = BufReader::new(tls);
        let head = read_head(&mut reader).await.unwrap();
        assert_eq!(parse_request(&head).unwrap().1, "/secret");
        reader.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\ndecrypted").await.unwrap();
        reader.flush().await.unwrap();
        reader.shutdown().await.unwrap();
    });
    let proxy = proxy::start_tun_bridge(engine.clone()).await.unwrap();
    let tcp = TcpStream::connect(proxy.address).await.unwrap();
    let mut tunnel = BufReader::new(tcp);
    tunnel.write_all(format!("CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n", origin_addr.port(), origin_addr.port()).as_bytes()).await.unwrap();
    assert!(read_head(&mut tunnel).await.unwrap().starts_with(b"HTTP/1.1 200"));
    let pem = std::fs::read(&engine.ca.cert_path).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &pem[..]) { roots.add(cert.unwrap()).unwrap(); }
    let config = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    let mut client = TlsConnector::from(Arc::new(config)).connect("localhost".try_into().unwrap(), tunnel).await.unwrap();
    client.write_all(format!("GET /secret HTTP/1.1\r\nHost: localhost:{}\r\nX-Order: one\r\nx-order: two\r\n\r\n", origin_addr.port()).as_bytes()).await.unwrap();
    let mut client = BufReader::new(client);
    let head = read_head(&mut client).await.unwrap();
    let (status, _, headers) = capture_core::http1::parse_response(&head).unwrap();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&head));
    assert_eq!(read_body(&mut client, &headers, false).await.unwrap().0, b"decrypted");
    let flow = finish_flow(&engine).await;
    assert!(flow.request.url.starts_with("https://localhost:"));
    assert_eq!(flow.request.headers[2].name, "x-order");
    let downstream = flow.client_tls.as_ref().unwrap();
    let upstream = flow.response.as_ref().unwrap().upstream_tls.as_ref().unwrap();
    assert_eq!(downstream.server_name.as_deref(), Some("localhost"));
    assert!(!downstream.offered_cipher_suites.is_empty());
    assert!(!downstream.signature_schemes.is_empty());
    for details in [downstream, upstream] {
        assert_eq!(details.version.as_deref(), Some("TLS 1.3"));
        assert!(details.cipher_suite.as_ref().unwrap().starts_with("TLS13_"));
        let cert = &details.certificates[0];
        assert!(cert.parse_error.is_none());
        assert!(!cert.subject.is_empty() && !cert.issuer.is_empty());
        assert!(!cert.not_before.is_empty() && !cert.not_after.is_empty());
        assert_eq!(cert.sha256.split(':').count(), 32);
        assert!(!STANDARD.decode(&cert.der_base64).unwrap().is_empty());
    }
    assert_ne!(downstream.certificates[0].sha256, upstream.certificates[0].sha256);
    assert!(engine.store.list().unwrap()[0].client_tls.is_some());
    assert_eq!(STANDARD.decode(flow.response.unwrap().body_base64).unwrap(), b"decrypted");
    proxy.stop().await; server.await.unwrap();
}


#[tokio::test]
async fn tun_connect_carries_plain_http_and_ordered_headers() {
    let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
    let origin=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=origin.local_addr().unwrap();
    let server=tokio::spawn(async move {
        let (stream,_)=origin.accept().await.unwrap();let mut stream=BufReader::new(stream);
        let head=read_head(&mut stream).await.unwrap();let (_,path,headers)=parse_request(&head).unwrap();
        assert_eq!(path,"/tun");assert_eq!(headers[1].name,"X-First");assert_eq!(headers[2].name,"x-second");
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
    });
    let bridge=proxy::start_tun_bridge(engine.clone()).await.unwrap();
    let mut stream=BufReader::new(TcpStream::connect(bridge.address).await.unwrap());
    stream.write_all(format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes()).await.unwrap();
    assert!(read_head(&mut stream).await.unwrap().starts_with(b"HTTP/1.1 200"));
    stream.write_all(format!("GET /tun HTTP/1.1\r\nHost: {address}\r\nX-First: one\r\nx-second: two\r\n\r\n").as_bytes()).await.unwrap();
    let mut response=Vec::new();tokio::time::timeout(Duration::from_secs(5),stream.read_to_end(&mut response)).await.unwrap().unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200"));assert!(finish_flow(&engine).await.request.url.starts_with("http://"));
    bridge.stop().await;server.await.unwrap();
}
