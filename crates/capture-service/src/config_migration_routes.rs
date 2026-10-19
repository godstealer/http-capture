use super::{Shared,ApiError,error};
use axum::{extract::{State,DefaultBodyLimit},routing::{get,post},Json,Router};
use capture_core::config_migration::{self as migration,ExportOptions,Exported,Preview,Imported};
use serde::Deserialize;
use std::collections::BTreeMap;

pub fn router()->Router<Shared>{Router::new().route("/config/status",get(status)).route("/config/export",post(export)).route("/config/preview",post(preview)).route("/config/import",post(import)).route("/config/recovery",post(recovery)).layer(DefaultBodyLimit::max(40*1024*1024))}
async fn status(State(s):State<Shared>)->Result<Json<serde_json::Value>,ApiError>{migration::status(&s.engine.store).map(Json).map_err(error)}
#[derive(Deserialize)]#[serde(deny_unknown_fields)]struct Export{preferences:BTreeMap<String,String>,options:ExportOptions}
async fn export(State(s):State<Shared>,Json(p):Json<Export>)->Result<Json<Exported>,ApiError>{let store=s.engine.store.clone();tokio::task::spawn_blocking(move||migration::export(&store,p.preferences,p.options,concat!(env!("CARGO_PKG_VERSION"),"-development"))).await.map_err(error)?.map(Json).map_err(error)}
#[derive(Deserialize)]#[serde(deny_unknown_fields)]struct Document{document:String}
async fn preview(State(s):State<Shared>,Json(p):Json<Document>)->Result<Json<Preview>,ApiError>{let store=s.engine.store.clone();tokio::task::spawn_blocking(move||migration::preview(&store,&p.document)).await.map_err(error)?.map(Json).map_err(error)}
#[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]struct Import{document:String,sections:Vec<String>,expected_revision:String,#[serde(default)]preferences:BTreeMap<String,String>}
async fn import(State(s):State<Shared>,Json(p):Json<Import>)->Result<Json<Imported>,ApiError>{
    let _migration=s.configuration_migration.lock().await;
    let store=s.engine.store.clone();
    let prepared=tokio::task::spawn_blocking(move||migration::prepare_import(&store,&p.document,&p.sections,&p.expected_revision,p.preferences,concat!(env!("CARGO_PKG_VERSION"),"-development"))).await.map_err(error)?.map_err(error)?;
    let stopped=s.shutdown_network().await;
    if let Err(failure)=stopped{resume_after_failure(&s).await;return Err(error(format!("配置未导入：{failure}。已停止的监听不会自动重启，请检查后重试。")));}
    let store=s.engine.store.clone();let result=tokio::task::spawn_blocking(move||migration::commit_import(&store,prepared)).await.map_err(|e|e.to_string()).and_then(|r|r.map_err(|e|format!("{e:#}")));
    match result {Ok(value)=>Ok(Json(value)),Err(failure)=>{resume_after_failure(&s).await;Err(error(format!("配置未导入：{failure}。原配置保留；监听已停止，如需继续使用外部 MCP 请重启内核。")))}}
}
async fn resume_after_failure(s:&Shared){if s.engine.store.configuration_ready().is_ok()&&s.mcp_clients.resume().await.is_ok(){s.resume_network_commands();}}
async fn recovery(State(s):State<Shared>)->Result<Json<Exported>,ApiError>{let store=s.engine.store.clone();tokio::task::spawn_blocking(move||migration::recovery(&store)).await.map_err(error)?.map(Json).map_err(error)}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{router as host_router,Service};
    use serde_json::{json,Value};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt,AsyncWriteExt};
    async fn fixture()->(tempfile::TempDir,Shared,std::net::SocketAddr,tokio::task::JoinHandle<()>){
        let temp=tempfile::tempdir().unwrap();let mut engine=capture_core::Engine::open(temp.path()).unwrap();
        // No test may read/restore a real user's OS proxy recovery journal.
        Arc::get_mut(&mut engine).unwrap().system_proxy=capture_core::system_proxy::SystemProxy::with_state_directory(temp.path().join("isolated-system-proxy"));
        let state=Service::new(engine,"config-contract".into()).unwrap();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let served=state.clone();
        let task=tokio::spawn(async move{axum::serve(listener,host_router(served)).await.unwrap()});(temp,state,address,task)
    }
    async fn raw(address:std::net::SocketAddr,path:&str,body:&str,authorized:bool)->(u16,Value){
        let mut socket=tokio::net::TcpStream::connect(address).await.unwrap();let token=if authorized{"Authorization: Bearer config-contract\r\n"}else{""};
        socket.write_all(format!("POST {path} HTTP/1.1\r\nHost: localhost\r\n{token}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
        if authorized {socket.write_all(body.as_bytes()).await.unwrap();} // Auth must reject before reading even a malformed body.
        let mut bytes=Vec::new();socket.read_to_end(&mut bytes).await.unwrap();let split=bytes.windows(4).position(|v|v==b"\r\n\r\n").unwrap();let status=std::str::from_utf8(&bytes[..split]).unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
        (status,serde_json::from_slice(&bytes[split+4..]).unwrap_or(Value::Null))
    }
    async fn call(address:std::net::SocketAddr,path:&str,body:Value)->(u16,Value){raw(address,path,&body.to_string(),true).await}
    #[tokio::test]
    async fn authentication_and_invalid_or_stale_files_do_not_stop_capture_or_write_configuration(){
        tokio::time::timeout(std::time::Duration::from_secs(30),async{
            let (_temp,state,address,task)=fixture().await;state.runtime.start_proxy("127.0.0.1".into(),0).await.unwrap();
            for path in ["/config/export","/config/preview","/config/import","/config/recovery"] {assert_eq!(raw(address,path,"{malformed",false).await.0,401);}
            assert_eq!(call(address,"/config/preview",json!({"document":"{broken}"})).await.0,400);
            assert_eq!(call(address,"/config/import",json!({"document":"{broken}","sections":["preferences"],"expectedRevision":"stale"})).await.0,400);
            let (code,snapshot)=call(address,"/config/export",json!({"preferences":{"http-capture.theme":"matrix"},"options":{}})).await;assert_eq!(code,200);
            let (_,plan)=call(address,"/config/preview",json!({"document":snapshot["document"]})).await;
            state.engine.store.save_setting("decryption.bypass.v1",&vec!["new.invalid"]).unwrap();
            let before=state.engine.store.workspace_revisions().unwrap();
            assert_eq!(call(address,"/config/import",json!({"document":snapshot["document"],"sections":["preferences"],"expectedRevision":plan["expectedRevision"]})).await.0,400);
            assert!(state.runtime.status().await.running);assert!(!state.shutting_down.load(std::sync::atomic::Ordering::Acquire));assert!(!state.engine.store.configuration_frozen());assert_eq!(state.engine.store.workspace_revisions().unwrap(),before);assert!(state.engine.store.setting::<Value>("config-migration.recovery.v1").unwrap().is_none());
            state.shutdown_network().await.unwrap();task.abort();let _=task.await;
        }).await.expect("configuration preflight contract stalled");
    }
    #[tokio::test]
    async fn successful_import_stops_network_freezes_writes_and_retains_a_local_recovery_point(){
        tokio::time::timeout(std::time::Duration::from_secs(30),async{
            let (_temp,state,address,task)=fixture().await;state.runtime.start_proxy("127.0.0.1".into(),0).await.unwrap();
            let (_,snapshot)=call(address,"/config/export",json!({"preferences":{"http-capture.theme":"matrix"},"options":{}})).await;
            let (_,plan)=call(address,"/config/preview",json!({"document":snapshot["document"]})).await;
            let (code,result)=call(address,"/config/import",json!({"document":snapshot["document"],"sections":["preferences"],"expectedRevision":plan["expectedRevision"],"preferences":{"http-capture.theme":"paper"}})).await;assert_eq!(code,200);assert_eq!(result["preferences"]["http-capture.theme"],"matrix");assert_eq!(result["requiresRestart"],true);
            assert!(!state.runtime.status().await.running);assert!(!state.engine.system_proxy.status().active);assert!(state.shutting_down.load(std::sync::atomic::Ordering::Acquire));assert!(state.engine.store.configuration_frozen());
            assert_eq!(call(address,"/start",json!({"port":8080})).await.0,400);assert_eq!(call(address,"/decryption",json!({"hosts":["forbidden.invalid"]})).await.0,400);
            let(code,recovery)=call(address,"/config/recovery",json!({})).await;assert_eq!(code,200);let package:Value=serde_json::from_str(recovery["document"].as_str().unwrap()).unwrap();assert_eq!(package["sections"]["preferences"]["data"]["http-capture.theme"],"paper");
            task.abort();let _=task.await;
        }).await.expect("configuration import contract stalled");
    }
    #[tokio::test]
    async fn concurrent_imports_and_failure_recovery_cannot_reopen_a_frozen_runtime(){
        tokio::time::timeout(std::time::Duration::from_secs(30),async{
            let (_temp,state,address,task)=fixture().await;
            let exported=migration::export(&state.engine.store,BTreeMap::new(),ExportOptions::default(),"0.1.99").unwrap();
            let preview=migration::preview(&state.engine.store,&exported.document).unwrap();
            let input=json!({"document":exported.document,"sections":["preferences"],"expectedRevision":preview.expected_revision});
            let (one,two)=tokio::join!(call(address,"/config/import",input.clone()),call(address,"/config/import",input));
            let mut statuses=[one.0,two.0];statuses.sort();assert_eq!(statuses,[200,400]);
            assert!(state.engine.store.configuration_frozen());assert!(state.shutting_down.load(std::sync::atomic::Ordering::Acquire));
            // Simulate a losing caller's late error cleanup after the winner
            // committed. It must never undo the winner's shutdown gate.
            resume_after_failure(&state).await;
            assert!(state.shutting_down.load(std::sync::atomic::Ordering::Acquire));
            // A separate lifecycle owner cannot bypass the persisted import
            // epoch by clearing only its in-memory shutdown flag either.
            state.resume_network_commands();assert!(state.ensure_network_ready().is_err());
            task.abort();let _=task.await;
        }).await.expect("concurrent import contract stalled");
    }
    #[tokio::test]
    async fn document_limit_is_sixteen_mib_and_json_envelope_exceeds_default_twelve_mib_limit(){
        tokio::time::timeout(std::time::Duration::from_secs(60),async{
            let (_temp,state,address,task)=fixture().await;
            let exported=migration::export(&state.engine.store,BTreeMap::new(),ExportOptions::default(),"0.1.99").unwrap();
            let mut document=exported.document;document.extend(std::iter::repeat_n(' ',migration::MAX_DOCUMENT_BYTES-document.len()));
            assert_eq!(document.len(),migration::MAX_DOCUMENT_BYTES);
            // The route-specific 40 MiB JSON envelope must override the host's
            // 12 MiB default, while the core enforces decoded document bytes.
            assert_eq!(call(address,"/config/preview",json!({"document":document})).await.0,200);
            document.push(' ');assert_eq!(call(address,"/config/preview",json!({"document":document})).await.0,400);
            let escaped="\\".repeat(migration::MAX_DOCUMENT_BYTES);let wrapped=json!({"document":escaped}).to_string();assert!(wrapped.len()>30*1024*1024);assert_eq!(raw(address,"/config/preview",&wrapped,true).await.0,400);
            assert!(!state.engine.store.configuration_frozen());task.abort();let _=task.await;
        }).await.expect("configuration size contract stalled");
    }
}
