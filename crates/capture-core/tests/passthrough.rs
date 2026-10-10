use capture_core::{Engine, decryption, http1::read_head, proxy};
use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}};

#[tokio::test]
async fn connect_passthrough_preserves_buffered_bytes_and_records_only_tunnel() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::open(directory.path()).unwrap();
        decryption::save(&engine.store, vec!["127.0.0.1".into()]).unwrap();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = origin.local_addr().unwrap();
        // Opaque TLS-shaped bytes: MITM parsing would reject this fixture.
        let payload = b"\x16\x03\x03\x00\x05hello";
        let server = tokio::spawn(async move {
            let (mut socket, _) = origin.accept().await.unwrap();
            let mut received = vec![0; payload.len()];
            socket.read_exact(&mut received).await.unwrap();
            assert_eq!(received, payload);
            socket.write_all(b"opaque reply").await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let handle = proxy::start(engine.clone(), 0).await.unwrap();
        let mut client = BufReader::new(TcpStream::connect(handle.address).await.unwrap());
        let mut request = format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").into_bytes();
        request.extend_from_slice(payload);
        client.write_all(&request).await.unwrap();
        assert!(read_head(&mut client).await.unwrap().starts_with(b"HTTP/1.1 200"));
        client.shutdown().await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(response, b"opaque reply");
        server.await.unwrap();
        loop {
            let records = engine.store.list().unwrap();
            if records.first().is_some_and(|f| f.notes.iter().any(|n| n.starts_with("Tunnel bytes:"))) {
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].request.method, "CONNECT");
                assert!(records[0].response.is_none());
                assert!(records[0].client_tls.is_none());
                assert!(records[0].error.is_none());
                break;
            }
            tokio::task::yield_now().await;
        }
        handle.stop().await;
    }).await.expect("passthrough stalled");
}
