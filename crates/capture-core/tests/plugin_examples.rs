use base64::{Engine as _, engine::general_purpose::STANDARD};
use capture_core::{Engine, model::*, plugins::{Plugin, validate_plugin}, transport::*};
use std::sync::{Arc, Mutex};

const EXAMPLES: [&str; 4] = [
    include_str!("../../../examples/plugins/demo-header.json"),
    include_str!("../../../examples/plugins/local-request-id.json"),
    include_str!("../../../examples/plugins/example-json-response.json"),
    include_str!("../../../examples/plugins/local-no-cache.json"),
];

struct Probe {
    calls: Arc<Mutex<Vec<RequestDraft>>>,
    response: Arc<Mutex<CapturedResponse>>,
}
impl EngineContract<RequestDraft, capture_core::upstream::UpstreamProxy> for Probe {
    fn id(&self) -> &'static str { "probe" }
    fn profiles(&self) -> Vec<String> { vec!["native".into()] }
    fn send<'a>(&'a self, request: &'a RequestDraft) -> SendFuture<'a> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(request.clone());
            Ok((self.response.lock().unwrap().clone(), vec![]))
        })
    }
}
fn header(name: &str, value: &str) -> Header {
    Header { name: name.into(), value: value.into(), disabled: false }
}
fn response(bytes: &[u8], headers: Vec<Header>) -> CapturedResponse {
    CapturedResponse {
        status: 200, version: "HTTP/1.1".into(), headers, trailers: vec![],
        body_base64: STANDARD.encode(bytes).into(), raw_head_base64: None,
        upstream_tls: None, tls_version: None, sent_request_headers: None,
    }
}
fn engine(reply: CapturedResponse) -> (tempfile::TempDir, Arc<Engine>, Arc<Mutex<Vec<RequestDraft>>>) {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let mut engines = SendEngines::empty();
    engines.register(Arc::new(Probe { calls: calls.clone(), response: Arc::new(Mutex::new(reply)) })).unwrap();
    let engine = Engine::open_with_engines(dir.path(), rustls::RootCertStore::empty(), engines).unwrap();
    (dir, engine, calls)
}
fn draft(url: &str, headers: Vec<Header>) -> RequestDraft {
    let mut request: RequestDraft = serde_json::from_str(r#"{"method":"POST","url":"http://127.0.0.1/","engine":"probe","headers":[],"bodyBase64":"AP9iaW5hcnk="}"#).unwrap();
    request.url = url.into(); request.headers = headers; request
}
fn example(id: &str) -> Plugin {
    EXAMPLES.iter().map(|json| serde_json::from_str::<Plugin>(json).unwrap()).find(|plugin| plugin.id == id).unwrap()
}
fn enable(engine: &Engine, id: &str) {
    let mut plugin = example(id); plugin.enabled = true; engine.plugins.save(vec![plugin]).unwrap();
}

#[test]
fn all_packages_validate_install_disabled_and_persist() {
    let (dir, engine, _) = engine(response(b"", vec![]));
    let plugins: Vec<Plugin> = EXAMPLES.iter().map(|json| serde_json::from_str(json).unwrap()).collect();
    assert!(plugins.iter().all(|plugin| !plugin.enabled));
    for plugin in &plugins { validate_plugin(plugin).unwrap(); }
    engine.plugins.save(plugins).unwrap();
    assert_eq!(engine.plugins.list().len(), EXAMPLES.len());
    drop(engine);
    let reopened = Engine::open(dir.path()).unwrap();
    assert_eq!(reopened.plugins.list().len(), EXAMPLES.len());
    assert!(reopened.plugins.list().iter().all(|plugin| !plugin.enabled));
}

#[tokio::test]
async fn request_id_replaces_case_insensitive_duplicates_and_respects_scope() {
    let (_dir, engine, calls) = engine(response(b"unchanged", vec![]));
    enable(&engine, "local-request-id");
    let request = draft("http://localhost:8042/echo", vec![header("x-debug-request-id", "old"), header("X-Keep", "one"), header("X-Debug-Request-Id", "also-old")]);
    let flow = engine.execute(request.clone(), None, "replay", None).await.unwrap();
    assert!(flow.error.is_none(), "{:?}", flow.error);
    let sent = calls.lock().unwrap()[0].clone();
    let ids: Vec<_> = sent.headers.iter().filter(|h| h.name.eq_ignore_ascii_case("x-debug-request-id")).collect();
    assert_eq!(ids.len(), 1); uuid::Uuid::parse_str(&ids[0].value).unwrap();
    assert_eq!(sent.body_base64, request.body_base64);
    assert!(sent.headers.iter().any(|h| h.name == "X-Keep" && h.value == "one"));
    assert_eq!(flow.response.unwrap().body_base64.as_bytes(), STANDARD.encode(b"unchanged").as_bytes());
    engine.execute(request, None, "capture", None).await.unwrap();
    assert_eq!(calls.lock().unwrap()[1].headers.iter().filter(|h| h.name.eq_ignore_ascii_case("x-debug-request-id")).count(), 2);
    assert!(engine.plugins.logs().iter().any(|line| line.contains("added X-Debug-Request-Id")));
}

#[tokio::test]
async fn json_response_preserves_unicode_and_recomputes_entity_length() {
    let original = "{\"message\":\"你好 🌏\",\"count\":2}".as_bytes();
    let (_dir, engine, _) = engine(response(original, vec![header("Content-Type", "application/problem+json; charset=UTF-8"), header("Content-Length", &original.len().to_string())]));
    enable(&engine, "example-json-response");
    let flow = engine.execute(draft("https://example.invalid:8443/api/item", vec![]), None, "replay", None).await.unwrap();
    assert!(flow.error.is_none(), "{:?}", flow.error);
    let changed = flow.response.unwrap();
    let bytes = STANDARD.decode(changed.body_base64.as_bytes()).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["message"], "你好 🌏"); assert_eq!(json["count"], 2);
    assert_eq!(json["debugPlugin"], "example-json-response");
    assert!(changed.headers.iter().any(|h| h.name.eq_ignore_ascii_case("content-length") && h.value == bytes.len().to_string()));
    assert_eq!(flow.original_response.unwrap().body_base64.as_bytes(), STANDARD.encode(original).as_bytes());
}

#[tokio::test]
async fn json_response_skips_unsupported_entities_without_corrupting_bytes() {
    for (bytes, headers, reason) in [
        (&b"\x00\xffcompressed"[..], vec![header("Content-Type", "application/json"), header("Content-Encoding", "br")], "encoded response"),
        (&b"plain"[..], vec![header("Content-Type", "text/plain")], "not JSON"),
        (&b"{\"value\":1}"[..], vec![header("Content-Type", "application/json; charset=shift_jis")], "not UTF-8"),
        (&b""[..], vec![header("Content-Type", "application/json")], "empty response"),
        (&b"[1,2]"[..], vec![header("Content-Type", "application/json")], "not an object"),
    ] {
        let (_dir, engine, _) = engine(response(bytes, headers));
        enable(&engine, "example-json-response");
        let flow = engine.execute(draft("http://example.invalid/api/item", vec![]), None, "replay", None).await.unwrap();
        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert_eq!(flow.response.unwrap().body_base64.as_bytes(), STANDARD.encode(bytes).as_bytes());
        assert!(engine.plugins.logs().iter().any(|line| line.contains(reason)), "{}", engine.plugins.logs().join("\n"));
    }
}

#[tokio::test]
async fn json_parse_failure_rolls_back_and_records_error() {
    let (_dir, engine, _) = engine(response(b"{broken", vec![header("Content-Type", "application/json")]));
    enable(&engine, "example-json-response");
    let flow = engine.execute(draft("http://example.invalid/api/item", vec![]), None, "replay", None).await.unwrap();
    assert!(flow.error.is_none());
    assert_eq!(flow.response.unwrap().body_base64.as_bytes(), STANDARD.encode(b"{broken").as_bytes());
    assert!(engine.plugins.logs().iter().any(|line| line.contains("SyntaxError")));
}

#[tokio::test]
async fn cache_example_changes_only_matched_cache_headers_for_both_sources() {
    let bytes = b"\x00\xfforiginal";
    let (_dir, engine, calls) = engine(response(bytes, vec![header("cache-control", "public"), header("ETag", "old"), header("Last-Modified", "old"), header("Expires", "old"), header("X-Keep", "yes")]));
    enable(&engine, "local-no-cache");
    for source in ["capture", "replay"] {
        let request = draft("http://127.0.0.1:8080/demo/item", vec![header("If-None-Match", "old"), header("if-modified-since", "old"), header("Cache-Control", "max-age=99"), header("Pragma", "old"), header("X-Keep", "one"), header("X-Keep", "two")]);
        let flow = engine.execute(request.clone(), None, source, None).await.unwrap();
        assert!(flow.error.is_none(), "{:?}", flow.error);
        let sent = calls.lock().unwrap().last().unwrap().clone();
        assert!(!sent.headers.iter().any(|h| h.name.eq_ignore_ascii_case("if-none-match") || h.name.eq_ignore_ascii_case("if-modified-since")));
        assert_eq!(sent.headers.iter().filter(|h| h.name.eq_ignore_ascii_case("cache-control")).map(|h| h.value.as_str()).collect::<Vec<_>>(), vec!["no-cache"]);
        assert_eq!(sent.headers.iter().filter(|h| h.name == "X-Keep").map(|h| h.value.as_str()).collect::<Vec<_>>(), vec!["one", "two"]);
        assert_eq!(sent.body_base64, request.body_base64);
        let reply = flow.response.unwrap();
        assert_eq!(reply.body_base64.as_bytes(), STANDARD.encode(bytes).as_bytes());
        assert!(!reply.headers.iter().any(|h| ["etag", "last-modified", "expires"].iter().any(|name| h.name.eq_ignore_ascii_case(name))));
        assert_eq!(reply.headers.iter().filter(|h| h.name.eq_ignore_ascii_case("cache-control")).map(|h| h.value.as_str()).collect::<Vec<_>>(), vec!["no-store"]);
    }
    let unmatched = engine.execute(draft("http://localhost:8080/elsewhere", vec![]), None, "replay", None).await.unwrap();
    assert!(unmatched.response.unwrap().headers.iter().any(|h| h.name == "ETag"));
}

#[tokio::test]
async fn original_header_demo_executes_before_and_after_for_capture_and_replay() {
    let (_dir, engine, calls) = engine(response(b"original", vec![]));
    enable(&engine, "demo-header");
    for source in ["capture", "replay"] {
        let flow = engine.execute(draft("http://127.0.0.1:8080/", vec![]), None, source, None).await.unwrap();
        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert!(calls.lock().unwrap().last().unwrap().headers.iter().any(|h| h.name == "X-Plugin-Demo" && h.value == "hello"));
        assert!(flow.response.unwrap().headers.iter().any(|h| h.name == "X-Plugin-Response" && h.value == "hello"));
    }
}
