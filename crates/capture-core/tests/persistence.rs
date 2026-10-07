use capture_core::{Engine, intercept::{Config, Rule}, scripts::Scripts, store::{Workspace,RequestEditor}};
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
 engine.store.save_workspace(Workspace{editors:vec![RequestEditor{id:"draft-1".into(),draft:request,selected:None,parent_id:None}],active_editor:"draft-1".into(),revision:0}).unwrap();
 drop(engine);
 let engine=Engine::open(dir.path()).unwrap();
 let config=engine.intercept.snapshot();assert!(config.config.request);assert!(config.items.is_empty());assert!(config.config.rules[0].compiled_url.as_ref().unwrap().is_match("https://example.test/path"));
 let scripts=engine.scripts.snapshot().0;assert!(scripts.enabled);assert_eq!(scripts.variables.get("counter").unwrap(),"2");assert!(!scripts.variables.contains_key("removed"));
 let work=engine.store.workspace().unwrap();assert_eq!(work.active_editor,"draft-1");assert_eq!(work.editors[0].draft.headers[2].name,"X-A");assert_eq!(work.editors[0].draft.body_base64,"AAEC/w==");assert!(work.editors[0].draft.url.is_empty());
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
