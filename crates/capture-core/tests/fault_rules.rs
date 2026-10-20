use capture_core::{Engine,config_migration, fault_rules::Config, http1::{parse_response,read_body,read_head}, model::RequestDraft, proxy};
use serde_json::{json,Value};
use std::{collections::BTreeMap,sync::Arc,time::{Duration,Instant}};
use tokio::{io::{AsyncWriteExt,BufReader},net::{TcpListener,TcpStream}};

fn config(delay:u32,requests:Option<u32>,scope:&str)->Config {
    serde_json::from_value(json!({"enabled":true,"scope":scope,"rules":[{"id":"test-rule","enabled":true,"matchType":"urlContains","pattern":"/api","method":"","delayMs":delay,"rateLimit":requests.map(|n|json!({"requests":n,"windowMs":1000}))}]})).unwrap()
}
fn apply(engine:&Engine,config:Config){engine.fault_rules.configure(config,&engine.fault_rules.snapshot().version).unwrap();}
fn draft(url:String)->RequestDraft {serde_json::from_value(json!({"url":url,"method":"GET","headers":[],"bodyBase64":"","engine":"native"})).unwrap()}
async fn no_connection(listener:&TcpListener){assert!(tokio::time::timeout(Duration::from_millis(80),listener.accept()).await.is_err());}
async fn get(address:std::net::SocketAddr,url:&str)->(u16,Vec<u8>){
    let mut io=BufReader::new(TcpStream::connect(address).await.unwrap());
    io.write_all(format!("GET {url} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let (status,_,headers)=parse_response(&read_head(&mut io).await.unwrap()).unwrap();
    (status,read_body(&mut io,&headers,true).await.unwrap().0)
}

#[test]
fn validation_cas_persistence_and_second_engine_writes_are_guarded(){
    let directory=tempfile::tempdir().unwrap();let engine=Engine::open(directory.path()).unwrap();
    let old=engine.fault_rules.snapshot().version;apply(&engine,config(10,Some(5),"all"));
    assert!(engine.fault_rules.configure(config(20,None,"all"),&old).is_err());
    for kind in ["delay","zeroLimit","window","regex","noAction","scope","duplicate","method"]{
        let mut bad=serde_json::to_value(config(10,Some(5),"all")).unwrap();
        match kind {
            "delay"=>bad["rules"][0]["delayMs"]=json!(30001),
            "zeroLimit"=>bad["rules"][0]["rateLimit"]["requests"]=json!(0),
            "window"=>bad["rules"][0]["rateLimit"]["windowMs"]=json!(99),
            "regex"=>{bad["rules"][0]["matchType"]=json!("urlRegex");bad["rules"][0]["pattern"]=json!("[");},
            "noAction"=>{bad["rules"][0]["delayMs"]=json!(0);bad["rules"][0]["rateLimit"]=Value::Null;},
            "scope"=>bad["scope"]=json!("websocket"),
            "method"=>bad["rules"][0]["method"]=json!("GET\r\nX: secret"),
            _=>{let duplicate=bad["rules"][0].clone();bad["rules"].as_array_mut().unwrap().push(duplicate);},
        }
        let version=engine.fault_rules.snapshot().version;
        assert!(engine.fault_rules.configure(serde_json::from_value(bad).unwrap(),&version).is_err(),"{kind}");
        assert_eq!(version,engine.fault_rules.snapshot().version);
    }
    let second=Engine::open(directory.path()).unwrap();let version=second.fault_rules.snapshot().version;
    apply(&engine,config(15,None,"all"));
    assert!(second.fault_rules.configure(config(20,None,"all"),&version).is_err());
    drop(second);drop(engine);let reopened=Engine::open(directory.path()).unwrap();
    assert_eq!(reopened.fault_rules.snapshot().config.rules[0].delay_ms,15);
}

#[tokio::test]
async fn proxy_delay_preserves_response_and_rate_limit_never_connects_upstream(){
    tokio::time::timeout(Duration::from_secs(5),async{
        let directory=tempfile::tempdir().unwrap();let engine=Engine::open(directory.path()).unwrap();let mut settings=serde_json::to_value(config(80,Some(1),"capture")).unwrap();settings["rules"][0]["rateLimit"]["windowMs"]=json!(60000);apply(&engine,serde_json::from_value(settings).unwrap());
        let origin=Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());let cloned=origin.clone();
        let server=tokio::spawn(async move{let (stream,_)=cloned.accept().await.unwrap();let mut io=BufReader::new(stream);read_head(&mut io).await.unwrap();io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n\0ok\xff").await.unwrap()});
        let proxy=proxy::start(engine.clone(),0).await.unwrap();let url=format!("http://{}/api?private=secret",origin.local_addr().unwrap());
        let start=Instant::now();assert_eq!(get(proxy.address,&url).await,(200,vec![0,b'o',b'k',255]));assert!(start.elapsed()>=Duration::from_millis(75));server.await.unwrap();
        let (status,body)=get(proxy.address,&url).await;assert_eq!(status,429);assert!(!body.windows(6).any(|b|b==b"secret"));no_connection(&origin).await;
        let blocked=engine.store.list().unwrap().into_iter().find(|flow|flow.state=="blocked").unwrap();let response=blocked.response.unwrap();
        assert!(response.headers.iter().any(|h|h.name=="Retry-After"&&h.value.parse::<u32>().is_ok_and(|n|(1..=60).contains(&n))));assert!(response.upstream_tls.is_none()&&response.sent_request_headers.is_none());
        proxy.stop().await;
    }).await.unwrap();
}

#[tokio::test]
async fn cancelling_delay_leaves_no_background_send(){
    let directory=tempfile::tempdir().unwrap();let engine=Engine::open(directory.path()).unwrap();apply(&engine,config(30000,None,"all"));
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let request=draft(format!("http://{}/api",listener.local_addr().unwrap()));
    let id=engine.executions.prepare().unwrap();let cloned=engine.clone();let execution=id.clone();let task=tokio::spawn(async move{cloned.replay(request,None,Some(execution)).await.unwrap()});
    tokio::time::timeout(Duration::from_secs(2),async{while engine.store.get(&id).unwrap().is_none(){tokio::task::yield_now().await}}).await.unwrap();
    assert!(engine.executions.cancel(&id));let flow=tokio::time::timeout(Duration::from_secs(1),task).await.unwrap().unwrap();assert_eq!(flow.state,"cancelled");no_connection(&listener).await;
}

#[tokio::test]
async fn first_match_and_final_script_url_are_used_once(){
    let directory=tempfile::tempdir().unwrap();let engine=Engine::open(directory.path()).unwrap();
    let mut value=serde_json::to_value(config(0,Some(1),"all")).unwrap();let mut second=value["rules"][0].clone();second["id"]=json!("second");second["delayMs"]=json!(30000);value["rules"].as_array_mut().unwrap().push(second);
    apply(&engine,serde_json::from_value(value).unwrap());
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}/api",listener.local_addr().unwrap());
    // First matching request consumes one slot, even if upstream later fails.
    let mut request=draft(url.clone());request.method="HEAD".into();
    let server=tokio::spawn(async move{let (stream,_)=listener.accept().await.unwrap();let mut io=BufReader::new(stream);read_head(&mut io).await.unwrap();io.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap()});
    let flow=tokio::time::timeout(Duration::from_secs(2),engine.replay(request.clone(),None,None)).await.unwrap().unwrap();assert_eq!(flow.response.unwrap().status,204);server.await.unwrap();
    request.url=url.replace("/api","/before");request.scripts.enabled=true;request.scripts.before=format!("request.url = {};",serde_json::to_string(&url).unwrap());
    let flow=engine.replay(request,None,None).await.unwrap();assert_eq!(flow.response.as_ref().unwrap().status,429);assert!(flow.response.unwrap().body_base64.is_empty());assert!(flow.notes.iter().any(|note|note.contains("test-rule")));
}

#[test]
fn migration_includes_rules_disables_them_and_blocks_old_writers(){
    let source_dir=tempfile::tempdir().unwrap();let source=Engine::open(source_dir.path()).unwrap();apply(&source,config(100,Some(5),"all"));
    let document=config_migration::export(&source.store,BTreeMap::new(),Default::default(),"0.1.99").unwrap().document;
    let json:Value=serde_json::from_str(&document).unwrap();assert_eq!(json["sections"]["faults"]["data"]["enabled"],false);assert_eq!(json["sections"]["faults"]["data"]["rules"][0]["delayMs"],100);
    let target_dir=tempfile::tempdir().unwrap();let target=Engine::open(target_dir.path()).unwrap();let old=target.fault_rules.snapshot().version;
    let preview=config_migration::preview(&target.store,&document).unwrap();let prepared=config_migration::prepare_import(&target.store,&document,&["faults".into()],&preview.expected_revision,BTreeMap::new(),"0.1.99").unwrap();config_migration::commit_import(&target.store,prepared).unwrap();
    assert!(target.fault_rules.configure(config(1,None,"all"),&old).is_err());drop(target);
    let target=Engine::open(target_dir.path()).unwrap();let config=target.fault_rules.snapshot().config;assert!(!config.enabled);assert_eq!(config.rules[0].rate_limit.as_ref().unwrap().requests,5);
}

#[tokio::test]
async fn concurrent_requests_share_one_rule_counter_without_overshoot(){
    tokio::time::timeout(Duration::from_secs(5),async{
        let directory=tempfile::tempdir().unwrap();let engine=Engine::open(directory.path()).unwrap();
        let mut value=serde_json::to_value(config(0,Some(3),"all")).unwrap();value["rules"][0]["rateLimit"]["windowMs"]=json!(60000);apply(&engine,serde_json::from_value(value).unwrap());
        let listener=Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());let request=draft(format!("http://{}/api",listener.local_addr().unwrap()));
        let origin=listener.clone();let server=tokio::spawn(async move{for _ in 0..3{let (stream,_)=origin.accept().await.unwrap();let mut io=BufReader::new(stream);read_head(&mut io).await.unwrap();io.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();}});
        let mut tasks=vec![];for _ in 0..20 {let engine=engine.clone();let request=request.clone();tasks.push(tokio::spawn(async move{engine.replay(request,None,None).await.unwrap()}));}
        let mut blocked=0;let mut allowed=0;for task in tasks {let flow=task.await.unwrap();if flow.response.as_ref().is_some_and(|r|r.status==429){blocked+=1}else{assert_eq!(flow.response.unwrap().status,204);allowed+=1}}
        assert_eq!((allowed,blocked),(3,17));server.await.unwrap();no_connection(&listener).await;
    }).await.unwrap();
}

#[tokio::test]
async fn fixed_window_reset_scope_and_explicit_save_reset_are_observable(){
    tokio::time::timeout(Duration::from_secs(5),async{
        let directory=tempfile::tempdir().unwrap();let engine=Engine::open(directory.path()).unwrap();
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let request=draft(format!("http://{}/api",listener.local_addr().unwrap()));
        let server=tokio::spawn(async move{loop{let (stream,_)=listener.accept().await.unwrap();let mut io=BufReader::new(stream);read_head(&mut io).await.unwrap();io.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();}});
        let mut value=serde_json::to_value(config(0,Some(1),"all")).unwrap();value["rules"][0]["rateLimit"]["windowMs"]=json!(500);let short:Config=serde_json::from_value(value).unwrap();apply(&engine,short.clone());
        assert_eq!(engine.replay(request.clone(),None,None).await.unwrap().response.unwrap().status,204);assert_eq!(engine.replay(request.clone(),None,None).await.unwrap().response.unwrap().status,429);
        tokio::time::sleep(Duration::from_millis(550)).await;assert_eq!(engine.replay(request.clone(),None,None).await.unwrap().response.unwrap().status,204);
        apply(&engine,short);assert_eq!(engine.replay(request.clone(),None,None).await.unwrap().response.unwrap().status,204);
        apply(&engine,config(0,Some(1),"capture"));for _ in 0..2 {assert_eq!(engine.replay(request.clone(),None,None).await.unwrap().response.unwrap().status,204);}
        assert_eq!(engine.execute(request.clone(),None,"capture",None).await.unwrap().response.unwrap().status,204);assert_eq!(engine.execute(request,None,"capture",None).await.unwrap().response.unwrap().status,429);server.abort();let _=server.await;
    }).await.unwrap();
}
