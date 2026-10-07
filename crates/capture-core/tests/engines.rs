use capture_core::{model::*, transport::*};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

struct Probe { id: &'static str, calls: Arc<AtomicUsize> }
impl EngineContract<RequestDraft,capture_core::upstream::UpstreamProxy> for Probe {
    fn id(&self) -> &'static str { self.id }
    fn profiles(&self) -> Vec<String> { vec!["chrome".into()] }
    fn send<'a>(&'a self, _: &'a RequestDraft) -> SendFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok((CapturedResponse { upstream_tls: None, sent_request_headers: None, tls_version: None, status: 204, version: "HTTP/1.1".into(), headers: vec![],
                body_base64: String::new(), raw_head_base64: None }, vec![self.id.into()]))
        })
    }
}
fn draft(engine: &str) -> RequestDraft {
    RequestDraft { upstream_profile_id: None, scripts: Default::default(), pseudo_headers: vec![], engine: engine.into(), method: "GET".into(), url: "https://localhost/".into(),
        headers: vec![], body_base64: String::new(), tls: TlsProfile { preset: "chrome".into(), ..Default::default() } }
}

#[tokio::test]
async fn requests_select_independent_engines_with_the_same_tls_profile() {
    let mut engines = SendEngines::empty();
    let a = Arc::new(AtomicUsize::new(0)); let b = Arc::new(AtomicUsize::new(0));
    engines.register(Arc::new(Probe { id: "a", calls: a.clone() })).unwrap();
    engines.register(Arc::new(Probe { id: "b", calls: b.clone() })).unwrap();
    let first = draft("a"); let second = draft("b");
    let (one, two) = tokio::join!(engines.send(&first), engines.send(&second));
    assert_eq!(one.unwrap().1, ["a"]); assert_eq!(two.unwrap().1, ["b"]);
    assert_eq!(a.load(Ordering::SeqCst), 1); assert_eq!(b.load(Ordering::SeqCst), 1);
    assert!(engines.send(&draft("missing")).await.is_err());
    let mut invalid = draft("a"); invalid.tls.preset = "native".into();
    assert!(engines.send(&invalid).await.is_err());
    assert_eq!(a.load(Ordering::SeqCst), 1); // No fallback or incompatible call.
}

#[tokio::test]
async fn unavailable_engine_is_explicit_and_missing_engine_defaults_to_auto() {
    let engines = SendEngines::with_roots(rustls::RootCertStore::empty());
    let helper = engines.list().into_iter().find(|e| e.id == "httpcloak").unwrap();
    if !helper.available { assert!(engines.send(&draft("httpcloak")).await.unwrap_err().to_string().contains("unavailable")); }
    else { assert!(!helper.browser_versions.is_empty()); }
    let legacy: RequestDraft = serde_json::from_str(r#"{"method":"GET","url":"http://localhost/","headers":[],"bodyBase64":""}"#).unwrap();
    assert_eq!(legacy.engine, "auto");
}
