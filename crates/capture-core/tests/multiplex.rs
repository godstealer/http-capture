use capture_core::{ca::CertificateAuthority, model::*, multiplex, proxy, Engine};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use bytes::{Buf, Bytes};
use std::{sync::Arc, time::Duration};
use tokio::{io::AsyncWriteExt, net::{TcpListener, TcpStream}, io::BufReader};

fn roots(ca: &CertificateAuthority) -> rustls::RootCertStore {
    let pem = std::fs::read(&ca.cert_path).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &pem[..]) { roots.add(cert.unwrap()).unwrap(); }
    roots
}
fn draft(engine: &str, url: String) -> RequestDraft {
    RequestDraft { upstream_profile_id: None, scripts: Default::default(), pseudo_headers: vec![], engine: engine.into(), method: "POST".into(), url,
        headers: vec![Header { name: "x-test".into(), value: "one".into() }, Header { name: "x-test".into(), value: "two".into() }],
        body_base64: STANDARD.encode(vec![255; 100_000]), tls: TlsProfile::default() }
}

#[tokio::test]
async fn auto_uses_h1_when_server_selects_h1_or_has_no_alpn() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let dir = tempfile::tempdir().unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let ca = CertificateAuthority::load_or_create(dir.path()).unwrap();
        for alpn in [vec![b"http/1.1".to_vec()], vec![]] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let mut config = (*ca.server_config("localhost").unwrap()).clone(); config.alpn_protocols = alpn;
            let server = tokio::spawn(async move {
                let (io, _) = listener.accept().await.unwrap();
                let mut io = BufReader::new(tokio_rustls::TlsAcceptor::from(Arc::new(config)).accept(io).await.unwrap());
                let head = capture_core::http1::read_head(&mut io).await.unwrap();
                let (_, _, fields) = capture_core::http1::parse_request(&head).unwrap();
                let body = capture_core::http1::read_body(&mut io, &fields, false).await.unwrap().0;
                assert_eq!(body.len(), 100_000);
                io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
            });
            let request = draft("auto", format!("https://localhost:{}/", address.port()));
            let (response, _) = capture_core::replay::send_with_roots(&request, &roots(&ca)).await.unwrap();
            assert_eq!(response.version, "HTTP/1.1"); assert_eq!(response.status, 200);
            assert_eq!(response.tls_version.as_deref(), Some("TLS 1.3"));
            server.await.unwrap();
        }
    }).await.unwrap();
}

#[tokio::test]
async fn h2_verified_capture_multiplexes_and_handles_flow_control() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let dir = tempfile::tempdir().unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let ca = CertificateAuthority::load_or_create(&dir.path().join("origin")).unwrap();
        let engine = Engine::open_with_roots(&dir.path().join("capture"), roots(&ca)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut config = (*ca.server_config("localhost").unwrap()).clone();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let config = Arc::new(config);
        let server = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            for _ in 0..2 {
                let (io, _) = listener.accept().await.unwrap(); let config = config.clone();
                tasks.spawn(async move {
                    let io = tokio_rustls::TlsAcceptor::from(config).accept(io).await.unwrap();
                    let mut conn = h2::server::handshake(io).await.unwrap();
                    let mut streams = tokio::task::JoinSet::new();
                    while let Some(request) = conn.accept().await {
                        let (request, mut respond) = request.unwrap();
                        streams.spawn(async move {
                            assert_eq!(request.headers().get_all("x-test").iter().count(), 2);
                            assert_eq!(request.extensions().get::<h2::ext::HeaderOrder>().unwrap().0,
                                [":path", ":authority", ":method", ":scheme", "x-test", "x-middle", "x-test", "te"]);
                            assert!(!request.headers().contains_key("content-length"));
                            assert_eq!(request.headers()["te"], "trailers");
                            let body = multiplex::recv_h2_body(&mut request.into_body()).await.unwrap();
                            let mut send = respond.send_response(http::Response::builder().status(200).body(()).unwrap(), false).unwrap();
                            multiplex::send_h2_body(&mut send, Bytes::from(body)).await.unwrap();
                        });
                    }
                });
            }
            while let Some(result) = tasks.join_next().await { result.unwrap(); }
        });
        let handle = proxy::start(engine.clone(), 0).await.unwrap();
        let mut tcp = TcpStream::connect(handle.address).await.unwrap();
        tcp.write_all(format!("CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n", address.port(), address.port()).as_bytes()).await.unwrap();
        let mut reader = BufReader::new(tcp);
        assert!(capture_core::http1::read_head(&mut reader).await.unwrap().starts_with(b"HTTP/1.1 200"));
        let mut tls = rustls::ClientConfig::builder().with_root_certificates(roots(&engine.ca)).with_no_client_auth();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        let io = tokio_rustls::TlsConnector::from(Arc::new(tls)).connect("localhost".try_into().unwrap(), reader).await.unwrap();
        assert_eq!(io.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
        let (sender, connection) = h2::client::handshake(io).await.unwrap();
        let driver = tokio::spawn(async move { let _ = connection.await; });
        let mut calls = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let sender = sender.clone();
            calls.spawn(async move {
                let mut sender = sender.ready().await.unwrap();
                let mut request = http::Request::builder().method("POST").uri(format!("https://localhost:{}/echo", address.port())).header("x-test", "one").header("x-middle", "between").header("x-test", "two").header("te", "trailers").body(()).unwrap();
                request.extensions_mut().insert(h2::ext::HeaderOrder([":path", ":authority", ":method", ":scheme", "x-test", "x-middle", "x-test", "te"].map(str::to_owned).to_vec()));
                let (response, mut send) = sender.send_request(request, false).unwrap();
                multiplex::send_h2_body(&mut send, Bytes::from(vec![255; 100_000])).await.unwrap();
                let response = response.await.unwrap(); assert_eq!(response.status(), 200);
                assert_eq!(multiplex::recv_h2_body(&mut response.into_body()).await.unwrap(), vec![255; 100_000]);
            });
        }
        while let Some(result) = calls.join_next().await { result.unwrap(); }
        let flows = engine.store.list().unwrap(); assert_eq!(flows.len(), 2);
        for flow in &flows {
            let original: Vec<_> = flow.request.pseudo_headers.iter().chain(&flow.request.headers).cloned().collect();
            assert_eq!(flow.response.as_ref().unwrap().sent_request_headers.as_ref().unwrap(), &original);
            assert_eq!(flow.request.engine, "h2");
            assert_eq!(flow.client_tls.as_ref().unwrap().alpn.as_deref(), Some("h2"));
            assert!(flow.client_tls.as_ref().unwrap().offered_alpn.contains(&"h2".to_string()));
            assert_eq!(flow.response.as_ref().unwrap().upstream_tls.as_ref().unwrap().alpn.as_deref(), Some("h2"));
        }
        assert!(flows.iter().all(|f| f.client_protocol.as_deref() == Some("HTTP/2") && f.response.as_ref().unwrap().version == "HTTP/2" && f.raw_request_head_base64.is_none()));
        driver.abort(); handle.stop().await; server.abort();
    }).await.unwrap();
}

#[tokio::test]
async fn h3_client_verifies_ca_and_roundtrips_binary_data() {
    h3_roundtrip(false).await;
}
#[tokio::test]
async fn h3_socks_udp_roundtrips_binary_data_with_authentication() {
    h3_roundtrip(true).await;
}
async fn h3_roundtrip(via_socks: bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        let dir = tempfile::tempdir().unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let ca = CertificateAuthority::load_or_create(dir.path()).unwrap();
        let mut tls = (*ca.server_config("127.0.0.1").unwrap()).clone(); tls.alpn_protocols = vec![b"h3".to_vec()];
        let endpoint = quinn::Endpoint::server(quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap())), "127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let conn = endpoint.accept().await.unwrap().await.unwrap();
            let mut server = h3::server::builder().build(h3_quinn::Connection::new(conn)).await.unwrap();
            let resolver = server.accept().await.unwrap().unwrap();
            let worker = tokio::spawn(async move {
                let (request, mut stream) = resolver.resolve_request().await.unwrap();
                assert_eq!(request.headers().get_all("x-test").iter().count(), 2);
                let mut body = Vec::new();
                while let Some(mut chunk) = stream.recv_data().await.unwrap() { body.extend_from_slice(&chunk.copy_to_bytes(chunk.remaining())); }
                stream.send_response(http::Response::builder().status(200).body(()).unwrap()).await.unwrap();
                stream.send_data(Bytes::from(body)).await.unwrap(); stream.finish().await.unwrap();
            });
            let _ = server.accept().await; worker.await.unwrap();
        });
        let engine = Engine::open_with_roots(&dir.path().join("capture"), roots(&ca)).unwrap();
        let (ready, listen) = tokio::sync::oneshot::channel();
        let proxy_engine = engine.clone();
        let proxy = tokio::spawn(async move { capture_core::quic_proxy::run(proxy_engine, format!("https://{address}/").parse().unwrap(), 0, Some(ready)).await.unwrap(); });
        let proxy_address = listen.await.unwrap();
        let request = draft("h3", format!("https://{proxy_address}/echo"));
        let state = capture_core::upstream::UpstreamState::default();
        let relay_task = if via_socks {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            state.update(capture_core::upstream::UpstreamInput { enabled:true, url:format!("socks5://{}",listener.local_addr().unwrap()), username:"tester".into(), password:Some("secret".into()), auth_enabled:true }).unwrap();
            Some(tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let (mut tcp, _) = listener.accept().await.unwrap();
                let mut hello=[0;3]; tcp.read_exact(&mut hello).await.unwrap(); assert_eq!(hello,[5,1,2]);
                tcp.write_all(&[5,2]).await.unwrap();
                let mut auth=[0;15]; tcp.read_exact(&mut auth).await.unwrap(); assert_eq!(&auth,b"\x01\x06tester\x06secret");
                tcp.write_all(&[1,0]).await.unwrap();
                let mut command=[0;10]; tcp.read_exact(&mut command).await.unwrap(); assert_eq!(command,[5,3,0,1,0,0,0,0,0,0]);
                let udp=tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
                let mut reply=vec![5,0,0,1,127,0,0,1]; reply.extend(udp.local_addr().unwrap().port().to_be_bytes());
                tcp.write_all(&reply).await.unwrap();
                let mut client=None; let mut buffer=vec![0;65535]; let mut control=[0;1];
                loop { tokio::select! {
                    _=tcp.read(&mut control) => break,
                    result=udp.recv_from(&mut buffer) => {
                        let (n,peer)=match result { Ok(value)=>value, Err(error) if error.kind()==std::io::ErrorKind::ConnectionReset=>continue, Err(error)=>panic!("{error}") };
                        if peer==proxy_address {
                            let mut packet=vec![0,0,0,1,127,0,0,1]; packet.extend(proxy_address.port().to_be_bytes()); packet.extend(&buffer[..n]);
                            udp.send_to(&packet,client.unwrap()).await.unwrap();
                        } else {
                            client=Some(peer); assert_eq!(&buffer[..8], &[0,0,0,1,127,0,0,1]);
                            assert_eq!(&buffer[8..10],&proxy_address.port().to_be_bytes());
                            udp.send_to(&buffer[10..n],proxy_address).await.unwrap();
                        }
                    }
                }}
            }))
        } else { None };
        let (response, _) = multiplex::h3_send_via(&request, &roots(&engine.ca), state.snapshot().as_ref()).await.unwrap();
        assert_eq!(response.version, "HTTP/3"); assert_eq!(response.status, 200);
        assert_eq!(response.body_base64, request.body_base64);
        let flows = engine.store.list().unwrap(); assert_eq!(flows.len(), 1);
        assert_eq!(flows[0].client_protocol.as_deref(), Some("HTTP/3"));
        assert_eq!(flows[0].response.as_ref().unwrap().version, "HTTP/3");
        proxy.abort();
        if let Some(task)=relay_task { tokio::time::timeout(Duration::from_secs(2),task).await.unwrap().unwrap(); }
        server.abort();
    }).await.unwrap();
}

#[tokio::test]
async fn raw_hpack_continuation_and_dynamic_index_preserve_order() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut wire, io) = tokio::io::duplex(65536);
        let server = tokio::spawn(async move {
            let mut conn = h2::server::handshake(io).await.unwrap();
            for _ in 0..2 {
                let (request, _respond) = conn.accept().await.unwrap().unwrap();
                assert_eq!(request.extensions().get::<h2::ext::HeaderOrder>().unwrap().0,
                    [":path", ":method", ":scheme", ":authority", "x-a", "x-b", "x-a"]);
                assert_eq!(request.headers().get_all("x-a").iter().map(|v| v.to_str().unwrap()).collect::<Vec<_>>(), ["one", "three"]);
            }
        });
        fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
            let mut bytes = vec![(payload.len() >> 16) as u8, (payload.len() >> 8) as u8, payload.len() as u8, kind, flags];
            bytes.extend_from_slice(&stream.to_be_bytes()); bytes.extend_from_slice(payload); bytes
        }
        // Independent RFC 7541 encoding: indexed static pseudo fields, then
        // literal incremental-indexed fields. No encoder under test is used.
        let mut block = vec![0x84, 0x82, 0x87, 0x41, 9]; block.extend_from_slice(b"localhost");
        for (name, value) in [("x-a", "one"), ("x-b", "two"), ("x-a", "three")] {
            block.extend_from_slice(&[0x40, name.len() as u8]); block.extend_from_slice(name.as_bytes());
            block.push(value.len() as u8); block.extend_from_slice(value.as_bytes());
        }
        wire.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").await.unwrap();
        wire.write_all(&frame(4, 0, 0, &[])).await.unwrap();
        wire.write_all(&frame(1, 1, 1, &block[..8])).await.unwrap();
        wire.write_all(&frame(9, 4, 1, &block[8..])).await.unwrap();
        // Dynamic indices now point to authority=65, x-a(one)=64,
        // x-b(two)=63, x-a(three)=62, preserving the original sequence.
        wire.write_all(&frame(1, 5, 3, &[0x84, 0x82, 0x87, 0xc1, 0xc0, 0xbf, 0xbe])).await.unwrap();
        server.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn h2_rejects_invalid_outgoing_header_order() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (io, peer) = tokio::io::duplex(65536);
        let server = tokio::spawn(async move {
            let mut connection = h2::server::handshake(peer).await.unwrap();
            while let Some(_request) = connection.accept().await {}
        });
        let (sender, connection) = h2::client::handshake(io).await.unwrap();
        let driver = tokio::spawn(async move { let _ = connection.await; });
        for order in [vec![":method"], vec!["x-test", ":method", ":scheme", ":authority", ":path"],
            vec![":method", ":scheme", ":authority", ":path", "x-test", "x-test"]] {
            let mut sender = sender.clone().ready().await.unwrap();
            let mut request = http::Request::builder().uri("https://localhost/").header("x-test", "one").body(()).unwrap();
            request.extensions_mut().insert(h2::ext::HeaderOrder(order.into_iter().map(str::to_owned).collect()));
            assert!(sender.send_request(request, true).is_err());
        }
        driver.abort(); server.abort();
    }).await.unwrap();
}
