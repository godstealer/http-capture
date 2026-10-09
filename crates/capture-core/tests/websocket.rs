use capture_core::{Engine, ca::CertificateAuthority, proxy, http1::*, websocket, replay::Stream};
use tokio::{net::{TcpListener, TcpStream}, io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader}};
use std::{sync::Arc, time::Duration};
use base64::{engine::general_purpose::STANDARD, Engine as _};

fn roots(ca: &CertificateAuthority) -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty(); let pem = std::fs::read(&ca.cert_path).unwrap();
    for cert in rustls_pemfile::certs(&mut &pem[..]) { roots.add(cert.unwrap()).unwrap(); } roots
}
fn wire(first: u8, payload: &[u8], mask: bool) -> Vec<u8> {
    let mut out = vec![first, payload.len() as u8 | if mask {128} else {0}];
    if mask { out.extend([1,2,3,4]); out.extend(payload.iter().enumerate().map(|(i,b)|b ^ [1,2,3,4][i%4])); }
    else {out.extend(payload);} out
}
async fn frame<R: AsyncRead + Unpin>(reader: &mut R, masked: bool) -> (u8, Vec<u8>) {
    let mut head=[0;2]; reader.read_exact(&mut head).await.unwrap(); assert_eq!(head[1]&128 != 0,masked);
    let mut mask=[0;4]; if masked {reader.read_exact(&mut mask).await.unwrap();}
    let mut bytes=vec![0;(head[1]&127) as usize]; reader.read_exact(&mut bytes).await.unwrap();
    if masked {for(i,b)in bytes.iter_mut().enumerate(){*b^=mask[i%4];}} (head[0],bytes)
}
#[tokio::test]
async fn ws_and_wss_relay_fragments_ping_binary_close_and_preserve_frames() {
    for (secure, stop) in [(false,false), (true,false), (false,true)] {
        tokio::time::timeout(Duration::from_secs(10), async {
            let dir=tempfile::tempdir().unwrap(); let _=rustls::crypto::ring::default_provider().install_default();
            let ca=CertificateAuthority::load_or_create(&dir.path().join("origin")).unwrap();
            let engine=Engine::open_with_roots(&dir.path().join("capture"),roots(&ca)).unwrap();
            engine.intercept.configure(capture_core::intercept::Config { request: true, response: true, scope: "all".into(), rules: vec![capture_core::intercept::Rule { host: "unrelated.invalid".into(), ..Default::default() }] }).unwrap();
            engine.scripts.configure(capture_core::scripts::Scripts { enabled: true, before: "throw new Error('unrelated')".into(), after: "throw new Error('unrelated')".into(), ..Default::default() }).unwrap();
            let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();
            let config=ca.server_config("localhost").unwrap();
            let server=tokio::spawn(async move {
                let(socket,_)=listener.accept().await.unwrap();
                let socket:Stream=if secure {Box::new(tokio_rustls::TlsAcceptor::from(config).accept(socket).await.unwrap())}else{Box::new(socket)};
                let mut socket=BufReader::new(socket);let head=read_head(&mut socket).await.unwrap();let(_,_,headers)=parse_request(&head).unwrap();
                let key=values(&headers,"sec-websocket-key").next().unwrap();
                socket.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n\r\n",websocket::accept(key)).as_bytes()).await.unwrap();
                loop {let(first,payload)=frame(&mut socket,true).await;socket.write_all(&wire(first,&payload,false)).await.unwrap();socket.flush().await.unwrap();if stop { std::future::pending::<()>().await; } if first&15==8{break;}}
            });
            let handle=proxy::start(engine.clone(),0).await.unwrap();let tcp=TcpStream::connect(handle.address).await.unwrap();
            let mut client:BufReader<Stream>=if secure {
                let mut tcp=BufReader::new(tcp);tcp.write_all(format!("CONNECT localhost:{port} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes()).await.unwrap();read_head(&mut tcp).await.unwrap();
                let mut tls=rustls::ClientConfig::builder().with_root_certificates(roots(&engine.ca)).with_no_client_auth();tls.alpn_protocols=vec![b"http/1.1".to_vec()];
                BufReader::new(Box::new(tokio_rustls::TlsConnector::from(Arc::new(tls)).connect("localhost".try_into().unwrap(),tcp).await.unwrap()) as Stream)
            }else{BufReader::new(Box::new(tcp) as Stream)};
            let target=if secure {"/ws".into()}else{format!("http://localhost:{port}/ws")};
            client.write_all(format!("GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").as_bytes()).await.unwrap();
            assert!(read_head(&mut client).await.unwrap().starts_with(b"HTTP/1.1 101"));
            let messages:Vec<(u8,Vec<u8>)>=if stop { vec![(129,b"live".to_vec())] } else { vec![(1,b"part".to_vec()),(137,b"ping".to_vec()),(138,b"pong".to_vec()),(128,b"two".to_vec()),(130,vec![0,255,1]),(136,vec![3,232])] };
            for(first,payload)in &messages{client.write_all(&wire(*first,payload,true)).await.unwrap();assert_eq!(frame(&mut client,false).await,(*first,payload.clone()));}
            if stop {
                let id = loop {
                    let flows = engine.store.list().unwrap();
                    if let Some(f) = flows.first().filter(|f|f.websocket.as_ref().unwrap().frames.len() == 2) { break f.id.clone(); }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                };
                assert!(engine.executions.cancel(&id));
                let mut rest = Vec::new(); client.read_to_end(&mut rest).await.unwrap(); assert!(rest.is_empty(), "no HTTP error bytes after Upgrade");
                let flow = engine.store.get(&id).unwrap().unwrap();
                assert_eq!(flow.websocket.as_ref().unwrap().state, "stopped");
                assert_eq!(flow.websocket.unwrap().frames.len(), 2);
                server.abort(); handle.stop().await; return;
            }
            loop {let flows=engine.store.list().unwrap();if flows.first().is_some_and(|f|f.websocket.as_ref().unwrap().state=="closed") {let f=&flows[0];assert!(f.error.is_none(),"{:?}",f.error);let frames=&f.websocket.as_ref().unwrap().frames;assert_eq!(frames.len(),messages.len()*2);for direction in ["client","server"]{let recorded:Vec<_>=frames.iter().filter(|f|f.direction==direction).map(|f|(f.opcode|if f.fin{128}else{0},STANDARD.decode(&f.payload_base64).unwrap())).collect();assert_eq!(recorded,messages);}assert_eq!(f.client_tls.is_some(),secure);break;}tokio::time::sleep(Duration::from_millis(20)).await;}
            server.await.unwrap();handle.stop().await;
        }).await.unwrap();
    }
}

#[test]
fn rfc_accept_key() {assert_eq!(websocket::accept("dGhlIHNhbXBsZSBub25jZQ=="),"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");}
