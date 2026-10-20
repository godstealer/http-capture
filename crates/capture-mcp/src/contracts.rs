//! Protocol hints describe effects; permissions and compare-and-swap remain in
//! the execution paths. No annotation authorizes an otherwise disabled tool.
use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{json, Value};
use std::sync::Arc;

pub const VERSION: u32 = 1;

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required})
}
fn string() -> Value { json!({"type":"string"}) }
fn nullable_string() -> Value { json!({"type":["string","null"]}) }
fn integer() -> Value { json!({"type":"integer","minimum":0}) }
fn next_offset() -> Value { json!({"type":["integer","null"],"minimum":0}) }
fn array(items: Value) -> Value { json!({"type":"array","items":items}) }
fn header() -> Value {
    object(json!({"name":string(),"value":string(),"disabled":{"type":["boolean","integer","null"]}}), &["name","value"])
}
fn body_chunk() -> Value {
    object(json!({"offset":integer(),"totalBytes":integer(),"bodyBase64":string(),"text":{"type":["string","null"]},"textTruncated":{"type":"boolean"},"nextOffset":next_offset(),"mayContainPrivateData":{"type":"boolean"}}), &["offset","totalBytes","bodyBase64","nextOffset"])
}
fn projected_request() -> Value {
    object(json!({"method":string(),"url":string(),"headers":array(header()),"headersTruncated":{"type":"boolean"},"hasScripts":{"type":"boolean"},"bodyBytes":integer(),"body":body_chunk()}), &["method","url","headers","headersTruncated","hasScripts","bodyBytes"])
}
fn flow() -> Value {
    let request=object(json!({"method":string(),"url":string(),"engine":nullable_string(),"headers":array(header()),"headersTruncated":{"type":"boolean"}}), &["method","url","headers"]);
    let response=object(json!({"status":{"type":"integer"},"version":string(),"headers":array(header()),"trailers":array(header()),"headersTruncated":{"type":"boolean"},"trailersTruncated":{"type":"boolean"}}), &["status","version","headers","trailers"]);
    object(json!({"id":string(),"parentId":nullable_string(),"startedAt":integer(),"durationMs":{"type":["number","null"]},"source":string(),"state":string(),"clientProtocol":nullable_string(),"request":request,"response":{"anyOf":[response,{"type":"null"}]}}), &["id","request","response","source","state"])
}
fn versioned(result: Value) -> Value {
    object(json!({"result":result,"expectedRevision":integer(),"expectedLibraryRevision":integer()}), &["result","expectedRevision","expectedLibraryRevision"])
}
fn scripts() -> Value {
    object(json!({"enabled":{"type":"boolean"},"before":string(),"after":string(),"variables":{"type":"object","additionalProperties":{"type":"string"}},"modules":{"type":"object","properties":{"encoding":{"type":"boolean"},"crypto":{"type":"boolean"},"utils":{"type":"boolean"}}}}), &["enabled","before","after","variables","modules"])
}

pub fn output_schema(name: &str) -> Option<Value> {
    Some(match name {
        "capture_status"=>object(json!({"running":{"type":"boolean"},"address":nullable_string(),"sendEngines":array(object(json!({"id":string(),"available":{"type":"boolean"},"reason":nullable_string(),"profiles":array(string()),"browserVersions":{"type":"object","additionalProperties":{"type":"array","items":{"type":"integer"}}}}), &["id","available","reason","profiles","browserVersions"])),"upstreamEnabled":{"type":"boolean"}}), &["running","address","sendEngines","upstreamEnabled"]),
        "list_requests"=>object(json!({"requests":array(object(json!({"id":string(),"method":string(),"url":string(),"status":{"type":["integer","null"]},"state":string(),"source":string(),"startedAt":integer(),"durationMs":{"type":["number","null"]},"clientProtocol":nullable_string(),"upstreamProtocol":nullable_string()}), &["id","method","url","state","source"])),"nextCursor":nullable_string()}), &["requests","nextCursor"]),
        "get_request"|"send_request"=>flow(),
        "read_body"|"read_pending_body"=>body_chunk(),
        "cancel_request"=>object(json!({"cancelled":{"type":"boolean"}}), &["cancelled"]),
        "start_capture"=>object(json!({"running":{"const":true},"address":nullable_string()}), &["running","address"]),
        "stop_capture"=>object(json!({"running":{"const":false}}), &["running"]),
        "get_workspaces"=>object(json!({"expectedRevision":integer(),"expectedLibraryRevision":integer(),"activeEditor":nullable_string(),"editors":array(object(json!({"editorId":string(),"method":string(),"url":string(),"savedRequestId":nullable_string(),"mode":{"enum":["http","grpc"]}}), &["editorId","method","url","savedRequestId","mode"]))}), &["expectedRevision","expectedLibraryRevision","activeEditor","editors"]),
        "list_collections"=>{
            let mut schema=object(json!({"expectedLibraryRevision":integer(),"nextOffset":next_offset(),"collections":array(object(json!({"id":string(),"name":string(),"requestCount":integer()}), &["id","name","requestCount"])),"collection":object(json!({"id":string(),"name":string()}), &["id","name"]),"requests":array(object(json!({"id":string(),"name":string(),"method":string(),"url":string(),"mode":nullable_string()}), &["id","name","method","url","mode"]))}), &["expectedLibraryRevision","nextOffset"]);
            schema["oneOf"]=json!([{"required":["collections"]},{"required":["collection","requests"]}]);schema
        }
        "get_saved_request"=>object(json!({"expectedLibraryRevision":integer(),"id":string(),"name":string(),"collectionId":string(),"request":projected_request()}), &["expectedLibraryRevision","id","name","collectionId","request"]),
        "get_request_editor"=>object(json!({"expectedRevision":integer(),"expectedLibraryRevision":integer(),"editorId":string(),"savedRequestId":nullable_string(),"request":projected_request()}), &["expectedRevision","expectedLibraryRevision","editorId","savedRequestId","request"]),
        "create_collection"=>versioned(object(json!({"collectionId":string(),"name":string()}), &["collectionId","name"])),
        "create_request"=>versioned(object(json!({"editorId":string(),"created":{"const":true},"sent":{"const":false}}), &["editorId","created","sent"])),
        "update_request"=>versioned(object(json!({"editorId":string(),"updated":{"const":true},"sent":{"const":false}}), &["editorId","updated","sent"])),
        "save_request_to_collection"=>versioned(object(json!({"editorId":string(),"savedRequestId":string(),"collectionId":string(),"created":{"type":"boolean"}}), &["editorId","savedRequestId","collectionId","created"])),
        "get_interception"=>object(json!({"config":{"type":"object","properties":{"request":{"type":"boolean"},"response":{"type":"boolean"},"scope":{"enum":["all","capture","replay"]},"rules":{"type":"array","items":{"type":"object"}}},"required":["request","response","scope","rules"]},"expectedVersion":string(),"redacted":{"type":"boolean"},"scope":{"const":"breakpoints"},"warning":string()}), &["config","expectedVersion","redacted","scope","warning"]),
        "configure_interception"=>{let mut schema=output_schema("get_interception")?;schema["properties"]["appliedVersion"]=string();schema["required"].as_array_mut().unwrap().push(json!("appliedVersion"));schema},
        "list_pending_interceptions"=>object(json!({"items":array(object(json!({"id":string(),"stage":string(),"requestId":string(),"source":string(),"method":string(),"url":string(),"status":{"type":["integer","null"]},"startedAt":integer()}), &["id","stage","requestId","source","method","url","status","startedAt"])),"total":integer(),"nextOffset":next_offset()}), &["items","total","nextOffset"]),
        "resolve_interception"=>object(json!({"id":string(),"action":{"enum":["continue","abort","replace"]},"resolved":{"const":true}}), &["id","action","resolved"]),
        "get_global_scripts"=>object(json!({"scripts":scripts(),"expectedVersion":string(),"scope":{"const":"global-capture"},"warning":string()}), &["scripts","expectedVersion","scope","warning"]),
        "get_script_framework"=>object(json!({"scope":{"const":"global-capture"},"language":string(),"lifecycle":array(string()),"matching":string(),"limits":{"type":"object","properties":{"hookBytes":integer(),"variablesBytes":integer(),"executionSecondsPerStage":integer(),"runtimeMemoryBytes":integer()}},"objects":{"type":"object"},"functions":{"type":"object"},"constraints":array(string())}), &["scope","language","lifecycle","matching","limits","objects","functions","constraints"]),
        "get_script_template"=>object(json!({"template":{"enum":["request_header","replace_json_response","signature_hmac"]},"scope":{"const":"global-capture"},"scripts":scripts(),"instructions":string()}), &["template","scope","scripts","instructions"]),
        "set_global_scripts"=>object(json!({"saved":{"const":true},"enabled":{"const":false},"expectedVersion":string(),"nextStep":string()}), &["saved","enabled","expectedVersion","nextStep"]),
        "set_global_scripts_enabled"=>object(json!({"enabled":{"type":"boolean"},"expectedVersion":string(),"scope":{"const":"global-capture"}}), &["enabled","expectedVersion","scope"]),
        _=>return None,
    })
}

pub fn decorate(mut tools: Vec<Tool>) -> Vec<Tool> {
    for tool in &mut tools {
        let name=tool.name.as_ref();
        let (read_only,destructive,idempotent,open_world)=match name {
            "send_request"=>(false,true,false,true),
            "cancel_request"|"stop_capture"=>(false,true,true,true),
            "start_capture"=>(false,false,false,true),
            "resolve_interception"|"configure_interception"|"set_global_scripts_enabled"=>(false,true,false,true),
            "create_collection"|"create_request"=>(false,false,false,false),
            "update_request"|"save_request_to_collection"|"set_global_scripts"=>(false,true,false,false),
            "capture_status"|"list_requests"|"get_request"|"read_body"|"list_collections"|"get_saved_request"|"get_workspaces"|"get_request_editor"|"get_interception"|"list_pending_interceptions"|"read_pending_body"|"get_global_scripts"|"get_script_framework"|"get_script_template"=>(true,false,true,false),
            _=>(false,true,false,true),
        };
        tool.annotations=Some(ToolAnnotations::from_raw(None,Some(read_only),Some(destructive),Some(idempotent),Some(open_world)));
        if let Some(schema)=output_schema(name) {tool.output_schema=Some(Arc::new(schema.as_object().unwrap().clone()));}
    }
    tools
}

#[cfg(test)]
pub(crate) fn assert_matches(name: &str, value: &Value) {
    fn matches(schema:&Value,value:&Value)->bool {
        if let Some(options)=schema.get("anyOf").and_then(Value::as_array) {if !options.iter().any(|s|matches(s,value)){return false;}}
        if let Some(options)=schema.get("oneOf").and_then(Value::as_array) {if options.iter().filter(|s|matches(s,value)).count()!=1{return false;}}
        if let Some(expected)=schema.get("const") {if expected!=value{return false;}}
        if let Some(options)=schema.get("enum").and_then(Value::as_array) {if !options.contains(value){return false;}}
        let is_type=|kind:&str|match kind {"object"=>value.is_object(),"array"=>value.is_array(),"string"=>value.is_string(),"integer"=>value.is_i64()||value.is_u64(),"number"=>value.is_number(),"boolean"=>value.is_boolean(),"null"=>value.is_null(),_=>false};
        if let Some(kind)=schema.get("type") {
            if let Some(kind)=kind.as_str(){if !is_type(kind){return false;}}
            else if let Some(kinds)=kind.as_array(){if !kinds.iter().any(|kind|kind.as_str().is_some_and(is_type)){return false;}}
        }
        if let Some(required)=schema.get("required").and_then(Value::as_array){if !required.iter().all(|key|key.as_str().is_some_and(|key|value.get(key).is_some())){return false;}}
        if let (Some(properties),Some(object))=(schema.get("properties").and_then(Value::as_object),value.as_object()) {for (key,entry) in object {if let Some(property)=properties.get(key){if !matches(property,entry){return false;}}}}
        if let (Some(items),Some(values))=(schema.get("items"),value.as_array()){if !values.iter().all(|value|matches(items,value)){return false;}}
        true
    }
    let schema=output_schema(name).unwrap();
    assert!(matches(&schema,value),"{name} result does not match its declared output schema:\nresult={value}\nschema={schema}");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_permission_scoped_tools_have_explicit_effects_and_output_contracts(){
        let config=crate::server::ServerConfig{allow_send:true,allow_capture_control:true,allow_workspace_write:true,allow_rules_write:true,allow_scripts_write:true,..Default::default()};
        let tools=crate::server::tools(&config);assert_eq!(tools.len(),26);
        for tool in tools {
            let hints=tool.annotations.as_ref().expect("explicit annotations");
            let write=capture_core::audit::AuditOperation::from_tool_name(tool.name.as_ref()).is_some();
            assert_eq!(hints.read_only_hint,Some(!write),"{}",tool.name);
            assert!(hints.destructive_hint.is_some()&&hints.idempotent_hint.is_some()&&hints.open_world_hint.is_some());
            let schema=tool.output_schema.as_ref().expect("output schema");
            assert_eq!(schema["type"],"object");assert!(!schema["required"].as_array().unwrap().is_empty());
        }
        let defaults=crate::server::tools(&Default::default());assert_eq!(defaults.len(),4);
        assert!(defaults.iter().all(|tool|tool.annotations.as_ref().unwrap().read_only_hint==Some(true)));
        let unknown=decorate(vec![Tool::new("future_unknown","unknown",json!({"type":"object"}).as_object().unwrap().clone())]);
        assert_eq!(unknown[0].annotations.as_ref().unwrap().read_only_hint,Some(false),"unknown effects must not be optimistically read-only");
    }
    #[tokio::test]
    async fn workflow_and_script_contracts_match_existing_saved_data_and_actual_mutation_results(){
        let dir=tempfile::tempdir().unwrap();let runtime=capture_core::CaptureRuntime::new(capture_core::Engine::open(dir.path()).unwrap());
        let draft: capture_core::model::RequestDraft=serde_json::from_value(json!({"method":"POST","url":"https://example.invalid/","headers":[{"name":"X-Repeated","value":"a"},{"name":"X-Repeated","value":"b","disabled":true}],"bodyBase64":"AAH+/w=="})).unwrap();
        runtime.engine.store.save_setting("workspace.v1",&json!({"revision":5,"activeEditor":"legacy-http","editors":[{"id":"legacy-http","savedRequestId":"saved","draft":draft,"selected":null,"parentId":null,"futureUi":{"keep":true}},{"id":"legacy-grpc","draft":draft,"grpcDraft":{"method":"example.Service/Call","json":"{}"},"selected":null,"parentId":null}]})).unwrap();
        runtime.engine.store.save_setting("library.v1",&json!({"revision":3,"collections":[{"id":"collection","name":"Existing"}],"requests":[{"id":"saved","name":"Saved","collectionId":"collection","draft":draft,"requestMode":"grpc"}],"scripts":[],"environments":[]})).unwrap();
        async fn workflow(runtime:&std::sync::Arc<capture_core::CaptureRuntime>,name:&str,args:Value)->Value {let value=crate::workflows::execute(runtime.clone(),name,args,true,false).await.unwrap();assert_matches(name,&value);value}
        let workspaces=workflow(&runtime,"get_workspaces",json!({})).await;assert_eq!(workspaces["editors"][0]["mode"],"http");assert_eq!(workspaces["editors"][1]["mode"],"grpc");
        workflow(&runtime,"get_request_editor",json!({"editorId":"legacy-http","includeBody":true,"bodyOffset":1,"bodyLength":2})).await;
        workflow(&runtime,"get_saved_request",json!({"id":"saved","includeBody":true})).await;
        workflow(&runtime,"list_collections",json!({})).await;workflow(&runtime,"list_collections",json!({"collectionId":"collection"})).await;
        let collection=workflow(&runtime,"create_collection",json!({"name":"New","expectedLibraryRevision":3})).await;
        let created=workflow(&runtime,"create_request",json!({"method":"GET","url":"https://example.invalid/new","expectedRevision":5})).await;
        let editor=created["result"]["editorId"].clone();
        workflow(&runtime,"update_request",json!({"editorId":editor,"method":"PATCH","expectedRevision":6})).await;
        workflow(&runtime,"save_request_to_collection",json!({"editorId":editor,"collectionId":collection["result"]["collectionId"],"name":"New saved","expectedRevision":7,"expectedLibraryRevision":4})).await;
        for (name,args) in [("get_interception",json!({})),("list_pending_interceptions",json!({})),("get_global_scripts",json!({})),("get_script_framework",json!({})),("get_script_template",json!({"template":"replace_json_response","urlContains":"example.invalid"}))] {
            let value=crate::rules::execute(runtime.clone(),name,args,true,true,false).await.unwrap();assert_matches(name,&value);
        }
        let (_,version)=runtime.engine.scripts.snapshot_versioned();
        let saved=crate::rules::execute(runtime.clone(),"set_global_scripts",json!({"expectedVersion":version,"before":"","after":""}),true,true,false).await.unwrap();assert_matches("set_global_scripts",&saved);
        let enabled=crate::rules::execute(runtime.clone(),"set_global_scripts_enabled",json!({"expectedVersion":saved["expectedVersion"],"enabled":false}),true,true,false).await.unwrap();assert_matches("set_global_scripts_enabled",&enabled);
        let (workspace,_)=runtime.engine.store.mcp_workspace_snapshot().unwrap();assert_eq!(workspace["editors"][0]["futureUi"]["keep"],true);
    }
}
