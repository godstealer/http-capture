use capture_core::{ca::CertificateAuthority, http1::*, model::*, upstream::*, Engine};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{sync::Arc, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}};
fn input(url: String, auth: bool) -> UpstreamInput {
    UpstreamInput { enabled: true, url, username: if auth { "test-user".into() } else { String::new() }, auth_enabled: auth, password: Some(if auth { "test-secret".into() } else { String::new() }) }
}
fn draft(engine: &str, url: String) -> RequestDraft {
    RequestDraft { upstream_profile_id: None, scripts: Default::default(), pseudo_headers: vec![], engine: engine.into(), method: "GET".into(), url,
        headers: vec![Header { name: "x-a".into(), value: "one".into() }, Header { name: "x-b".into(), value: "two".into() }, Header { name: "x-a".into(), value: "three".into() }], body_base64: String::new(), tls: Default::default() }
}
#[tokio::test]
async fn http_proxy_absolute_form_auth_redaction_and_replay_scope() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let dir = tempfile::tempdir().unwrap(); let engine = Engine::open(dir.path()).unwrap();
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        engine.upstream.update(input(format!("http://{}", proxy.local_addr().unwrap()), true)).unwrap();
        let observer = tokio::spawn(async move {
            let (io, _) = proxy.accept().await.unwrap(); let mut io = BufReader::new(io);
            let head = read_head(&mut io).await.unwrap(); let (_, target, fields) = parse_request(&head).unwrap();
            assert_eq!(target, "http://does-not-resolve.invalid/path?q=1");
            assert_eq!(fields[0].name, "Proxy-Authorization");
            assert_eq!(fields[0].value, format!("Basic {}", STANDARD.encode("test-user:test-secret")));
            assert_eq!(fields.iter().filter(|h|h.name.starts_with("x-")).map(|h|h.name.as_str()).collect::<Vec<_>>(), ["x-a", "x-b", "x-a"]);
            io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await.unwrap();
        });
        let flow = engine.execute(draft("native", "http://does-not-resolve.invalid/path?q=1".into()), None, "capture", None).await.unwrap();
        assert!(flow.error.is_none(), "{:?}", flow.error); assert_eq!(flow.response.as_ref().unwrap().status, 200);
        assert_eq!(flow.response.as_ref().unwrap().sent_request_headers.as_ref().unwrap()[0].value, "[redacted]");
        let serialized = serde_json::to_string(&flow).unwrap();
        assert!(!serialized.contains("test-secret") && !serialized.contains(&STANDARD.encode("test-user:test-secret")));
        assert!(!serde_json::to_string(&engine.upstream.status()).unwrap().contains("test-secret"));
        observer.await.unwrap();
        // Replay remains direct even though the configured proxy is now closed.
        let direct = TcpListener::bind("127.0.0.1:0").await.unwrap(); let address = direct.local_addr().unwrap();
        let direct_task = tokio::spawn(async move { let (io, _) = direct.accept().await.unwrap(); let mut io=BufReader::new(io); read_head(&mut io).await.unwrap(); io.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").await.unwrap(); });
        let replay = engine.execute(draft("native", format!("http://{address}/")), None, "replay", None).await.unwrap();
        assert_eq!(replay.response.unwrap().status, 204); direct_task.await.unwrap();
    }).await.unwrap();
}
async fn tunnel_handshake(io: TcpStream, protocol: &str, expected_port: u16) -> BufReader<TcpStream> {
    let mut io=BufReader::new(io);
    if protocol == "http" {
        let head=read_head(&mut io).await.unwrap(); let (method,target,fields)=parse_request(&head).unwrap();
        assert_eq!(method,"CONNECT"); assert_eq!(target,format!("localhost:{expected_port}"));
        assert!(fields.iter().any(|h| h.name == "Proxy-Authorization"));
        io.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.unwrap();
    } else {
        let mut greeting=[0;3];io.read_exact(&mut greeting).await.unwrap();assert_eq!(greeting,[5,1,2]);
        io.write_all(&[5,2]).await.unwrap(); assert_eq!(io.read_u8().await.unwrap(),1);
        let len=io.read_u8().await.unwrap(); let mut user=vec![0;len as usize];io.read_exact(&mut user).await.unwrap();assert_eq!(user,b"test-user");
        let len=io.read_u8().await.unwrap(); let mut pass=vec![0;len as usize];io.read_exact(&mut pass).await.unwrap();assert_eq!(pass,b"test-secret");
        io.write_all(&[1,0]).await.unwrap();let mut request=[0;4];io.read_exact(&mut request).await.unwrap();assert_eq!(request,[5,1,0,3]);
        let len=io.read_u8().await.unwrap();let mut host=vec![0;len as usize];io.read_exact(&mut host).await.unwrap();assert_eq!(host,b"localhost");assert_eq!(io.read_u16().await.unwrap(),expected_port);
        io.write_all(&[5,0,0,1,127,0,0,1,0,0]).await.unwrap();
    }
    io
}
#[tokio::test]
async fn http_and_socks_tunnels_preserve_h1_h2_tls_and_order() {
    tokio::time::timeout(Duration::from_secs(12), async {
        let _ = rustls::crypto::ring::default_provider().install_default();
        for proxy_kind in ["http", "socks5"] { for protocol in ["native", "h2"] {
            let dir=tempfile::tempdir().unwrap();let ca=CertificateAuthority::load_or_create(&dir.path().join("origin")).unwrap();
            let mut roots=rustls::RootCertStore::empty();let pem=std::fs::read(&ca.cert_path).unwrap();for cert in rustls_pemfile::certs(&mut &pem[..]) { roots.add(cert.unwrap()).unwrap(); }
            let engine=Engine::open_with_roots(&dir.path().join("capture"),roots).unwrap();
            let origin=TcpListener::bind("127.0.0.1:0").await.unwrap();let origin_address=origin.local_addr().unwrap();
            let mut config=(*ca.server_config("localhost").unwrap()).clone();config.alpn_protocols=vec![if protocol=="h2" {b"h2".to_vec()} else {b"http/1.1".to_vec()}];
            let origin_task=tokio::spawn(async move {
                let (io,_)=origin.accept().await.unwrap();let io=tokio_rustls::TlsAcceptor::from(Arc::new(config)).accept(io).await.unwrap();
                if protocol=="h2" {
                    let mut conn=h2::server::handshake(io).await.unwrap();
                    while let Some(request)=conn.accept().await {
                        let (request,mut respond)=request.unwrap();
                        assert!(!request.headers().contains_key("proxy-authorization"));
                        assert_eq!(request.extensions().get::<h2::ext::HeaderOrder>().unwrap().0,[":method",":scheme",":authority",":path","x-a","x-b","x-a"]);
                        respond.send_response(http::Response::builder().status(204).body(()).unwrap(),true).unwrap();
                    }
                } else {
                    let mut io=BufReader::new(io);let raw=read_head(&mut io).await.unwrap();let (_,target,fields)=parse_request(&raw).unwrap();assert_eq!(target,"/echo");
                    assert!(!fields.iter().any(|h|h.name.eq_ignore_ascii_case("proxy-authorization")));
                    assert_eq!(fields.iter().filter(|h|h.name.starts_with("x-")).map(|h|h.name.as_str()).collect::<Vec<_>>(),["x-a","x-b","x-a"]);
                    io.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").await.unwrap();
                }
            });
            let proxy=TcpListener::bind("127.0.0.1:0").await.unwrap();engine.upstream.update(input(format!("{proxy_kind}://{}",proxy.local_addr().unwrap()),true)).unwrap();
            let proxy_task=tokio::spawn(async move {
                let (io,_)=proxy.accept().await.unwrap();let mut io=tunnel_handshake(io,proxy_kind,origin_address.port()).await;
                let mut target=TcpStream::connect(origin_address).await.unwrap();let _=tokio::io::copy_bidirectional(&mut io,&mut target).await;
            });
            let flow=engine.execute(draft(protocol,format!("https://localhost:{}/echo",origin_address.port())),None,"capture",None).await.unwrap();
            assert!(flow.error.is_none(),"{proxy_kind} {protocol}: {:?}",flow.error);let response=flow.response.unwrap();assert_eq!(response.status,204);assert!(response.upstream_tls.unwrap().certificates.len()>0);
            proxy_task.await.unwrap();origin_task.await.unwrap();
        }}
    }).await.unwrap();
}
#[tokio::test]
async fn rejected_proxy_never_falls_back_and_self_loop_is_blocked() {
    tokio::time::timeout(Duration::from_secs(5),async {
        let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
        let origin=TcpListener::bind("127.0.0.1:0").await.unwrap();let origin_address=origin.local_addr().unwrap();
        let proxy=TcpListener::bind("127.0.0.1:0").await.unwrap();engine.upstream.update(input(format!("http://{}",proxy.local_addr().unwrap()),true)).unwrap();
        let task=tokio::spawn(async move {let (io,_)=proxy.accept().await.unwrap();let mut io=BufReader::new(io);read_head(&mut io).await.unwrap();io.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n").await.unwrap();});
        let flow=engine.execute(draft("h2",format!("https://{origin_address}/")),None,"capture",None).await.unwrap();assert!(flow.error.unwrap().contains("407"));assert!(tokio::time::timeout(Duration::from_millis(100),origin.accept()).await.is_err());task.await.unwrap();
        *engine.upstream.listener.write().unwrap()=Some(origin_address);
        assert!(engine.upstream.update(input(format!("http://localhost:{}",origin_address.port()),false)).is_err());
        assert!(engine.upstream.update(input("ftp://127.0.0.1:21".into(),false)).is_err());
    }).await.unwrap();
}

#[tokio::test]
async fn socks_no_auth_remote_dns_and_method_rejection() {
    tokio::time::timeout(Duration::from_secs(5),async {
        for accepted in [true,false] {
            let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
            let proxy=TcpListener::bind("127.0.0.1:0").await.unwrap();engine.upstream.update(input(format!("socks5://{}",proxy.local_addr().unwrap()),false)).unwrap();
            let task=tokio::spawn(async move {
                let (mut io,_)=proxy.accept().await.unwrap();let mut greeting=[0;3];io.read_exact(&mut greeting).await.unwrap();assert_eq!(greeting,[5,1,0]);
                io.write_all(&[5,if accepted {0} else {255}]).await.unwrap();if !accepted {return;}
                let mut request=[0;4];io.read_exact(&mut request).await.unwrap();assert_eq!(request,[5,1,0,3]);
                let len=io.read_u8().await.unwrap();let mut host=vec![0;len as usize];io.read_exact(&mut host).await.unwrap();assert_eq!(host,b"does-not-resolve.invalid");assert_eq!(io.read_u16().await.unwrap(),80);
                io.write_all(&[5,0,0,3,1,b'x',0,0]).await.unwrap();let mut io=BufReader::new(io);let raw=read_head(&mut io).await.unwrap();let (_,target,fields)=parse_request(&raw).unwrap();assert_eq!(target,"/echo");assert!(!fields.iter().any(|h|h.name.eq_ignore_ascii_case("proxy-authorization")));
                io.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").await.unwrap();
            });
            let flow=engine.execute(draft("native","http://does-not-resolve.invalid/echo".into()),None,"capture",None).await.unwrap();
            if accepted {assert_eq!(flow.response.unwrap().status,204);} else {assert!(flow.error.unwrap().contains("认证方式"));}
            task.await.unwrap();
        }
    }).await.unwrap();
}

#[test]
fn named_profiles_switch_persist_and_keep_passwords_in_memory() {
 use capture_core::upstream::{UpstreamState,ProfileInput,UpstreamInput};
 let dir=tempfile::tempdir().unwrap();let state=UpstreamState::open(dir.path()).unwrap();
 let input=|id:Option<String>,name:&str,url:&str,auth:bool,password:Option<&str>|ProfileInput{id,name:name.into(),remember_password:false,config:UpstreamInput{enabled:true,url:url.into(),username:if auth{"tester".into()}else{String::new()},auth_enabled:auth,password:password.map(str::to_owned)}};
 state.save_profile(input(None,"业务 A","http://127.0.0.1:18001",true,Some("private-secret-test"))).unwrap();
 state.save_profile(input(None,"业务 B","socks5://127.0.0.1:18002",false,None)).unwrap();
 let list=state.list_profiles();let a=list[0].id.clone();let b=list[1].id.clone();
 state.select_profile(Some(a.clone())).unwrap();assert_eq!(state.status().profile_name.as_deref(),Some("业务 A"));
 let old=state.snapshot().unwrap();state.select_profile(Some(b.clone())).unwrap();assert_eq!(old.url.port(),Some(18001));assert_eq!(state.snapshot().unwrap().url.port(),Some(18002));
 assert!(state.delete_profile(b.clone()).is_err());
 state.save_profile(input(Some(a.clone()),"业务 A 更新","http://127.0.0.1:18001",true,None)).unwrap();
 state.select_profile(Some(a.clone())).unwrap();state.save_profile(input(Some(a.clone()),"业务 A 生效","http://127.0.0.1:18001",true,None)).unwrap();assert_eq!(state.status().profile_name.as_deref(),Some("业务 A 生效"));
 assert!(state.save_profile(input(Some(a.clone()),"业务 A 生效","http://127.0.0.1:18003",true,None)).is_err());
 assert!(state.save_profile(input(None,"业务 B","http://127.0.0.1:19000",false,None)).is_err());
 let disk=std::fs::read_to_string(dir.path().join("upstream-profiles.json")).unwrap();assert!(!disk.contains("private-secret-test"));assert!(!serde_json::to_string(&state.list_profiles()).unwrap().contains("private-secret-test"));
 let restored=UpstreamState::open(dir.path()).unwrap();assert!(!restored.status().enabled);assert_eq!(restored.list_profiles().len(),2);assert!(restored.list_profiles()[0].needs_password);assert!(restored.select_profile(Some(a)).is_err());restored.select_profile(Some(b.clone())).unwrap();
 restored.select_profile(None).unwrap();restored.delete_profile(b).unwrap();assert_eq!(UpstreamState::open(dir.path()).unwrap().list_profiles().len(),1);
}

#[tokio::test]
async fn manual_requests_select_independent_http_and_socks_profiles() {
 tokio::time::timeout(Duration::from_secs(5),async {
  let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
  let mut jobs=vec![];let mut requests=vec![];
  for kind in ["http","socks5"] {
   let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
   engine.upstream.save_profile(ProfileInput{id:None,name:kind.into(),remember_password:false,config:input(format!("{kind}://{address}"),kind=="socks5")}).unwrap();
   let id=engine.upstream.list_profiles().into_iter().find(|p|p.name==kind).unwrap().id;
   let mut request=draft("auto","http://localhost:9876/manual".into());request.upstream_profile_id=Some(id.clone());requests.push(request);
   if kind=="http" {engine.upstream.select_profile(Some(id)).unwrap();}
   jobs.push(tokio::spawn(async move {
    let (io,_)=listener.accept().await.unwrap();let mut io=if kind=="socks5" {tunnel_handshake(io,"socks5",9876).await}else{BufReader::new(io)};
    let head=read_head(&mut io).await.unwrap();let (_,target,headers)=parse_request(&head).unwrap();
    assert_eq!(target,if kind=="http"{"http://localhost:9876/manual"}else{"/manual"});
    assert_eq!(headers.iter().filter(|h|h.name.starts_with("x-")).map(|h|h.name.as_str()).collect::<Vec<_>>(),["x-a","x-b","x-a"]);
    io.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").await.unwrap();
   }));
  }
  let capture_id=engine.upstream.status().profile_id;
  let (a,b)=tokio::join!(engine.execute(requests[0].clone(),None,"replay",None),engine.execute(requests[1].clone(),None,"replay",None));
  for response in [a.unwrap(),b.unwrap()] {assert!(response.error.is_none(),"{:?}",response.error);assert_eq!(response.response.unwrap().status,204);}
  assert_eq!(engine.upstream.status().profile_id,capture_id);
  for job in jobs{job.await.unwrap();}
 }).await.unwrap();
}

#[tokio::test]
async fn manual_missing_profile_and_unsupported_engine_never_fall_back() {
 let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
 let target=TcpListener::bind("127.0.0.1:0").await.unwrap();
 let mut request=draft("auto",format!("http://{}/",target.local_addr().unwrap()));request.upstream_profile_id=Some("missing".into());
 let result=engine.execute(request.clone(),None,"replay",None).await.unwrap();assert!(result.error.unwrap().contains("代理配置不存在"));
 engine.upstream.save_profile(ProfileInput{id:None,name:"h3-test".into(),remember_password:false,config:input("http://127.0.0.1:9".into(),false)}).unwrap();
 request.upstream_profile_id=Some(engine.upstream.list_profiles()[0].id.clone());request.engine="h3".into();
 request.url=format!("https://{}/",target.local_addr().unwrap());
 let result=engine.execute(request,None,"replay",None).await.unwrap();let error=result.error.expect("HTTP upstream must be rejected for H3");assert!(error.contains("HTTP CONNECT 不支持 UDP"),"{error}");
 assert!(tokio::time::timeout(Duration::from_millis(50),target.accept()).await.is_err());
}
