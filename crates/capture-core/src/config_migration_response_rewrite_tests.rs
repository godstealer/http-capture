use super::*;

fn saved_rule() -> Value {
    json!({"enabled":true,"scope":"capture","rules":[{
        "id":"response-template","name":"Local API template","enabled":true,
        "matchType":"urlRegex","pattern":"^https://example[.]invalid/api$","method":"GET",
        "action":"replace","replacement":{"status":201,"headers":[{"name":"X-Private","value":"private-value"}],"bodyBase64":"cHJpdmF0ZS1ib2R5"},"modifications":[]
    }]})
}

#[test]
fn response_templates_are_opt_in_data_and_export_disabled() {
    let directory=tempfile::tempdir().unwrap();
    let store=Store::open(&directory.path().join("source.db")).unwrap();
    store.save_setting("response-rewrite.v1",&saved_rule()).unwrap();
    let excluded=export(&store,BTreeMap::new(),ExportOptions::default(),"test").unwrap();
    let package=parse_document(&excluded.document).unwrap();
    assert_eq!(package.sections["responseRewrite"].data["rules"],json!([]));
    assert!(!excluded.document.contains("private-value"));
    assert!(!excluded.document.contains("cHJpdmF0ZS1ib2R5"));
    let included=export(&store,BTreeMap::new(),ExportOptions{include_request_data:true,..Default::default()},"test").unwrap();
    let package=parse_document(&included.document).unwrap();
    let rewrites=&package.sections["responseRewrite"].data;
    assert_eq!(rewrites["enabled"],false);
    assert_eq!(rewrites["rules"][0]["enabled"],false);
    assert_eq!(rewrites["rules"][0]["replacement"]["bodyBase64"],"cHJpdmF0ZS1ib2R5");
    assert_eq!(rewrites["rules"][0]["replacement"]["headers"][0]["value"],"private-value");
    assert_eq!(store.setting::<Value>("response-rewrite.v1").unwrap().unwrap(),saved_rule());
}

#[test]
fn import_validates_and_disables_rewrites_without_requiring_the_new_category() {
    let directory=tempfile::tempdir().unwrap();
    let source=Store::open(&directory.path().join("source.db")).unwrap();
    let target=Store::open(&directory.path().join("target.db")).unwrap();
    source.save_setting("response-rewrite.v1",&saved_rule()).unwrap();
    let document=export(&source,BTreeMap::new(),ExportOptions{include_request_data:true,..Default::default()},"test").unwrap().document;
    let mut legacy:Value=serde_json::from_str(&document).unwrap();
    legacy["sections"].as_object_mut().unwrap().remove("responseRewrite");
    assert!(preview(&target,&legacy.to_string()).is_ok());
    let mut bad:Value=serde_json::from_str(&document).unwrap();
    bad["sections"]["responseRewrite"]["data"]["rules"][0]["pattern"]=json!("[");
    assert!(preview(&target,&bad.to_string()).is_err());
    assert!(target.setting::<Value>("response-rewrite.v1").unwrap().is_none());
    let mut enabled:Value=serde_json::from_str(&document).unwrap();
    enabled["sections"]["responseRewrite"]["data"]["enabled"]=json!(true);
    enabled["sections"]["responseRewrite"]["data"]["rules"][0]["enabled"]=json!(true);
    let enabled=enabled.to_string();
    let plan=preview(&target,&enabled).unwrap();
    let prepared=prepare_import(&target,&enabled,&["responseRewrite".into()],&plan.expected_revision,BTreeMap::new(),"test").unwrap();
    commit_import(&target,prepared).unwrap();
    let imported=target.setting::<Value>("response-rewrite.v1").unwrap().unwrap();
    assert_eq!(imported["enabled"],false);
    assert_eq!(imported["rules"][0]["enabled"],false);
    assert_eq!(imported["rules"][0]["replacement"]["bodyBase64"],"cHJpdmF0ZS1ib2R5");
}
