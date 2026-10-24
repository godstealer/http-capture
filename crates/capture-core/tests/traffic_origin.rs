use capture_core::{ca::CertificateAuthority, http1::read_head, model::*, proxy, Engine};
use std::{sync::Arc, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}};

fn check_local(origin: &TrafficOrigin, address: String) {
    assert_eq!(origin.entry, "explicit-proxy");
    assert_eq!(origin.client_address.as_deref(), Some(address.as_str()));
    #[cfg(any(windows, target_os = "linux"))]
    {
        let process = origin.process.as_ref().expect("the test's live local TCP socket must resolve");
        assert_eq!(process.pid, std::process::id());
        assert!(!process.name.is_empty());
        assert_eq!(origin.process_status, "identified");
    }
}

async fn completed(engine: &Engine, count: usize) -> Vec<Flow> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let flows = engine.store.list().unwrap();
            if flows.len() == count && flows.iter().all(|flow| !["sending", "receiving"].contains(&flow.state.as_str())) { return flows; }
            tokio::task::yield_now().await;
        }
    }).await.expect("capture did not finish")
}

#[tokio::test]
async fn distinct_clients_persist_their_origin_and_replay_does_not_inherit_it() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).unwrap();
        let source = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = source.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (io, _) = source.accept().await.unwrap();
                let mut io = BufReader::new(io);
                read_head(&mut io).await.unwrap();
                io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
            }
        });
        let handle = proxy::start(engine.clone(), 0).await.unwrap();
        let mut clients = Vec::new();
        for index in 0..2 {
            let mut client = TcpStream::connect(handle.address).await.unwrap();
            let peer = client.local_addr().unwrap().to_string();
            client.write_all(format!("GET http://{address}/client-{index} HTTP/1.1\r\nHost: {address}\r\nUser-Agent: deliberately-not-a-process-name\r\nX-Capture-Process: fake.exe\r\n\r\n").as_bytes()).await.unwrap();
            clients.push((client, peer));
        }
        for (client, _) in &mut clients {
            let mut response = Vec::new(); client.read_to_end(&mut response).await.unwrap();
            assert!(response.starts_with(b"HTTP/1.1 200"));
        }
        let flows = completed(&engine, 2).await;
        for (index, (_, peer)) in clients.iter().enumerate() {
            let flow = flows.iter().find(|flow| flow.request.url.ends_with(&format!("/client-{index}"))).unwrap();
            check_local(flow.traffic_origin.as_ref().unwrap(), peer.clone());
            assert_ne!(flow.traffic_origin.as_ref().unwrap().process.as_ref().map(|p| p.name.as_str()), Some("fake.exe"));
        }
        let replay = engine.replay(flows[0].request.clone(), Some(flows[0].id.clone()), None).await.unwrap();
        assert_eq!(replay.source, "replay"); assert!(replay.traffic_origin.is_none());
        server.await.unwrap(); handle.stop().await;
        drop(engine);
        let reopened = Engine::open(dir.path()).unwrap();
        for flow in flows {
            assert_eq!(reopened.store.get(&flow.id).unwrap().unwrap().traffic_origin, flow.traffic_origin);
        }
    }).await.unwrap();
}

#[tokio::test]
async fn browser_sized_connection_burst_keeps_each_clients_own_origin() {
    tokio::time::timeout(Duration::from_secs(20), async {
        const COUNT: usize = 16;
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).unwrap();
        let source = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = source.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..COUNT {
                let (io, _) = source.accept().await.unwrap();
                let mut io = BufReader::new(io);
                read_head(&mut io).await.unwrap();
                io.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").await.unwrap();
            }
        });
        let handle = proxy::start(engine.clone(), 0).await.unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(COUNT));
        let mut clients = tokio::task::JoinSet::new();
        for index in 0..COUNT {
            let proxy = handle.address;
            let barrier = barrier.clone();
            clients.spawn(async move {
                let mut client = TcpStream::connect(proxy).await.unwrap();
                let peer = client.local_addr().unwrap().to_string();
                barrier.wait().await;
                client.write_all(format!("GET http://{address}/burst-{index} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes()).await.unwrap();
                let mut response = Vec::new(); client.read_to_end(&mut response).await.unwrap();
                assert!(response.starts_with(b"HTTP/1.1 204"));
                (index, peer)
            });
        }
        let mut peers = Vec::new();
        while let Some(client) = clients.join_next().await { peers.push(client.unwrap()); }
        let flows = completed(&engine, COUNT).await;
        for (index, peer) in peers {
            let flow = flows.iter().find(|flow| flow.request.url.ends_with(&format!("/burst-{index}"))).unwrap();
            check_local(flow.traffic_origin.as_ref().unwrap(), peer);
        }
        server.await.unwrap(); handle.stop().await;
    }).await.unwrap();
}

fn roots(ca: &CertificateAuthority) -> rustls::RootCertStore {
    let pem = std::fs::read(&ca.cert_path).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &pem[..]) { roots.add(cert.unwrap()).unwrap(); }
    roots
}

#[tokio::test]
async fn h2_spawned_streams_keep_the_client_connection_origin() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let dir = tempfile::tempdir().unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let ca = CertificateAuthority::load_or_create(&dir.path().join("origin")).unwrap();
        let engine = Engine::open_with_roots(&dir.path().join("capture"), roots(&ca)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut config = (*ca.server_config("localhost").unwrap()).clone();
        config.alpn_protocols = vec![b"h2".to_vec()];
        let server = tokio::spawn(async move {
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                let (io, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tasks.spawn(async move {
                    let io = acceptor.accept(io).await.unwrap();
                    let mut connection = h2::server::handshake(io).await.unwrap();
                    while let Some(item) = connection.accept().await {
                        let (_, mut respond) = item.unwrap();
                        respond.send_response(http::Response::builder().status(204).body(()).unwrap(), true).unwrap();
                    }
                });
            }
        });
        let handle = proxy::start(engine.clone(), 0).await.unwrap();
        let tcp = TcpStream::connect(handle.address).await.unwrap();
        let peer = tcp.local_addr().unwrap().to_string();
        let mut tcp = BufReader::new(tcp);
        tcp.write_all(format!("CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n", address.port(), address.port()).as_bytes()).await.unwrap();
        assert!(read_head(&mut tcp).await.unwrap().starts_with(b"HTTP/1.1 200"));
        let mut config = rustls::ClientConfig::builder().with_root_certificates(roots(&engine.ca)).with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];
        let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect("localhost".try_into().unwrap(), tcp).await.unwrap();
        let (sender, connection) = h2::client::handshake(tls).await.unwrap();
        let driver = tokio::spawn(async move { let _ = connection.await; });
        let mut calls = tokio::task::JoinSet::new();
        for index in 0..3 {
            let sender = sender.clone();
            calls.spawn(async move {
                let mut sender = sender.ready().await.unwrap();
                let (response, _) = sender.send_request(http::Request::builder().uri(format!("https://localhost:{}/stream-{index}", address.port())).body(()).unwrap(), true).unwrap();
                assert_eq!(response.await.unwrap().status(), 204);
            });
        }
        while let Some(result) = calls.join_next().await { result.unwrap(); }
        let flows = completed(&engine, 3).await;
        for flow in flows {
            assert_eq!(flow.client_protocol.as_deref(), Some("HTTP/2"));
            check_local(flow.traffic_origin.as_ref().unwrap(), peer.clone());
        }
        driver.abort(); handle.stop().await; server.abort();
    }).await.unwrap();
}

#[tokio::test]
async fn websocket_origin_survives_the_upgrade_and_frame_updates() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let dir = tempfile::tempdir().unwrap(); let engine = Engine::open(dir.path()).unwrap();
        let source = TcpListener::bind("127.0.0.1:0").await.unwrap(); let address = source.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (io, _) = source.accept().await.unwrap(); let mut io = BufReader::new(io);
            let head = read_head(&mut io).await.unwrap();
            let (_, _, headers) = capture_core::http1::parse_request(&head).unwrap();
            let key = headers.iter().find(|h| h.name.eq_ignore_ascii_case("sec-websocket-key")).unwrap();
            io.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n\r\n", capture_core::websocket::accept(&key.value)).as_bytes()).await.unwrap();
            let mut close = [0; 6]; io.read_exact(&mut close).await.unwrap();
            assert_eq!(&close[..2], &[0x88, 0x80]);
            io.write_all(&[0x88, 0]).await.unwrap();
        });
        let handle = proxy::start(engine.clone(), 0).await.unwrap();
        let io = TcpStream::connect(handle.address).await.unwrap(); let peer = io.local_addr().unwrap().to_string();
        let mut io = BufReader::new(io);
        io.write_all(format!("GET http://{address}/socket HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").as_bytes()).await.unwrap();
        assert!(read_head(&mut io).await.unwrap().starts_with(b"HTTP/1.1 101"));
        io.write_all(&[0x88, 0x80, 1, 2, 3, 4]).await.unwrap();
        let mut response = Vec::new(); io.read_to_end(&mut response).await.unwrap();
        let flow = completed(&engine, 1).await.pop().unwrap();
        assert!(flow.websocket.is_some());
        check_local(flow.traffic_origin.as_ref().unwrap(), peer);
        handle.stop().await; server.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn tun_bridge_does_not_misidentify_the_helper_as_the_original_process() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let dir = tempfile::tempdir().unwrap(); let engine = Engine::open(dir.path()).unwrap();
        let source = TcpListener::bind("127.0.0.1:0").await.unwrap(); let address = source.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (io, _) = source.accept().await.unwrap(); let mut io = BufReader::new(io);
            read_head(&mut io).await.unwrap();
            io.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let bridge = proxy::start_tun_bridge(engine.clone()).await.unwrap();
        let mut client = BufReader::new(TcpStream::connect(bridge.address).await.unwrap());
        client.write_all(format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes()).await.unwrap();
        assert!(read_head(&mut client).await.unwrap().starts_with(b"HTTP/1.1 200"));
        client.write_all(format!("GET / HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes()).await.unwrap();
        let mut response = Vec::new(); client.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 204"));
        let flow = completed(&engine, 1).await.pop().unwrap();
        let origin = flow.traffic_origin.unwrap();
        assert_eq!(origin.entry, "tun"); assert_eq!(origin.process_status, "tun-unavailable");
        assert!(origin.process.is_none()); assert!(origin.client_address.is_none());
        bridge.stop().await; server.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn tun_bridge_records_the_registered_android_application() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let dir = tempfile::tempdir().unwrap(); let engine = Engine::open(dir.path()).unwrap();
        let source = TcpListener::bind("127.0.0.1:0").await.unwrap(); let address = source.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (io, _) = source.accept().await.unwrap(); let mut io = BufReader::new(io);
            read_head(&mut io).await.unwrap();
            io.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let bridge = proxy::start_tun_bridge(engine.clone()).await.unwrap();
        let registry = bridge.origin_registry.as_ref().unwrap();
        registry.enable();
        let socket = TcpStream::connect(bridge.address).await.unwrap();
        registry.register(socket.local_addr().unwrap(), capture_core::model::TrafficOrigin {
            entry: "tun".into(), client_address: Some("10.111.0.2:51234".into()),
            process: None, process_status: "identified".into(),
            application: Some(capture_core::model::ApplicationInfo {
                uid: 10123, packages: vec!["example.android".into()], label: Some("Example".into()),
            }),
        });
        let mut client = BufReader::new(socket);
        client.write_all(format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes()).await.unwrap();
        assert!(read_head(&mut client).await.unwrap().starts_with(b"HTTP/1.1 200"));
        client.write_all(format!("GET / HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes()).await.unwrap();
        let mut response = Vec::new(); client.read_to_end(&mut response).await.unwrap();
        let flow = completed(&engine, 1).await.pop().unwrap();
        let origin = flow.traffic_origin.unwrap();
        assert_eq!(origin.client_address.as_deref(), Some("10.111.0.2:51234"));
        assert_eq!(origin.application.unwrap().packages, vec!["example.android"]);
        bridge.stop().await; server.await.unwrap();
    }).await.unwrap();
}
