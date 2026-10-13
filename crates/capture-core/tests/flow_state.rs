use capture_core::{model::Flow, store::Store};
fn sample() -> Flow { serde_json::from_value(serde_json::json!({"id":"test","startedAt":0,"durationMs":0,"source":"capture","request":{"method":"GET","url":"https://example.invalid","headers":[],"bodyBase64":""},"response":null,"error":null,"notes":[]})).unwrap() }
#[test]
fn lifecycle_import_and_restart_do_not_leave_false_pending_records() {
 let temp=tempfile::tempdir().unwrap();let path=temp.path().join("test.db");
 let store=Store::open(&path).unwrap();
 assert_eq!(sample().state,"unknown");
 let mut pending=sample();pending.state="sending".into();store.insert(&pending).unwrap();
 let id=store.begin_import().unwrap();store.append_import(&id,0,vec![pending.clone()]).unwrap();store.finish_import(&id,true).unwrap();
 assert!(store.list().unwrap().iter().any(|f|f.state=="imported"));
 drop(store);let store=Store::open(&path).unwrap();
 assert_eq!(store.get("test").unwrap().unwrap().state,"interrupted");
 assert!(store.list().unwrap().iter().any(|f|f.state=="imported"));
}
