use base64::{engine::general_purpose::STANDARD, Engine as _};
use capture_core::{blocking::Config, http1::{parse_response, read_body, read_head}, intercept, model::{Flow, RequestDraft}, proxy, scripts::Scripts, Engine};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}};

fn config(kind:&str, pattern:&str, scope:&str) -> Config {
    serde_json::from_value(json!({"enabled":true,"scope":scope,"rules":[{"id":"rule-1","enabled":true,"matchType":kind,"pattern":pattern,"method":""}]})).unwrap()
}
fn draft(url:String) -> RequestDraft {
    serde_json::from_value(json!({"method":"GET","url":url,"engine":"native","headers":[],"bodyBase64":""})).unwrap()
}
fn assert_blocked(flow:&Flow) {
    assert_eq!(flow.state,"blocked");
    assert!(flow.error.is_none(),"{:?}",flow.error);
    let response=flow.response.as_ref().unwrap();
    assert_eq!(response.status,403);
    assert!(response.headers.iter().any(|h|h.name=="X-HTTP-Capture-Blocked" && h.value=="rule-1"));
    assert!(response.upstream_tls.is_none());
    assert!(response.sent_request_headers.is_none());
    assert!(flow.notes.iter().any(|note|note.contains("规则 rule-1")));
    assert!(!STANDARD.decode(&response.body_base64).unwrap().windows(6).any(|w|w==b"secret"));
}
async fn no_connection(listener:&TcpListener) {
    assert!(tokio::time::timeout(Duration::from_millis(100),listener.accept()).await.is_err(),"blocked origin received a TCP connection");
}
async fn captured(engine:&Engine) -> Flow {
    tokio::time::timeout(Duration::from_secs(2),async {
        loop {
            if let Some(flow)=engine.store.list().unwrap().into_iter().find(|f|f.state=="blocked") { return flow; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap()
}
async fn proxy_get(address:std::net::SocketAddr, url:&str, upgrade:bool) -> u16 {
    let mut io=BufReader::new(TcpStream::connect(address).await.unwrap());
    let extra=if upgrade {"Connection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"} else {"Connection: close\r\n"};
    io.write_all(format!("GET {url} HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra}\r\n").as_bytes()).await.unwrap();
    let (status,_,headers)=parse_response(&read_head(&mut io).await.unwrap()).unwrap();
    read_body(&mut io,&headers,true).await.unwrap();
    status
}

#[test]
fn config_validation_is_atomic_persistent_and_restores_compiled_regex() {
    let dir=tempfile::tempdir().unwrap();
    let engine=Engine::open(dir.path()).unwrap();
    assert!(!engine.blocking.snapshot().enabled);
    assert_eq!(engine.blocking.snapshot().scope,"capture");
    let valid=config("urlRegex",r"/blocked(?:\?|$)","capture");
    engine.blocking.configure(valid.clone()).unwrap();
    for mutate in ["regex","scope","host","method","duplicate","limit"] {
        let mut bad=serde_json::to_value(&valid).unwrap();
        match mutate {
            "regex"=>bad["rules"][0]["pattern"]=json!("["),
            "scope"=>bad["scope"]=json!("invalid"),
            "host"=>{bad["rules"][0]["matchType"]=json!("host");bad["rules"][0]["pattern"]=json!("*.example.test");},
            "method"=>bad["rules"][0]["method"]=json!("GET\r\n"),
            "duplicate"=>{let rule=bad["rules"][0].clone();bad["rules"].as_array_mut().unwrap().push(rule);},
            _=>{let mut rules=vec![];for i in 0..101{let mut rule=bad["rules"][0].clone();rule["id"]=json!(format!("r{i}"));rules.push(rule);}bad["rules"]=json!(rules);},
        }
        assert!(engine.blocking.configure(serde_json::from_value(bad).unwrap()).is_err(),"{mutate}");
        assert_eq!(serde_json::to_value(engine.blocking.snapshot()).unwrap(),serde_json::to_value(&valid).unwrap());
    }
    drop(engine);
    let engine=Engine::open(dir.path()).unwrap();
    assert!(engine.blocking.check(&draft("https://example.test/blocked?secret=value".into()),"capture").is_some());
    assert!(engine.blocking.check(&draft("https://example.test/allowed".into()),"capture").is_none());
    assert!(engine.blocking.check(&draft("https://example.test/blocked".into()),"replay").is_none());
}

#[test]
fn host_method_and_url_matching_are_explicit() {
    let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
    let mut rules=config("host","EXAMPLE.test","all");rules.rules[0].method="POST".into();
    engine.blocking.configure(rules).unwrap();
    let mut request=draft("https://example.test:8443/path".into());
    assert!(engine.blocking.check(&request,"capture").is_none());request.method="POST".into();
    assert!(engine.blocking.check(&request,"capture").is_some());
    request.url="https://sub.example.test/path".into();assert!(engine.blocking.check(&request,"capture").is_none());
    let mut rules=config("urlContains","/CaseSensitive","all");rules.rules[0].enabled=false;
    engine.blocking.configure(rules.clone()).unwrap();request.url="https://example.test/CaseSensitive".into();assert!(engine.blocking.check(&request,"capture").is_none());
    rules.rules[0].enabled=true;engine.blocking.configure(rules.clone()).unwrap();assert!(engine.blocking.check(&request,"capture").is_some());
    request.url="https://example.test/casesensitive".into();assert!(engine.blocking.check(&request,"capture").is_none());
    rules.rules.clear();engine.blocking.configure(rules).unwrap();assert!(engine.blocking.check(&request,"capture").is_none());
}

#[tokio::test]
async fn blocked_capture_precedes_scripts_and_interception_but_unmatched_requests_reach_origin() {
    tokio::time::timeout(Duration::from_secs(5),async {
        let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
        engine.blocking.configure(config("urlContains","/blocked","capture")).unwrap();
        engine.scripts.configure(Scripts{enabled:true,before:"throw new Error('must not run');".into(),..Default::default()}).unwrap();
        engine.intercept.configure(intercept::Config{request:true,response:true,scope:"all".into(),rules:vec![intercept::Rule{host:"127.0.0.1".into(),..Default::default()}]}).unwrap();
        let origin=TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}/blocked?secret=value",origin.local_addr().unwrap());
        let proxy=proxy::start(engine.clone(),0).await.unwrap();
        assert_eq!(proxy_get(proxy.address,&url,false).await,403);
        assert_blocked(&captured(&engine).await);
        assert!(engine.intercept.snapshot().items.is_empty());no_connection(&origin).await;
        engine.scripts.configure(Scripts::default()).unwrap();engine.intercept.configure(intercept::Config{scope:"all".into(),..Default::default()}).unwrap();
        let server=tokio::spawn(async move {let (stream,_)=origin.accept().await.unwrap();let mut stream=BufReader::new(stream);let head=read_head(&mut stream).await.unwrap();assert!(head.starts_with(b"GET /allowed "));stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();});
        assert_eq!(proxy_get(proxy.address,&url.replace("/blocked?secret=value","/allowed"),false).await,200);
        server.await.unwrap();proxy.stop().await;
    }).await.unwrap();
}

#[tokio::test]
async fn capture_scope_all_scope_and_script_rewrites_are_checked_before_send() {
    tokio::time::timeout(Duration::from_secs(5),async {
        let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
        engine.blocking.configure(config("urlContains","/blocked","capture")).unwrap();
        let origin=TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}/blocked",origin.local_addr().unwrap());
        let server=tokio::spawn(async move {let (stream,_)=origin.accept().await.unwrap();let mut stream=BufReader::new(stream);read_head(&mut stream).await.unwrap();stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").await.unwrap();});
        let flow=engine.execute(draft(url),None,"replay",None).await.unwrap();assert_eq!(flow.response.unwrap().status,204);server.await.unwrap();
        let unused=TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}/blocked?secret=value",unused.local_addr().unwrap());
        engine.blocking.configure(config("urlContains","/blocked","replay")).unwrap();
        assert!(engine.blocking.check(&draft(url.clone()),"capture").is_none());
        assert_blocked(&engine.execute(draft(url.clone()),None,"replay",None).await.unwrap());
        engine.blocking.configure(config("urlContains","/blocked","all")).unwrap();
        assert_blocked(&engine.execute(draft(url.clone()),None,"replay",None).await.unwrap());
        let mut request=draft(url.replace("/blocked?secret=value","/allowed"));request.scripts.enabled=true;request.scripts.before=format!("request.url={};",serde_json::to_string(&url).unwrap());
        assert_blocked(&engine.execute(request,None,"replay",None).await.unwrap());no_connection(&unused).await;
    }).await.unwrap();
}

#[tokio::test]
async fn https_host_block_skips_eager_upstream_and_returns_local_http2_denial() {
    Box::pin(tokio::time::timeout(Duration::from_secs(5),async {
        let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
        engine.blocking.configure(config("host","127.0.0.1","capture")).unwrap();
        let origin=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=origin.local_addr().unwrap();
        let proxy=proxy::start(engine.clone(),0).await.unwrap();
        let mut client=BufReader::new(TcpStream::connect(proxy.address).await.unwrap());
        client.write_all(format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes()).await.unwrap();
        assert!(read_head(&mut client).await.unwrap().starts_with(b"HTTP/1.1 200"));
        let pem=std::fs::read(&engine.ca.cert_path).unwrap();let mut roots=rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut &pem[..]){roots.add(cert.unwrap()).unwrap();}
        let mut tls=rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();tls.alpn_protocols=vec![b"h2".to_vec(),b"http/1.1".to_vec()];
        let client=tokio_rustls::TlsConnector::from(Arc::new(tls)).connect("127.0.0.1".try_into().unwrap(),client).await.unwrap();
        assert_eq!(client.get_ref().1.alpn_protocol(),Some(b"h2".as_slice()));
        let (mut sender,connection)=h2::client::handshake(client).await.unwrap();let driver=tokio::spawn(connection);
        let request=http::Request::builder().method("HEAD").uri(format!("https://{address}/blocked?secret=value")).body(()).unwrap();
        let response=sender.send_request(request,true).unwrap().0.await.unwrap();assert_eq!(response.status(),403);assert_eq!(response.headers()["x-http-capture-blocked"],"rule-1");
        assert!(response.into_body().data().await.is_none(),"HEAD denial must not send HTTP/2 DATA");
        assert_blocked(&captured(&engine).await);no_connection(&origin).await;driver.abort();drop(sender);proxy.stop().await;
    })).await.unwrap();
}

#[tokio::test]
async fn decryption_bypass_host_block_rejects_connect_without_a_remote_socket() {
    tokio::time::timeout(Duration::from_secs(5),async {
        let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
        engine.blocking.configure(config("host","127.0.0.1","capture")).unwrap();engine.store.save_setting("decryption.bypass.v1",&vec!["127.0.0.1"]).unwrap();
        let origin=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=origin.local_addr().unwrap();let proxy=proxy::start(engine.clone(),0).await.unwrap();
        let mut client=BufReader::new(TcpStream::connect(proxy.address).await.unwrap());client.write_all(format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes()).await.unwrap();
        let (status,_,_)=parse_response(&read_head(&mut client).await.unwrap()).unwrap();assert_eq!(status,403);
        let flow=captured(&engine).await;assert_blocked(&flow);assert_eq!(flow.request.method,"CONNECT");no_connection(&origin).await;proxy.stop().await;
    }).await.unwrap();
}

#[tokio::test]
async fn websocket_upgrade_is_blocked_before_contacting_the_origin() {
    tokio::time::timeout(Duration::from_secs(5),async {
        let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();engine.blocking.configure(config("host","127.0.0.1","capture")).unwrap();
        let origin=TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("ws://{}/chat",origin.local_addr().unwrap());let proxy=proxy::start(engine.clone(),0).await.unwrap();
        assert_eq!(proxy_get(proxy.address,&url,true).await,403);let flow=captured(&engine).await;assert_blocked(&flow);assert_eq!(flow.websocket.unwrap().state,"blocked");no_connection(&origin).await;proxy.stop().await;
    }).await.unwrap();
}

#[tokio::test]
async fn blocked_head_keeps_get_representation_length_without_http1_payload_bytes() {
    tokio::time::timeout(Duration::from_secs(5),async {
        let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();engine.blocking.configure(config("host","127.0.0.1","all")).unwrap();
        let origin=TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}/blocked",origin.local_addr().unwrap());
        let get=engine.execute(draft(url.clone()),None,"replay",None).await.unwrap();let get_length=STANDARD.decode(&get.response.unwrap().body_base64).unwrap().len();
        let proxy=proxy::start(engine.clone(),0).await.unwrap();let mut client=BufReader::new(TcpStream::connect(proxy.address).await.unwrap());
        client.write_all(format!("HEAD {url} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let (status,_,headers)=parse_response(&read_head(&mut client).await.unwrap()).unwrap();assert_eq!(status,403);
        assert_eq!(headers.iter().find(|h|h.name.eq_ignore_ascii_case("content-length")).unwrap().value,get_length.to_string());
        let mut payload=vec![];client.read_to_end(&mut payload).await.unwrap();assert!(payload.is_empty());
        no_connection(&origin).await;proxy.stop().await;
    }).await.unwrap();
}
