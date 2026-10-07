//! All real adapters use the same origin-side assertions. These H1 baseline
//! checks do not certify ClientHello or HTTP/2 fingerprints.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use capture_core::{http1::{parse_request, read_body, read_head}, model::*, transport::*};
use tokio::{io::{AsyncWriteExt, BufReader}, net::TcpListener};

async fn h1_contract(engine: &dyn SendEngine, profile: &str) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let observer = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(tcp);
        let head = read_head(&mut stream).await.unwrap();
        let (method, target, headers) = parse_request(&head).unwrap();
        let body = read_body(&mut stream, &headers, false).await.unwrap().0;
        stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /must-not-follow\r\nContent-Length: 4\r\nConnection: close\r\n\r\n\x00\xff\x80\x2a").await.unwrap();
        (method, target, headers, body)
    });
    let request = RequestDraft { upstream_profile_id: None, scripts: Default::default(), pseudo_headers: vec![], engine: engine.id().into(), method: "POST".into(),
        url: format!("http://{address}/echo?q=1"), headers: vec![
            Header { name: "X-First".into(), value: "1".into() },
            Header { name: "X-Second".into(), value: "2".into() },
        ], body_base64: STANDARD.encode([0, 255, 128, 42]),
        tls: TlsProfile { preset: profile.into(), ..Default::default() } };
    let before = serde_json::to_value(&request).unwrap();
    let response = engine.send(&request).await.unwrap().0;
    let (method, target, headers, body) = observer.await.unwrap();
    assert_eq!(method, "POST"); assert_eq!(target, "/echo?q=1");
    let observed: Vec<_> = headers.iter().filter(|h| h.name.to_ascii_lowercase().starts_with("x-"))
        .map(|h| (h.name.as_str(), h.value.as_str())).collect();
    assert_eq!(observed, [("X-First", "1"), ("X-Second", "2")], "{} header fidelity", engine.id());
    assert_eq!(body, [0, 255, 128, 42]);
    assert_eq!(response.status, 302, "redirect must not be followed");
    assert_eq!(STANDARD.decode(response.body_base64).unwrap(), [0, 255, 128, 42]);
    assert_eq!(serde_json::to_value(request).unwrap(), before);
}

#[tokio::test]
async fn native_h1_fidelity() {
    tokio::time::timeout(std::time::Duration::from_secs(5),
        h1_contract(&NativeEngine::new(rustls::RootCertStore::empty()), "native")).await.unwrap();
}

#[cfg(feature = "browser-replay")]
#[tokio::test]
async fn wreq_h1_fidelity() {
    for profile in ["chrome", "firefox"] {
        tokio::time::timeout(std::time::Duration::from_secs(5),
            h1_contract(&WreqEngine, profile)).await.unwrap();
    }
}

#[tokio::test]
async fn native_reports_exact_sent_order_with_interleaved_duplicates() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let observer = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(tcp);
            let raw = read_head(&mut stream).await.unwrap();
            let (_, _, fields) = parse_request(&raw).unwrap();
            assert_eq!(read_body(&mut stream, &fields, false).await.unwrap().0, b"abc");
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await.unwrap();
            fields
        });
        let fields = [("X-A", "1"), ("hOsT", "old.test"), ("X-B", "2"), ("x-a", "3"),
            ("content-length", "999"), ("Connection", "X-Remove"), ("X-Remove", "gone"), ("Expect", "100-continue")];
        let draft = RequestDraft { upstream_profile_id: None, scripts: Default::default(), pseudo_headers: vec![], engine: "native".into(), method: "POST".into(), url: format!("http://{address}/"),
            headers: fields.into_iter().map(|(n,v)| Header { name:n.into(), value:v.into() }).collect(),
            body_base64: STANDARD.encode(b"abc"), tls: TlsProfile::default() };
        let response = NativeEngine::new(rustls::RootCertStore::empty()).send(&draft).await.unwrap().0;
        let actual = observer.await.unwrap();
        assert_eq!(response.sent_request_headers.as_ref().unwrap(), &actual);
        assert_eq!(actual.iter().map(|h|h.name.as_str()).collect::<Vec<_>>(), ["X-A", "hOsT", "X-B", "x-a", "content-length", "Connection"]);
        assert_eq!(actual[1].value, address.to_string());
        assert_eq!(actual[4].value, "3");
        assert!(response.tls_version.is_none());
    }).await.unwrap();
}
