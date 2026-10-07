use capture_core::{Engine, model::*, intercept::*};
use std::{sync::Arc, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};
fn draft(url: String) -> RequestDraft {
    serde_json::from_value(serde_json::json!({"engine":"native","method":"GET","url":url,"headers":[],"bodyBase64":""})).unwrap()
}
async fn bounded<T>(f: impl std::future::Future<Output=T>) -> T {
    tokio::time::timeout(Duration::from_secs(3), f).await.expect("operation stalled")
}
async fn waiting(e: &Engine) {
    bounded(async { while e.intercept.snapshot().items.is_empty() { tokio::task::yield_now().await; } }).await;
}
#[tokio::test]
async fn cancel_before_start_records_same_id_without_sending() {
    let temp=tempfile::tempdir().unwrap(); let e=Engine::open(temp.path()).unwrap();
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
    let id=e.executions.prepare().unwrap(); assert!(e.executions.cancel(&id));
    let f=e.replay(draft(format!("http://{}/",listener.local_addr().unwrap())),None,Some(id.clone())).await.unwrap();
    assert_eq!(f.id,id); assert!(f.error.unwrap().contains("用户取消")); assert!(f.response.is_none());
    assert!(!e.executions.cancel(&id));
    assert!(e.replay(draft("http://localhost/".into()),None,Some(id.clone())).await.is_err());
    assert_eq!(e.store.list().unwrap()[0].id,id);
    assert!(tokio::time::timeout(Duration::from_millis(50),listener.accept()).await.is_err());
}
#[tokio::test]
async fn cancelling_network_closes_socket_without_affecting_other_request() {
    let temp=tempfile::tempdir().unwrap(); let e=Engine::open(temp.path()).unwrap();
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url=format!("http://{}/",listener.local_addr().unwrap());
    let id=e.executions.prepare().unwrap(); let other=e.clone(); let aid=id.clone(); let aurl=url.clone();
    let a=tokio::spawn(async move {other.replay(draft(aurl),None,Some(aid)).await.unwrap()});
    let (mut socket,_)=bounded(listener.accept()).await.unwrap(); let mut buf=[0;4096];
    assert!(bounded(socket.read(&mut buf)).await.unwrap()>0);
    assert!(e.replay(draft(url.clone()),None,Some(id.clone())).await.is_err());
    let other=e.clone(); let b=tokio::spawn(async move {other.replay(draft(url),None,None).await.unwrap()});
    let (mut second,_)=bounded(listener.accept()).await.unwrap();
    assert!(bounded(second.read(&mut buf)).await.unwrap()>0);
    e.executions.cancel(&id);
    let f=bounded(a).await.unwrap(); assert!(f.error.unwrap().contains("用户取消"));
    assert_eq!(bounded(socket.read(&mut buf)).await.unwrap(),0);
    second.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap(); drop(second);
    let f=bounded(b).await.unwrap(); assert!(f.error.is_none()); assert_eq!(f.response.unwrap().status,200);
}
#[tokio::test]
async fn cancellation_cleans_request_and_response_breakpoints() {
    for stage in ["request","response"] {
        let temp=tempfile::tempdir().unwrap(); let e=Engine::open(temp.path()).unwrap();
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        e.intercept.configure(Config{request:stage=="request",response:stage=="response",scope:"replay".into(),rules:vec![Rule{url_contains:"127.0.0.1".into(),..Default::default()}]}).unwrap();
        let id=e.executions.prepare().unwrap(); let other=e.clone(); let aid=id.clone(); let url=format!("http://{}/",listener.local_addr().unwrap());
        let task=tokio::spawn(async move {other.replay(draft(url),None,Some(aid)).await.unwrap()});
        if stage=="response" {
            let(mut socket,_)=bounded(listener.accept()).await.unwrap();let mut buf=[0;4096];bounded(socket.read(&mut buf)).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        }
        waiting(&e).await; assert_eq!(e.intercept.snapshot().items[0].stage,stage);
        e.executions.cancel(&id); assert!(bounded(task).await.unwrap().error.unwrap().contains("用户取消"));
        assert!(e.intercept.snapshot().items.is_empty());
    }
}
#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn cancelling_script_interrupts_and_does_not_apply_variables() {
    let temp=tempfile::tempdir().unwrap();let e=Engine::open(temp.path()).unwrap();
    let mut request=draft("http://localhost/".into());request.scripts.enabled=true;
    request.scripts.before="variables.changed='yes'; while(true) {}".into();
    let id=e.executions.prepare().unwrap();let other=Arc::clone(&e);let aid=id.clone();
    let task=tokio::spawn(async move {other.replay(request,None,Some(aid)).await.unwrap()});
    tokio::time::sleep(Duration::from_millis(100)).await;
    e.executions.cancel(&id);
    let f=tokio::time::timeout(Duration::from_millis(500),task).await.unwrap().unwrap();
    assert!(f.error.unwrap().contains("用户取消"));assert!(!f.request.scripts.variables.contains_key("changed"));
}
