//! Real transports exercise the shared Engine pipeline: automatic rewrites
//! happen after an upstream response and before a manual response breakpoint.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use capture_core::{
    http1::{self, parse_request, parse_response, read_head},
    intercept,
    model::{Flow, Header},
    proxy,
    response_rewrite::Config,
    Engine,
};
use serde_json::{json, Value};
use std::{net::SocketAddr, time::Duration};
use tokio::{io::{AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}};

fn replace(status: u16, body: &[u8]) -> Value {
    json!({"id":"replacement","enabled":true,"matchType":"urlContains","pattern":"/rewrite","action":"replace",
        "replacement":{"status":status,"headers":[{"name":"X-First","value":"one"},{"name":"X-Middle","value":"between"},{"name":"x-first","value":"two"}],"bodyBase64":STANDARD.encode(body)}})
}
fn modify(operations: Value) -> Value {
    json!({"id":"modification","enabled":true,"matchType":"urlContains","pattern":"/rewrite","action":"modify","modifications":operations})
}
fn body_edit() -> Value {
    json!({"target":"body","operation":"replace","find":"flight-(\\d+)","value":"ticket-${1}","regex":true,"all":true})
}
fn configure(engine: &Engine, scope: &str, rules: Vec<Value>) {
    let config: Config = serde_json::from_value(json!({"enabled":true,"scope":scope,"rules":rules})).unwrap();
    engine.response_rewrite.configure(config, &engine.response_rewrite.snapshot().version).unwrap();
}
fn response(status: u16, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!("HTTP/1.1 {status} Test\r\n{headers}Connection: close\r\n\r\n").into_bytes();
    bytes.extend_from_slice(body); bytes
}

async fn origin(bytes: Vec<u8>) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/rewrite",listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut io = BufReader::new(stream);
        let request = read_head(&mut io).await.unwrap();
        // A successful origin handshake and received request distinguish this
        // from a request-side mock or a replacement that bypasses the origin.
        let (_,target,_) = parse_request(&request).unwrap(); assert_eq!(target,"/rewrite");
        io.write_all(&bytes).await.unwrap(); io.shutdown().await.unwrap();
        request
    });
    (url,task)
}

async fn request(address: SocketAddr, url: &str, method: &str) -> (u16,Vec<Header>,Vec<u8>) {
    let mut io = BufReader::new(TcpStream::connect(address).await.unwrap());
    io.write_all(format!("{method} {url} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",http1::authority(&url::Url::parse(url).unwrap())).as_bytes()).await.unwrap();
    let (status,_,headers) = parse_response(&read_head(&mut io).await.unwrap()).unwrap();
    let body = if method=="HEAD" || [204,205,304].contains(&status) { Vec::new() }
        else {http1::read_body(&mut io,&headers,true).await.unwrap().0};
    (status,headers,body)
}

async fn recorded(engine: &Engine, url: &str) -> Flow {
    tokio::time::timeout(Duration::from_secs(3),async {
        loop {
            if let Some(flow) = engine.store.list().unwrap().into_iter().find(|flow|flow.request.url==url&&flow.state=="completed") {return flow;}
            tokio::task::yield_now().await;
        }
    }).await.expect("shared pipeline did not persist the completed response")
}

#[tokio::test]
async fn real_proxy_replaces_after_upstream_preserves_order_and_persists_original() {
    tokio::time::timeout(Duration::from_secs(10),async {
        let directory = tempfile::tempdir().unwrap(); let engine = Engine::open(directory.path()).unwrap();
        let replacement = [0,255,3,9,b'R'];
        configure(&engine,"capture",vec![replace(201,&replacement)]);
        let original = [0,128,255,b'U'];
        let (url,server) = origin(response(200,"X-Origin: first\r\nX-Origin: second\r\nSet-Cookie: a=1; Path=/\r\nSet-Cookie: b=2; Path=/\r\nContent-Length: 4\r\n",&original)).await;
        let proxy = proxy::start(engine.clone(),0).await.unwrap();
        let (status,headers,bytes) = request(proxy.address,&url,"GET").await;
        assert_eq!(status,201); assert_eq!(bytes,replacement);
        assert_eq!(headers.iter().filter(|h|h.name.to_ascii_lowercase().starts_with("x-")).map(|h|(h.name.as_str(),h.value.as_str())).collect::<Vec<_>>(),[("X-First","one"),("X-Middle","between"),("x-first","two")]);
        assert_eq!(parse_request(&server.await.unwrap()).unwrap().0,"GET");
        assert!(engine.intercept.snapshot().items.is_empty());
        let flow = recorded(&engine,&url).await; assert!(flow.error.is_none());
        assert_eq!(engine.response_rewrite.snapshot().hits,vec![1]);
        let before = flow.original_response.as_ref().unwrap();
        assert_eq!(before.status,200); assert_eq!(STANDARD.decode(&before.body_base64).unwrap(),original);
        assert_eq!(before.headers.iter().filter(|h|h.name.eq_ignore_ascii_case("x-origin")).map(|h|h.value.as_str()).collect::<Vec<_>>(),["first","second"]);
        assert_eq!(before.headers.iter().filter(|h|h.name.eq_ignore_ascii_case("set-cookie")).count(),2);
        assert_eq!(flow.response.as_ref().unwrap().version,before.version);
        assert!(flow.response.as_ref().unwrap().sent_request_headers.is_some(),"connection evidence still comes from the actual upstream send");
        assert!(flow.notes.iter().any(|note|note.contains("replacement")&&note.contains("已访问上游")));
        proxy.stop().await; drop(engine);
        let reopened = Engine::open(directory.path()).unwrap();
        assert!(reopened.response_rewrite.snapshot().config.enabled);
        let saved = reopened.store.get(&flow.id).unwrap().unwrap();
        assert_eq!(serde_json::to_value(saved).unwrap(),serde_json::to_value(flow).unwrap());
    }).await.unwrap();
}

#[tokio::test]
async fn real_proxy_decodes_gzip_and_both_deflate_formats_for_regex_body_edits() {
    use flate2::{write::{DeflateEncoder,GzEncoder,ZlibEncoder},Compression};
    use std::io::Write;
    tokio::time::timeout(Duration::from_secs(15),async {
        let directory = tempfile::tempdir().unwrap(); let engine = Engine::open(directory.path()).unwrap();
        configure(&engine,"capture",vec![modify(json!([
            body_edit(),{"target":"header","operation":"replace","name":"X-Order","find":"first","value":"edited","all":false}
        ]))]);
        let proxy = proxy::start(engine.clone(),0).await.unwrap();
        let text = b"flight-123 flight-456";
        for kind in ["gzip","zlib","raw-deflate"] {
            let compressed = match kind {
                "gzip"=>{let mut value=GzEncoder::new(Vec::new(),Compression::default());value.write_all(text).unwrap();value.finish().unwrap()},
                "zlib"=>{let mut value=ZlibEncoder::new(Vec::new(),Compression::default());value.write_all(text).unwrap();value.finish().unwrap()},
                _=>{let mut value=DeflateEncoder::new(Vec::new(),Compression::default());value.write_all(text).unwrap();value.finish().unwrap()},
            };
            let coding = if kind=="gzip" {"gzip"} else {"deflate"};
            let head = format!("X-Order: first\r\nX-Middle: between\r\nx-order: second\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Encoding: {coding}\r\nETag: original-tag\r\nContent-Length: {}\r\n",compressed.len());
            let (url,server) = origin(response(200,&head,&compressed)).await;
            let (status,headers,bytes) = request(proxy.address,&url,"GET").await;
            assert_eq!(status,200); assert_eq!(bytes,b"ticket-123 ticket-456"); server.await.unwrap();
            assert_eq!(headers.iter().filter(|h|h.name.to_ascii_lowercase().starts_with("x-")).map(|h|(h.name.as_str(),h.value.as_str())).collect::<Vec<_>>(),[("X-Order","edited"),("X-Middle","between"),("x-order","second")]);
            assert!(!headers.iter().any(|h|["content-encoding","etag"].iter().any(|name|h.name.eq_ignore_ascii_case(name))));
            assert_eq!(http1::values(&headers,"content-length").collect::<Vec<_>>(),["21"]);
            let flow = recorded(&engine,&url).await;
            assert_eq!(STANDARD.decode(&flow.original_response.as_ref().unwrap().body_base64).unwrap(),compressed);
            assert_eq!(http1::values(&flow.original_response.as_ref().unwrap().headers,"content-encoding").collect::<Vec<_>>(),[coding]);
            assert!(flow.response.as_ref().unwrap().raw_head_base64.is_none());
        }
        assert_eq!(engine.response_rewrite.snapshot().hits,vec![3]);
        assert!(engine.intercept.snapshot().items.is_empty()); proxy.stop().await;
    }).await.unwrap();
}

#[tokio::test]
async fn scope_method_and_url_nonmatches_leave_real_responses_untouched() {
    tokio::time::timeout(Duration::from_secs(10),async {
        let directory = tempfile::tempdir().unwrap(); let engine = Engine::open(directory.path()).unwrap();
        let proxy = proxy::start(engine.clone(),0).await.unwrap();
        for reason in ["scope","method","url"] {
            let mut rule = replace(202,b"unexpected");
            if reason=="method" {rule["method"]=json!("POST");}
            if reason=="url" {rule["pattern"]=json!("/other-path");}
            configure(&engine,if reason=="scope" {"replay"} else {"capture"},vec![rule]);
            let (url,server) = origin(response(200,"X-Unchanged: first\r\nX-Unchanged: second\r\nContent-Length: 8\r\n",b"upstream")).await;
            let (status,headers,bytes) = request(proxy.address,&url,"GET").await;
            assert_eq!(status,200); assert_eq!(bytes,b"upstream"); server.await.unwrap();
            assert_eq!(http1::values(&headers,"x-unchanged").collect::<Vec<_>>(),["first","second"]);
            let flow = recorded(&engine,&url).await;
            assert!(flow.original_response.is_none()); assert_eq!(engine.response_rewrite.snapshot().hits,vec![0]);
        }
        proxy.stop().await;
    }).await.unwrap();
}

#[tokio::test]
async fn manual_response_breakpoint_sees_automatic_edit_and_keeps_earliest_upstream_original() {
    tokio::time::timeout(Duration::from_secs(10),async {
        let directory = tempfile::tempdir().unwrap(); let engine = Engine::open(directory.path()).unwrap();
        configure(&engine,"capture",vec![replace(201,b"automatic")]);
        engine.intercept.configure(intercept::Config{request:false,response:true,scope:"capture".into(),rules:vec![intercept::Rule{url_contains:"/rewrite".into(),..Default::default()}]}).unwrap();
        let (url,server) = origin(response(200,"Content-Length: 8\r\n",b"upstream")).await;
        let proxy = proxy::start(engine.clone(),0).await.unwrap();
        let address = proxy.address; let client_url = url.clone();
        let client = tokio::spawn(async move {request(address,&client_url,"GET").await});
        let item = tokio::time::timeout(Duration::from_secs(3),async {
            loop {if let Some(item)=engine.intercept.snapshot().items.into_iter().next(){break item;}tokio::task::yield_now().await;}
        }).await.unwrap();
        server.await.unwrap(); assert_eq!(item.stage,"response");
        assert_eq!(STANDARD.decode(&item.flow.response.as_ref().unwrap().body_base64).unwrap(),b"automatic");
        assert_eq!(STANDARD.decode(&item.flow.original_response.as_ref().unwrap().body_base64).unwrap(),b"upstream");
        let mut manual = item.flow.response.clone().unwrap(); manual.status=202; manual.body_base64=STANDARD.encode(b"manual").into();
        engine.intercept.resolve(intercept::Decision{id:item.id,action:"modify".into(),request:None,response:Some(manual)}).unwrap();
        let (status,_,bytes) = client.await.unwrap(); assert_eq!(status,202); assert_eq!(bytes,b"manual");
        let flow = recorded(&engine,&url).await;
        assert_eq!(flow.original_response.as_ref().unwrap().status,200);
        assert_eq!(STANDARD.decode(&flow.original_response.as_ref().unwrap().body_base64).unwrap(),b"upstream");
        assert!(engine.intercept.snapshot().items.is_empty());
        proxy.stop().await; drop(engine);
        let reopened = Engine::open(directory.path()).unwrap();
        let saved = reopened.store.get(&flow.id).unwrap().unwrap();
        assert_eq!(STANDARD.decode(&saved.original_response.unwrap().body_base64).unwrap(),b"upstream");
        assert_eq!(STANDARD.decode(&saved.response.unwrap().body_base64).unwrap(),b"manual");
    }).await.unwrap();
}

#[tokio::test]
async fn head_no_content_not_modified_and_finite_sse_skip_body_edits() {
    tokio::time::timeout(Duration::from_secs(10),async {
        let directory = tempfile::tempdir().unwrap(); let engine = Engine::open(directory.path()).unwrap();
        configure(&engine,"capture",vec![modify(json!([body_edit()]))]);
        let proxy = proxy::start(engine.clone(),0).await.unwrap();
        for (method,status,headers,body) in [
            ("HEAD",200,"Content-Length: 10\r\n",&b""[..]),
            ("GET",204,"",&b""[..]),
            ("GET",304,"Content-Length: 10\r\n",&b""[..]),
            ("GET",200,"Content-Type: text/event-stream\r\nContent-Length: 18\r\n",&b"data: flight-123\n\n"[..]),
        ] {
            // SSE length is derived from the actual fixture, avoiding a stalled
            // stream caused by accidentally advertising an extra body byte.
            let headers = if body.is_empty(){headers.to_string()}else{format!("Content-Type: text/event-stream\r\nContent-Length: {}\r\n",body.len())};
            let (url,server) = origin(response(status,&headers,body)).await;
            let (actual_status,_,bytes) = request(proxy.address,&url,method).await;
            assert_eq!(actual_status,status); assert_eq!(bytes,body); server.await.unwrap();
            let flow = recorded(&engine,&url).await; assert!(flow.original_response.is_none());
            assert_eq!(STANDARD.decode(&flow.response.as_ref().unwrap().body_base64).unwrap(),body);
            assert!(flow.notes.iter().any(|note|note.contains("自动响应重写")&&(note.contains("失败")||note.contains("不支持自动修改"))));
        }
        proxy.stop().await;
    }).await.unwrap();
}

#[tokio::test]
async fn h2_capture_rewrites_a_verified_real_h2_response_before_downstream_forwarding() {
    use bytes::Bytes;
    use capture_core::{ca::CertificateAuthority,multiplex};
    use std::sync::Arc;
    tokio::time::timeout(Duration::from_secs(15),async {
        let directory = tempfile::tempdir().unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let authority = CertificateAuthority::load_or_create(&directory.path().join("upstream-ca")).unwrap();
        let pem = std::fs::read(&authority.cert_path).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut &pem[..]) {roots.add(cert.unwrap()).unwrap();}
        let engine = Engine::open_with_roots(&directory.path().join("capture"),roots).unwrap();
        configure(&engine,"capture",vec![replace(203,b"h2-edited")]);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://localhost:{}/rewrite",listener.local_addr().unwrap().port());
        let mut config = (*authority.server_config("localhost").unwrap()).clone(); config.alpn_protocols=vec![b"h2".to_vec()];
        let (observed,saw_request) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (tcp,_) = listener.accept().await.unwrap();
            let tls = tokio_rustls::TlsAcceptor::from(Arc::new(config)).accept(tcp).await.unwrap();
            assert_eq!(tls.get_ref().1.alpn_protocol(),Some(&b"h2"[..]));
            let mut connection = h2::server::handshake(tls).await.unwrap();
            let mut observed = Some(observed);
            while let Some(request) = connection.accept().await {
                let (request,mut respond) = request.unwrap();
                assert_eq!(request.method(),"GET"); assert_eq!(request.uri().path(),"/rewrite");
                if let Some(sender)=observed.take(){let _=sender.send(());}
                let response = http::Response::builder().status(200).header("content-length","8").header("x-upstream","verified-h2").body(()).unwrap();
                respond.send_response(response,false).unwrap().send_data(Bytes::from_static(b"upstream"),true).unwrap();
            }
        });
        let (client_io,proxy_io) = tokio::io::duplex(65_536);
        let captured_engine = engine.clone(); let target = url.parse().unwrap();
        let capture = tokio::spawn(async move {multiplex::h2_capture(proxy_io,captured_engine,target).await});
        let (mut client,connection) = h2::client::handshake(client_io).await.unwrap();
        let driver = tokio::spawn(connection);
        let (response,_) = client.send_request(http::Request::builder().uri(&url).body(()).unwrap(),true).unwrap();
        let response = response.await.unwrap(); assert_eq!(response.status(),203);
        assert_eq!(multiplex::recv_h2_body(&mut response.into_body()).await.unwrap(),b"h2-edited");
        saw_request.await.unwrap();
        let flow = recorded(&engine,&url).await;
        assert_eq!(flow.client_protocol.as_deref(),Some("HTTP/2"));
        let original = flow.original_response.as_ref().unwrap(); let final_response = flow.response.as_ref().unwrap();
        assert_eq!(original.version,"HTTP/2"); assert_eq!(final_response.version,"HTTP/2");
        assert_eq!(STANDARD.decode(&original.body_base64).unwrap(),b"upstream");
        assert!(original.upstream_tls.is_some() && final_response.upstream_tls.is_some());
        assert_eq!(serde_json::to_value(&original.upstream_tls).unwrap(),serde_json::to_value(&final_response.upstream_tls).unwrap());
        assert!(engine.intercept.snapshot().items.is_empty());
        drop(client); driver.abort(); capture.abort(); server.abort();
        let _=driver.await; let _=capture.await; let _=server.await;
    }).await.unwrap();
}
