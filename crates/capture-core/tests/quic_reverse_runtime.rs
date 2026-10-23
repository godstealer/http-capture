use capture_core::{runtime::CaptureRuntime, Engine};

#[tokio::test]
async fn fixed_target_listener_lifecycle_and_validation() {
    let directory = tempfile::tempdir().unwrap();
    let engine = Engine::open_with_roots(directory.path(), rustls::RootCertStore::empty()).unwrap();
    let runtime = CaptureRuntime::new(engine);

    assert!(!runtime.reverse_quic_status().await.running);
    for invalid in ["http://example.test/", "https://example.test/path", "https://user@example.test/"] {
        assert!(runtime.start_reverse_quic(invalid.into(), 0).await.is_err());
    }
    let started = runtime.start_reverse_quic("https://example.test/".into(), 0).await.unwrap();
    assert!(started.running);
    assert_eq!(started.target.as_deref(), Some("https://example.test/"));
    assert!(started.address.as_deref().unwrap().starts_with("127.0.0.1:"));
    assert!(runtime.start_reverse_quic("https://example.test/".into(), 0).await.is_err());
    let stopped = runtime.stop_reverse_quic().await.unwrap();
    assert!(!stopped.running);
    assert!(stopped.address.is_none());
    let restarted = runtime.start_reverse_quic("https://example.test/".into(), 0).await.unwrap();
    assert!(restarted.running);
    runtime.shutdown_network().await.unwrap();
    assert!(!runtime.reverse_quic_status().await.running);
}
