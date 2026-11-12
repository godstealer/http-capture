use base64::Engine as _;
use capture_core::{model::RequestDraft, transport::SendEngines};

#[tokio::test]
async fn custom_engine_uses_versioned_stdio_request_and_response() {
    if std::env::var_os("HTTP_CAPTURE_CUSTOM_ENGINE").is_none() {
        eprintln!("Build the custom engine fixture to run this integration test");
        return;
    }
    let engines = SendEngines::with_roots(rustls::RootCertStore::empty());
    let info = engines.list().into_iter().find(|engine| engine.id == "custom").unwrap();
    assert!(info.available, "{:?}", info.reason);
    assert_eq!(info.profiles, ["native"]);
    let draft: RequestDraft = serde_json::from_value(serde_json::json!({
        "engine":"custom", "method":"POST", "url":"https://fixture.invalid/path",
        "headers":[{"name":"X-Order","value":"one"},{"name":"x-order","value":"two"}],
        "bodyBase64":"AAEC", "tls":{"preset":"native"}
    })).unwrap();
    let (response, notes) = engines.send(&draft).await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.version, "HTTP/1.1");
    let body = base64::engine::general_purpose::STANDARD.decode(response.body_base64.as_bytes()).unwrap();
    assert_eq!(String::from_utf8(body).unwrap(),
        "custom engine received POST https://fixture.invalid/path using native; bodyBase64=AAEC");
    assert!(notes.iter().any(|note| note.contains("no network request")));
}
