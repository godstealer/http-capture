use capture_core::{
    http1::{parse_request, read_head},
    intercept::{Config, Decision, Rule},
    model::{Header, RequestDraft},
    store::{Collection, Library, RequestEditor, SavedRequest, Workspace},
    Engine,
};
use std::time::Duration;
use tokio::{io::{AsyncWriteExt, BufReader}, net::TcpListener};

fn draft(url: String, engine: &str) -> RequestDraft {
    serde_json::from_value(serde_json::json!({
        "method": "GET", "url": url, "engine": engine, "bodyBase64": "",
        "headers": [
            {"name":"X-Keep","value":"one"},
            {"name":"X-Off","value":"hidden","disabled":true},
            {"name":"X-Keep","value":"two"},
            {"name":"Connection","value":"X-Live","disabled":true},
            {"name":"X-Live","value":"still present"},
            {"name":"","value":"invalid\n{{unresolved}}","disabled":true},
            {"name":"te","value":"trailers","disabled":true}
        ]
    })).unwrap()
}

fn assert_enabled(fields: &[Header]) {
    assert!(fields.iter().all(|h| !h.disabled));
    assert!(!fields.iter().any(|h| matches!(h.name.to_ascii_lowercase().as_str(), "x-off" | "x-script-off" | "x-intercept-off" | "te")));
    assert!(fields.iter().any(|h| h.name.eq_ignore_ascii_case("x-live")));
    assert_eq!(fields.iter().filter(|h| h.name.eq_ignore_ascii_case("x-keep")).map(|h| h.value.as_str()).collect::<Vec<_>>(), ["one", "two"]);
}

#[test]
fn workspace_and_library_preserve_disabled_rows_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let request = draft(String::new(), "auto");
    let old: Header = serde_json::from_str(r#"{"name":"X-Legacy","value":"enabled"}"#).unwrap();
    assert!(!old.disabled);
    assert!(serde_json::to_value(old).unwrap().get("disabled").is_none());
    {
        let engine = Engine::open(dir.path()).unwrap();
        engine.store.save_workspace(Workspace {
            editors: vec![RequestEditor { id:"editor".into(), draft:request.clone(), selected:None, parent_id:None, grpc_draft:None }],
            active_editor:"editor".into(), revision:0,
        }).unwrap();
        engine.store.save_library(Library {
            collections:vec![Collection { id:"collection".into(), name:"Saved".into() }],
            requests:vec![SavedRequest { id:"saved".into(), name:"Request".into(), collection_id:"collection".into(), draft:request.clone() }],
            ..Default::default()
        }).unwrap();
    }
    let engine = Engine::open(dir.path()).unwrap();
    for restored in [engine.store.workspace().unwrap().editors.remove(0).draft, engine.store.library().unwrap().requests.remove(0).draft] {
        assert_eq!(restored.headers, request.headers);
        assert!(restored.headers[1].disabled);
        assert_eq!(restored.headers[5].value, "invalid\n{{unresolved}}");
        assert!(restored.url.is_empty());
    }
}

#[tokio::test]
async fn http1_sends_only_enabled_headers_and_scripts_see_enabled_rows() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut request = draft(format!("http://{}/", listener.local_addr().unwrap()), "native");
        request.scripts.enabled = true;
        request.scripts.before = "assert(request.headers.every(h=>!h.disabled)); assert(!request.headers.some(h=>h.name==='X-Off')); request.headers.push({name:'X-Script-Off',value:'secret',disabled:true});".into();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let head = read_head(&mut stream).await.unwrap();
            let (_, _, headers) = parse_request(&head).unwrap();
            assert_enabled(&headers);
            stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let flow = engine.execute(request, None, "replay", None).await.unwrap();
        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert_enabled(&flow.request.headers);
        assert!(flow.original_request.as_ref().unwrap().headers.iter().any(|h| h.disabled));
        assert_enabled(flow.response.unwrap().sent_request_headers.as_ref().unwrap());
        server.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn http2_filters_disabled_headers_added_during_interception() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).unwrap();
        engine.intercept.configure(Config {
            request:true, response:false, scope:"all".into(),
            rules:vec![Rule { host:"127.0.0.1".into(), ..Default::default() }],
        }).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let request = draft(format!("http://{}/", listener.local_addr().unwrap()), "h2");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut connection = h2::server::handshake(stream).await.unwrap();
            let (request, mut respond) = connection.accept().await.unwrap().unwrap();
            assert!(!request.headers().contains_key("x-off"));
            assert!(!request.headers().contains_key("x-intercept-off"));
            assert!(!request.headers().contains_key("te"));
            assert_eq!(request.headers()["x-live"], "still present");
            assert_eq!(request.headers().get_all("x-keep").iter().map(|h| h.to_str().unwrap()).collect::<Vec<_>>(), ["one", "two"]);
            respond.send_response(http::Response::builder().status(204).body(()).unwrap(), true).unwrap();
            while let Some(result) = connection.accept().await { result.unwrap(); }
        });
        let resolver_engine = engine.clone();
        let resolver = tokio::spawn(async move {
            let item = loop {
                if let Some(item) = resolver_engine.intercept.snapshot().items.into_iter().next() { break item; }
                tokio::task::yield_now().await;
            };
            let mut modified = item.flow.request;
            modified.headers.push(Header { name:"X-Intercept-Off".into(), value:"secret".into(), disabled:true });
            resolver_engine.intercept.resolve(Decision { id:item.id, action:"modify".into(), request:Some(modified), response:None }).unwrap();
        });
        let flow = engine.execute(request, None, "replay", None).await.unwrap();
        assert!(flow.error.is_none(), "{:?}", flow.error);
        assert_enabled(&flow.request.headers);
        let response = flow.response.unwrap();
        assert_eq!(response.version, "HTTP/2");
        assert_enabled(response.sent_request_headers.as_ref().unwrap());
        resolver.await.unwrap();
        server.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn direct_send_engine_filters_without_mutating_the_saved_draft() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let request = draft(format!("http://{}/", listener.local_addr().unwrap()), "native");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let (_, _, headers) = parse_request(&read_head(&mut stream).await.unwrap()).unwrap();
            assert_enabled(&headers);
            stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let (response, _) = engine.send_engines.send(&request).await.unwrap();
        assert_enabled(response.sent_request_headers.as_ref().unwrap());
        assert!(request.headers[1].disabled);
        assert_eq!(request.headers.len(), 7);
        server.await.unwrap();
    }).await.unwrap();
}
