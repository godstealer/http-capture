use capture_core::{Engine, intercept::{Config, Rule}, scripts::Scripts, store::{Library,Workspace,RequestEditor}};
use std::collections::BTreeMap;

#[test]
fn restart_restores_drafts_rules_scripts_and_committed_variables() {
 let dir=tempfile::tempdir().unwrap();
 let engine=Engine::open(dir.path()).unwrap();
 assert!(engine.store.workspace().unwrap().editors.is_empty());
 engine.intercept.configure(Config{request:true,response:false,scope:"capture".into(),rules:vec![Rule{url_contains:"^https://example\\.test/".into(),url_regex:true,..Default::default()}]}).unwrap();
 let scripts=Scripts{enabled:true,before:"variables.counter = '2';".into(),variables:BTreeMap::from([("counter".into(),"1".into()),("removed".into(),"value".into())]),..Default::default()};
 engine.scripts.configure(scripts.clone()).unwrap();
 let revision=engine.scripts.snapshot().1;
 engine.scripts.commit_variables(revision,&scripts.variables,&BTreeMap::from([("counter".into(),"2".into())])).unwrap();
 let request=serde_json::from_str(r#"{"method":"POST","url":"","engine":"h2","headers":[{"name":"X-A","value":"1"},{"name":"X-B","value":"2"},{"name":"X-A","value":"3"}],"bodyBase64":"AAEC/w=="}"#).unwrap();
 engine.store.save_workspace(Workspace{editors:vec![RequestEditor{id:"draft-1".into(),draft:request,selected:None,parent_id:None,grpc_draft:Some(capture_core::store::GrpcDraft{method:"/demo.Echo/Bidi".into(),json:r#"{"text":"{{message}}""#.into()}),saved_request_id:None}],active_editor:"draft-1".into(),revision:0}).unwrap();
 drop(engine);
 let engine=Engine::open(dir.path()).unwrap();
 let config=engine.intercept.snapshot();assert!(config.config.request);assert!(config.items.is_empty());assert!(config.config.rules[0].compiled_url.as_ref().unwrap().is_match("https://example.test/path"));
 let scripts=engine.scripts.snapshot().0;assert!(scripts.enabled);assert_eq!(scripts.variables.get("counter").unwrap(),"2");assert!(!scripts.variables.contains_key("removed"));
 let work=engine.store.workspace().unwrap();assert_eq!(work.active_editor,"draft-1");assert_eq!(work.editors[0].draft.headers[2].name,"X-A");assert_eq!(work.editors[0].draft.body_base64.as_str(),"AAEC/w==");assert!(work.editors[0].draft.url.is_empty());
 let grpc=work.editors[0].grpc_draft.as_ref().unwrap();assert_eq!(grpc.method,"/demo.Echo/Bidi");assert_eq!(grpc.json,r#"{"text":"{{message}}""#);
 engine.store.save_workspace(Workspace{revision:work.revision,..Default::default()}).unwrap();drop(engine);
 assert!(Engine::open(dir.path()).unwrap().store.workspace().unwrap().editors.is_empty());
}

#[test]
fn stale_windows_and_invalid_settings_cannot_overwrite_saved_data() {
 let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();
 let old=engine.store.workspace().unwrap();assert_eq!(engine.store.save_workspace(old.clone()).unwrap(),1);
 assert!(engine.store.save_workspace(old).is_err());assert_eq!(engine.store.workspace().unwrap().revision,1);
 engine.intercept.configure(Config{scope:"all".into(),..Default::default()}).unwrap();
 assert!(engine.intercept.configure(Config{scope:"all".into(),rules:vec![Rule{url_contains:"[".into(),url_regex:true,..Default::default()}],..Default::default()}).is_err());
 let before=engine.scripts.snapshot().1;
 engine.scripts.configure(Scripts{variables:BTreeMap::from([("version".into(),"new".into())]),..Default::default()}).unwrap();
 engine.scripts.commit_variables(before,&BTreeMap::new(),&BTreeMap::from([("version".into(),"old".into())])).unwrap();
 assert!(engine.scripts.configure(Scripts{before:"x".repeat(65537),..Default::default()}).is_err());
 drop(engine);let restored=Engine::open(dir.path()).unwrap();assert!(restored.intercept.snapshot().config.rules.is_empty());assert_eq!(restored.scripts.snapshot().0.variables["version"],"new");
}

#[test]
fn legacy_workspace_without_grpc_draft_loads() {
 let value:RequestEditor=serde_json::from_value(serde_json::json!({"id":"old","draft":{"method":"GET","url":"","headers":[],"bodyBase64":""},"selected":null,"parentId":null})).unwrap();
 assert!(value.grpc_draft.is_none());
}

#[test]
fn legacy_workspace_without_saved_request_id_loads_and_omits_it() {
 let value:Workspace=serde_json::from_value(serde_json::json!({
  "editors":[{"id":"old","draft":{"method":"GET","url":"","headers":[],"bodyBase64":""},"selected":null,"parentId":null}],
  "activeEditor":"old","revision":3
 })).unwrap();
 assert!(value.editors[0].saved_request_id.is_none());
 let serialized=serde_json::to_value(value).unwrap();
 assert!(serialized["editors"][0].get("savedRequestId").is_none());
 assert!(serialized["editors"][0].get("saved_request_id").is_none());
}

#[test]
fn saved_request_association_roundtrips_without_library_entry() {
 let dir=tempfile::tempdir().unwrap();
 let value:Workspace=serde_json::from_value(serde_json::json!({
  "editors":[{"id":"linked","draft":{"method":"GET","url":"https://example.invalid/","headers":[],"bodyBase64":""},"selected":null,"parentId":null,"savedRequestId":"saved-request-1"}],
  "activeEditor":"linked","revision":0
 })).unwrap();
 assert_eq!(value.editors[0].saved_request_id.as_deref(),Some("saved-request-1"));
 let serialized=serde_json::to_value(&value).unwrap();
 assert_eq!(serialized["editors"][0]["savedRequestId"],"saved-request-1");
 assert!(serialized["editors"][0].get("saved_request_id").is_none());
 let roundtrip:Workspace=serde_json::from_value(serialized).unwrap();
 {
  let engine=Engine::open(dir.path()).unwrap();
  assert!(engine.store.library().unwrap().requests.is_empty());
  engine.store.save_workspace(roundtrip).unwrap();
 }
 let restored=Engine::open(dir.path()).unwrap().store.workspace().unwrap();
 assert_eq!(restored.editors[0].saved_request_id.as_deref(),Some("saved-request-1"));
}

#[test]
fn legacy_library_without_request_editor_metadata_loads_and_omits_it() {
 let value:Library=serde_json::from_value(serde_json::json!({
  "revision":4,"collections":[{"id":"c","name":"Saved"}],"environments":[],"scripts":[],
  "requests":[{"id":"old","name":"Old request","collectionId":"c","draft":{"method":"GET","url":"","headers":[],"bodyBase64":""}}]
 })).unwrap();
 assert!(value.requests[0].grpc_draft.is_none());
 assert!(value.requests[0].request_mode.is_none());
 let serialized=serde_json::to_value(value).unwrap();
 assert!(serialized["requests"][0].get("grpcDraft").is_none());
 assert!(serialized["requests"][0].get("requestMode").is_none());
}

#[test]
fn library_preserves_grpc_message_and_request_modes_across_restart() {
 let dir=tempfile::tempdir().unwrap();
 let value:Library=serde_json::from_value(serde_json::json!({
  "revision":0,"collections":[{"id":"c","name":"Saved"}],"environments":[],"scripts":[],
  "requests":[
   {"id":"g","name":"gRPC request","collectionId":"c","requestMode":"grpc","grpcDraft":{"method":"/demo.Echo/Call","json":"{\"text\":\"{{message}}\"}"},"draft":{"method":"POST","url":"https://example.invalid/","headers":[],"bodyBase64":""}},
   {"id":"w","name":"WebSocket request","collectionId":"c","requestMode":"websocket","draft":{"method":"GET","url":"wss://example.invalid/socket","headers":[],"bodyBase64":""}},
   {"id":"h","name":"HTTP request","collectionId":"c","requestMode":"http","draft":{"method":"GET","url":"https://example.invalid/","headers":[],"bodyBase64":""}}
  ]
 })).unwrap();
 let serialized=serde_json::to_value(&value).unwrap();
 assert_eq!(serialized["requests"][0]["requestMode"],"grpc");
 assert_eq!(serialized["requests"][0]["grpcDraft"]["method"],"/demo.Echo/Call");
 assert_eq!(serialized["requests"][0]["grpcDraft"]["json"],r#"{"text":"{{message}}"}"#);
 assert!(serialized["requests"][0].get("grpc_draft").is_none());
 assert!(serialized["requests"][0].get("request_mode").is_none());
 let roundtrip:Library=serde_json::from_value(serialized).unwrap();
 {
  let engine=Engine::open(dir.path()).unwrap();
  engine.store.save_library(roundtrip).unwrap();
 }
 let restored=Engine::open(dir.path()).unwrap().store.library().unwrap();
 assert_eq!(restored.requests.iter().map(|request|request.request_mode.as_deref()).collect::<Vec<_>>(),vec![Some("grpc"),Some("websocket"),Some("http")]);
 let grpc=restored.requests[0].grpc_draft.as_ref().unwrap();
 assert_eq!(grpc.method,"/demo.Echo/Call");
 assert_eq!(grpc.json,r#"{"text":"{{message}}"}"#);
 assert!(restored.requests[1].grpc_draft.is_none());
 assert!(restored.requests[2].grpc_draft.is_none());
}
