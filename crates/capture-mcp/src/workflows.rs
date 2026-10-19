//! Request composition and collection management. These tools never send a
//! request, evaluate scripts, expand environments, or modify transport settings.
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use capture_core::{CaptureRuntime, model::RequestDraft};
use rmcp::model::Tool;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

const MAX_INPUT:usize=256*1024;
const MAX_BODY:usize=65536;
fn tool(name:&'static str,description:&'static str,properties:Value,required:&[&str])->Tool {
    Tool::new(name,description,json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}).as_object().unwrap().clone())
}
fn headers_schema()->Value {json!({"type":"array","maxItems":200,"items":{"type":"object","properties":{"name":{"type":"string","maxLength":256},"value":{"type":"string","maxLength":8192},"disabled":{"type":"boolean"}},"required":["name","value"],"additionalProperties":false}})}
pub fn tools(allow_write:bool)->Vec<Tool> {
    if !allow_write {return Vec::new();}
    let id=json!({"type":"string","minLength":1,"maxLength":128});
    let revision=json!({"type":"integer","minimum":0});
    let body_read=json!({"type":"boolean","default":false});
    vec![
        tool("list_collections","List collection names and request summaries, without drafts, scripts or environment values. Returns the library revision required by writes.",json!({"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"maximum":100,"default":50},"collectionId":id}),&[]),
        tool("get_saved_request","Read a saved HTTP request without scripts, TLS, proxy settings or environment expansion. Sensitive headers and URL parameters are redacted by default. Body bytes require includeBody=true and may contain private data.",json!({"id":id,"includeBody":body_read,"bodyOffset":{"type":"integer","minimum":0,"default":0},"bodyLength":{"type":"integer","minimum":1,"maximum":65536,"default":16384}}),&["id"]),
        tool("get_workspaces","Read request tab summaries and workspace/library revisions. Does not return headers, bodies, scripts or environment variables.",json!({}),&[]),
        tool("get_request_editor","Read one request tab with the current expectedRevision. Does not return scripts or transport credentials. Body bytes require includeBody=true and may contain private data.",json!({"editorId":id,"includeBody":body_read,"bodyOffset":{"type":"integer","minimum":0,"default":0},"bodyLength":{"type":"integer","minimum":1,"maximum":65536,"default":16384}}),&["editorId"]),
        tool("create_collection","Create an empty named request collection. Requires the current library revision; a conflict makes no changes.",json!({"name":{"type":"string","minLength":1,"maxLength":512},"expectedLibraryRevision":revision}),&["name","expectedLibraryRevision"]),
        tool("create_request","Create and persist an HTTP request tab without sending it. Provide flowId to copy an exact captured HTTP request (body at most 65536 bytes), OR method/url/headers/bodyBase64. Captured scripts and transport credentials are never copied. Requires current workspace revision.",json!({"expectedRevision":revision,"flowId":id,"method":{"type":"string","maxLength":32},"url":{"type":"string","maxLength":8192},"headers":headers_schema(),"bodyBase64":{"type":"string","maxLength":87384}}),&["expectedRevision"]),
        tool("update_request","Edit ordinary HTTP fields in an existing tab without sending it. Header arrays replace all headers and retain their order, duplicates and disabled flags. Existing scripts and other UI fields are preserved. Requires current workspace revision.",json!({"editorId":id,"expectedRevision":revision,"method":{"type":"string","maxLength":32},"url":{"type":"string","maxLength":8192},"headers":headers_schema(),"bodyBase64":{"type":"string","maxLength":87384}}),&["editorId","expectedRevision"]),
        tool("save_request_to_collection","Save a request tab into a collection and associate its saved request ID atomically. By default updates that tab's associated saved request; saveAsNew=true creates a separate saved request. Requires BOTH current workspace and library revisions. Never sends the request or executes scripts.",json!({"editorId":id,"collectionId":id,"name":{"type":"string","minLength":1,"maxLength":512},"expectedRevision":revision,"expectedLibraryRevision":revision,"saveAsNew":{"type":"boolean","default":false}}),&["editorId","collectionId","name","expectedRevision","expectedLibraryRevision"]),
    ]
}
#[derive(Deserialize)]#[serde(deny_unknown_fields)]struct Empty{}
#[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]struct List {#[serde(default)]offset:usize,#[serde(default="default_limit")]limit:usize,collection_id:Option<String>}
fn default_limit()->usize{50}
#[derive(Deserialize,Default)]#[serde(rename_all="camelCase",deny_unknown_fields)]struct BodyRead {#[serde(default)]include_body:bool,#[serde(default)]body_offset:u64,#[serde(default="default_body_length")]body_length:usize}
fn default_body_length()->usize{16384}
#[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]struct Detail {id:String,#[serde(default)]include_body:bool,#[serde(default)]body_offset:u64,#[serde(default="default_body_length")]body_length:usize}
#[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]struct EditorDetail {editor_id:String,#[serde(default)]include_body:bool,#[serde(default)]body_offset:u64,#[serde(default="default_body_length")]body_length:usize}
#[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]struct NewCollection {name:String,expected_library_revision:u64}
#[derive(Deserialize)]#[serde(deny_unknown_fields)]struct Header {name:String,value:String,#[serde(default)]disabled:bool}
#[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]struct Create {expected_revision:u64,flow_id:Option<String>,method:Option<String>,url:Option<String>,headers:Option<Vec<Header>>,body_base64:Option<String>}
#[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]struct Update {editor_id:String,expected_revision:u64,method:Option<String>,url:Option<String>,headers:Option<Vec<Header>>,body_base64:Option<String>}
#[derive(Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]struct Save {editor_id:String,collection_id:String,name:String,expected_revision:u64,expected_library_revision:u64,#[serde(default)]save_as_new:bool}
fn parse<T:serde::de::DeserializeOwned>(args:Value)->Result<T>{serde_json::from_value(args).context("Invalid tool arguments")}
fn valid_id(value:&str)->Result<()> {ensure!(!value.is_empty()&&value.len()<=128,"Invalid ID");Ok(())}
fn valid_name(value:&str)->Result<()> {ensure!(!value.trim().is_empty()&&value.len()<=512,"Name must be between 1 and 512 bytes");Ok(())}
fn token(b:u8)->bool {b.is_ascii_alphanumeric()||b"!#$%&'*+-.^_`|~".contains(&b)}
fn validate_draft(value:&Value)->Result<()> {
    let method=value["method"].as_str().context("HTTP method is required")?;
    ensure!(!method.is_empty()&&method.len()<=32&&method.bytes().all(token)&&!method.eq_ignore_ascii_case("CONNECT"),"Only ordinary HTTP methods are supported");
    let raw=value["url"].as_str().context("HTTP URL is required")?;
    ensure!(raw.len()<=8192,"URL exceeds 8192 bytes");let url=url::Url::parse(raw).context("Invalid HTTP URL")?;
    ensure!(matches!(url.scheme(),"http"|"https")&&url.host_str().is_some()&&url.username().is_empty()&&url.password().is_none(),"Use an HTTP URL without embedded credentials");
    let headers=value["headers"].as_array().context("Invalid headers")?;ensure!(headers.len()<=200,"At most 200 headers are supported");
    for header in headers {
        let name=header["name"].as_str().context("Invalid header name")?;let value=header["value"].as_str().context("Invalid header value")?;
        ensure!(!name.is_empty()&&name.len()<=256&&name.bytes().all(token)&&value.len()<=8192&&!value.contains(['\r','\n','\0']),"Invalid HTTP header or value");
        ensure!(!name.eq_ignore_ascii_case("proxy-authorization")&&!name.eq_ignore_ascii_case("upgrade")&&!(name.eq_ignore_ascii_case("content-type")&&value.to_ascii_lowercase().starts_with("application/grpc")),"Proxy credentials and WebSocket/gRPC requests are not supported by these HTTP tools");
    }
    let body=value["bodyBase64"].as_str().context("Invalid bodyBase64")?;
    ensure!(body.len()<=MAX_BODY.div_ceil(3)*4,"Body exceeds 65536 decoded bytes");
    ensure!(STANDARD.decode(body).context("bodyBase64 must be standard Base64")?.len()<=MAX_BODY,"Body exceeds 65536 decoded bytes");
    Ok(())
}
fn header_values(headers:Vec<Header>)->Value {Value::Array(headers.into_iter().map(|h|json!({"name":h.name,"value":h.value,"disabled":h.disabled})).collect())}
fn list<'a>(root:&'a Value,key:&str)->Result<&'a Vec<Value>> {root[key].as_array().with_context(||format!("Invalid saved {key}"))}
fn list_mut<'a>(root:&'a mut Value,key:&str)->Result<&'a mut Vec<Value>> {root.get_mut(key).and_then(Value::as_array_mut).with_context(||format!("Invalid saved {key}"))}
fn ordinary(editor:&Value)->Result<()> {ensure!(editor.get("grpcDraft").is_none_or(Value::is_null),"Use the application's gRPC editor for this request");Ok(())}
fn response(value:Value,workspace_revision:u64,library_revision:u64)->Value {json!({"result":value,"expectedRevision":workspace_revision,"expectedLibraryRevision":library_revision})}

pub async fn execute(runtime:Arc<CaptureRuntime>,name:&str,args:Value,allow_write:bool,include_sensitive:bool)->Result<Value> {
    ensure!(allow_write,"Request and collection management permission is disabled");
    ensure!(serde_json::to_vec(&args)?.len()<=MAX_INPUT,"Tool arguments exceed 256 KiB");
    let store=runtime.engine.store.clone();let name=name.to_owned();
    tokio::task::spawn_blocking(move||->Result<Value>{match name.as_str() {
        "get_workspaces"=>{
            let _:Empty=parse(args)?;let (workspace,library)=store.mcp_workspace_snapshot()?;
            let editors=list(&workspace,"editors")?.iter().take(100).map(|e|json!({"editorId":e["id"],"method":bounded(e["draft"]["method"].as_str().unwrap_or(""),32),"url":bounded(&redact_url(e["draft"]["url"].as_str().unwrap_or(""),include_sensitive),2048),"savedRequestId":e["savedRequestId"],"mode":if e.get("grpcDraft").is_some_and(|v|!v.is_null()){"grpc"}else{"http"}})).collect::<Vec<_>>();
            Ok(json!({"expectedRevision":workspace["revision"],"expectedLibraryRevision":library["revision"],"activeEditor":workspace["activeEditor"],"editors":editors}))
        }
        "list_collections"=>{
            let p:List=parse(args)?;ensure!((1..=100).contains(&p.limit),"limit must be between 1 and 100");
            if let Some(id)=&p.collection_id {valid_id(id)?;}
            let (_,library)=store.mcp_workspace_snapshot()?;let requests=list(&library,"requests")?;
            if let Some(id)=p.collection_id {
                let collection=list(&library,"collections")?.iter().find(|c|c["id"]==id).context("Collection not found")?;
                let matching=requests.iter().filter(|r|r["collectionId"]==id).collect::<Vec<_>>();
                let values=matching.iter().skip(p.offset).take(p.limit).map(|r|json!({"id":r["id"],"name":bounded(r["name"].as_str().unwrap_or(""),512),"method":bounded(r["draft"]["method"].as_str().unwrap_or(""),32),"url":bounded(&redact_url(r["draft"]["url"].as_str().unwrap_or(""),include_sensitive),2048),"mode":r["requestMode"]})).collect::<Vec<_>>();
                Ok(json!({"expectedLibraryRevision":library["revision"],"collection":{"id":collection["id"],"name":bounded(collection["name"].as_str().unwrap_or(""),512)},"requests":values,"nextOffset":if p.offset.saturating_add(p.limit)<matching.len(){Some(p.offset+p.limit)}else{None}}))
            } else {
                let collections=list(&library,"collections")?;
                let values=collections.iter().skip(p.offset).take(p.limit).map(|c|json!({"id":c["id"],"name":bounded(c["name"].as_str().unwrap_or(""),512),"requestCount":requests.iter().filter(|r|r["collectionId"]==c["id"]).count()})).collect::<Vec<_>>();
                Ok(json!({"expectedLibraryRevision":library["revision"],"collections":values,"nextOffset":if p.offset.saturating_add(p.limit)<collections.len(){Some(p.offset+p.limit)}else{None}}))
            }
        }
        "get_saved_request"=>{
            let p:Detail=parse(args)?;valid_id(&p.id)?;let (_,library)=store.mcp_workspace_snapshot()?;
            let request=list(&library,"requests")?.iter().find(|r|r["id"]==p.id).context("Saved request not found")?;
            Ok(json!({"expectedLibraryRevision":library["revision"],"id":request["id"],"name":bounded(request["name"].as_str().unwrap_or(""),512),"collectionId":request["collectionId"],"request":project(&request["draft"],BodyRead{include_body:p.include_body,body_offset:p.body_offset,body_length:p.body_length},include_sensitive)?}))
        }
        "get_request_editor"=>{
            let p:EditorDetail=parse(args)?;valid_id(&p.editor_id)?;let (workspace,library)=store.mcp_workspace_snapshot()?;
            let editor=list(&workspace,"editors")?.iter().find(|e|e["id"]==p.editor_id).context("Request editor not found")?;ordinary(editor)?;
            Ok(json!({"expectedRevision":workspace["revision"],"expectedLibraryRevision":library["revision"],"editorId":editor["id"],"savedRequestId":editor["savedRequestId"],"request":project(&editor["draft"],BodyRead{include_body:p.include_body,body_offset:p.body_offset,body_length:p.body_length},include_sensitive)?}))
        }
        "create_collection"=>{
            let p:NewCollection=parse(args)?;valid_name(&p.name)?;let id=uuid::Uuid::new_v4().to_string();
            let (value,w,l)=store.mcp_change_workspace(None,Some(p.expected_library_revision),|_,library|{list_mut(library,"collections")?.push(json!({"id":id,"name":p.name}));Ok(json!({"collectionId":id,"name":p.name}))})?;
            Ok(response(value,w,l))
        }
        "create_request"=>{
            let p:Create=parse(args)?;
            let (fields,parent)=if let Some(id)=p.flow_id {
                ensure!(p.method.is_none()&&p.url.is_none()&&p.headers.is_none()&&p.body_base64.is_none(),"flowId cannot be combined with inline request fields");
                valid_id(&id)?;(store.mcp_compose_request(&id)?,Some(id))
            } else {(json!({"method":p.method.context("method is required without flowId")?,"url":p.url.context("url is required without flowId")?,"headers":header_values(p.headers.unwrap_or_default()),"bodyBase64":p.body_base64.unwrap_or_default()}),None)};
            validate_draft(&fields)?;
            // Deserialization adds only the application's safe default transport
            // and disabled/empty scripts; capture-specific state is never copied.
            let draft:RequestDraft=serde_json::from_value(fields)?;let draft=serde_json::to_value(draft)?;
            let id=uuid::Uuid::new_v4().to_string();
            let (value,w,l)=store.mcp_change_workspace(Some(p.expected_revision),None,|workspace,_|{
                ensure!(list(workspace,"editors")?.len()<100,"At most 100 request tabs can be saved");
                list_mut(workspace,"editors")?.push(json!({"id":id,"draft":draft,"selected":null,"parentId":parent}));workspace["activeEditor"]=json!(id);
                Ok(json!({"editorId":id,"created":true,"sent":false}))
            })?;Ok(response(value,w,l))
        }
        "update_request"=>{
            let p:Update=parse(args)?;valid_id(&p.editor_id)?;
            ensure!(p.method.is_some()||p.url.is_some()||p.headers.is_some()||p.body_base64.is_some(),"Provide at least one request field to update");
            let (value,w,l)=store.mcp_change_workspace(Some(p.expected_revision),None,|workspace,_|{
                let editor=list_mut(workspace,"editors")?.iter_mut().find(|e|e["id"]==p.editor_id).context("Request editor not found")?;ordinary(editor)?;
                let draft=editor.get_mut("draft").context("Invalid request draft")?;
                if let Some(method)=p.method {draft["method"]=json!(method);}
                if let Some(url)=p.url {draft["url"]=json!(url);}
                if let Some(headers)=p.headers {draft["headers"]=header_values(headers);}
                if let Some(body)=p.body_base64 {draft["bodyBase64"]=json!(body);}
                validate_draft(draft)?;
                Ok(json!({"editorId":p.editor_id,"updated":true,"sent":false}))
            })?;Ok(response(value,w,l))
        }
        "save_request_to_collection"=>{
            let p:Save=parse(args)?;valid_id(&p.editor_id)?;valid_id(&p.collection_id)?;valid_name(&p.name)?;
            let (value,w,l)=store.mcp_change_workspace(Some(p.expected_revision),Some(p.expected_library_revision),|workspace,library|{
                ensure!(list(library,"collections")?.iter().any(|c|c["id"]==p.collection_id),"Collection not found");
                let editor=list_mut(workspace,"editors")?.iter_mut().find(|e|e["id"]==p.editor_id).context("Request editor not found")?;ordinary(editor)?;
                // Saving an existing UI draft copies it verbatim. MCP cannot
                // supply scripts or any other hidden UI fields in this call.
                let draft=editor["draft"].clone();
                let requests=list_mut(library,"requests")?;
                let associated=if p.save_as_new {None}else{editor["savedRequestId"].as_str().and_then(|id|requests.iter().position(|r|r["id"]==id))};
                let id=if let Some(index)=associated {
                    let request=&mut requests[index];request["name"]=json!(p.name);request["collectionId"]=json!(p.collection_id);request["draft"]=draft;request["id"].as_str().context("Invalid saved request ID")?.to_owned()
                } else {
                    let id=uuid::Uuid::new_v4().to_string();requests.push(json!({"id":id,"name":p.name,"collectionId":p.collection_id,"draft":draft,"requestMode":"http"}));id
                };
                editor["savedRequestId"]=json!(id);Ok(json!({"editorId":p.editor_id,"savedRequestId":id,"collectionId":p.collection_id,"created":associated.is_none()}))
            })?;Ok(response(value,w,l))
        }
        _=>anyhow::bail!("Unknown request workflow tool"),
    }}).await?
}
fn bounded(value:&str,count:usize)->String {value.chars().take(count).collect()}
fn sensitive(name:&str)->bool {let name=name.to_ascii_lowercase().replace(['-','_','.'],"");name=="key"||["auth","cookie","token","secret","password","passwd","apikey","accesskey","session","signature","credential","csrf","xsrf"].iter().any(|part|name.contains(part))}
fn redact_url(value:&str,include_sensitive:bool)->String {
    let Ok(mut url)=url::Url::parse(value) else{return "[invalid URL]".into();};
    if include_sensitive {return bounded(value,8192);}
    let _=url.set_username("");let _=url.set_password(None);url.set_fragment(None);
    let pairs=url.query_pairs().map(|(name,value)|(name.to_string(),if sensitive(&name){"[REDACTED]".into()}else{value.into_owned()})).collect::<Vec<_>>();
    if url.query().is_some() {url.query_pairs_mut().clear().extend_pairs(pairs);}
    bounded(url.as_str(),8192)
}
fn project(draft:&Value,body:BodyRead,include_sensitive:bool)->Result<Value> {
    ensure!((1..=MAX_BODY).contains(&body.body_length),"bodyLength must be between 1 and 65536 bytes");
    let headers=draft["headers"].as_array().context("Invalid saved request headers")?;
    let values=headers.iter().take(80).map(|header|{let name=header["name"].as_str().unwrap_or("");json!({"name":bounded(name,256),"value":if !include_sensitive&&sensitive(name){"[REDACTED]".into()}else{bounded(header["value"].as_str().unwrap_or(""),512)},"disabled":header["disabled"].as_bool().unwrap_or(false)})}).collect::<Vec<_>>();
    let mut value=json!({"method":bounded(draft["method"].as_str().unwrap_or(""),32),"url":redact_url(draft["url"].as_str().unwrap_or(""),include_sensitive),"headers":values,"headersTruncated":headers.len()>80,"hasScripts":draft.get("scripts").is_some_and(|s|s["enabled"].as_bool()==Some(true)||s["before"].as_str().is_some_and(|v|!v.is_empty())||s["after"].as_str().is_some_and(|v|!v.is_empty()))});
    let encoded=draft["bodyBase64"].as_str().unwrap_or("");
    let padding=if encoded.ends_with("=="){2}else if encoded.ends_with('='){1}else{0};
    let total=encoded.len().checked_div(4).unwrap_or(0).saturating_mul(3).saturating_sub(padding);
    value["bodyBytes"]=json!(total);
    if body.include_body {
        let offset=usize::try_from(body.body_offset).context("Invalid bodyOffset")?.min(total);
        let length=body.body_length.min(total-offset);let aligned=offset/3*4;let skip=offset%3;
        let end=(offset+length).div_ceil(3)*4;
        let data=STANDARD.decode(encoded.get(aligned..end).context("Invalid saved Base64 body")?).context("Invalid saved Base64 body")?;
        let bytes=data.get(skip..skip+length).context("Invalid saved Base64 body")?;
        value["body"]=json!({"offset":offset,"totalBytes":total,"bodyBase64":STANDARD.encode(bytes),"nextOffset":if offset+length<total{Some(offset+length)}else{None},"mayContainPrivateData":true});
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use capture_core::{Engine,model::Flow};
    fn runtime(path:&std::path::Path)->Arc<CaptureRuntime> {CaptureRuntime::new(Engine::open(path).unwrap())}
    async fn call(runtime:&Arc<CaptureRuntime>,name:&str,args:Value)->Value {execute(runtime.clone(),name,args,true,false).await.unwrap()}
    fn captured()->Flow {serde_json::from_value(json!({"id":"captured","parentId":null,"startedAt":1,"durationMs":1,"source":"capture","state":"completed","request":{"method":"POST","url":"https://example.invalid/path?token=private-query&x=1","headers":[{"name":"X-A","value":"first"},{"name":"Cookie","value":"private-cookie"},{"name":"X-A","value":"third","disabled":true}],"bodyBase64":"AAH+/w==","scripts":{"enabled":true,"before":"throw new Error('never run me');","variables":{"password":"private-variable"}},"upstreamProfileId":"private-proxy"},"response":null,"error":null,"notes":[]})).unwrap()}
    #[tokio::test]
    async fn captured_compose_update_and_save_are_persistent_exact_and_never_mutate_capture() {
        let dir=tempfile::tempdir().unwrap();let app=runtime(dir.path());let flow=captured();app.engine.store.insert(&flow).unwrap();
        let collection=call(&app,"create_collection",json!({"name":"Flights","expectedLibraryRevision":0})).await;
        let collection_id=collection["result"]["collectionId"].as_str().unwrap().to_owned();
        let created=call(&app,"create_request",json!({"flowId":"captured","expectedRevision":0})).await;
        let editor_id=created["result"]["editorId"].as_str().unwrap().to_owned();
        let workspace=app.engine.store.workspace().unwrap();let draft=&workspace.editors[0].draft;
        assert!(!draft.scripts.enabled);assert!(draft.scripts.before.is_empty());assert!(draft.scripts.variables.is_empty());assert!(draft.upstream_profile_id.is_none());
        assert_eq!(draft.body_base64.as_str(),"AAH+/w==");assert_eq!(draft.headers[0].value,"first");assert_eq!(draft.headers[2].value,"third");assert!(draft.headers[2].disabled);
        let read=call(&app,"get_request_editor",json!({"editorId":editor_id})).await;
        let text=read.to_string();assert!(!text.contains("private-cookie"));assert!(!text.contains("private-query"));assert!(!text.contains("private-variable"));assert!(read["request"].get("body").is_none());
        let read=call(&app,"get_request_editor",json!({"editorId":editor_id,"includeBody":true,"bodyOffset":1,"bodyLength":2})).await;
        assert_eq!(read["request"]["body"]["bodyBase64"],"Af4=");
        call(&app,"update_request",json!({"editorId":editor_id,"expectedRevision":1,"url":"https://example.invalid/new"})).await;
        let saved=call(&app,"save_request_to_collection",json!({"editorId":editor_id,"collectionId":collection_id,"name":"Tomorrow","expectedRevision":2,"expectedLibraryRevision":1})).await;
        let saved_id=saved["result"]["savedRequestId"].as_str().unwrap().to_owned();
        assert_eq!(saved["expectedRevision"],3);assert_eq!(saved["expectedLibraryRevision"],2);
        assert_eq!(serde_json::to_value(app.engine.store.get("captured").unwrap().unwrap()).unwrap(),serde_json::to_value(flow).unwrap());
        assert_eq!(app.engine.store.list().unwrap().len(),1); // No send/replay occurred.
        drop(app);let app=runtime(dir.path());let workspace=app.engine.store.workspace().unwrap();let library=app.engine.store.library().unwrap();
        assert_eq!(workspace.editors[0].saved_request_id.as_deref(),Some(saved_id.as_str()));assert_eq!(library.requests[0].id,saved_id);assert_eq!(library.requests[0].draft.url,"https://example.invalid/new");assert_eq!(library.requests[0].draft.body_base64.as_str(),"AAH+/w==");
        let detail=call(&app,"get_saved_request",json!({"id":saved_id,"includeBody":true})).await;assert_eq!(detail["request"]["body"]["bodyBase64"],"AAH+/w==");
    }
    #[tokio::test]
    async fn revisions_and_errors_never_partially_save_or_overwrite_other_windows() {
        let dir=tempfile::tempdir().unwrap();let app=runtime(dir.path());
        let c=call(&app,"create_collection",json!({"name":"One","expectedLibraryRevision":0})).await;
        let e=call(&app,"create_request",json!({"expectedRevision":0,"method":"GET","url":"https://example.invalid/"})).await;
        let id=e["result"]["editorId"].as_str().unwrap();
        call(&app,"create_collection",json!({"name":"Other window","expectedLibraryRevision":1})).await;
        let before=app.engine.store.mcp_workspace_snapshot().unwrap();
        let failed=execute(app.clone(),"save_request_to_collection",json!({"editorId":id,"collectionId":c["result"]["collectionId"],"name":"Should not save","expectedRevision":1,"expectedLibraryRevision":1}),true,false).await.unwrap_err();
        assert!(failed.to_string().contains("revision conflict"));assert_eq!(app.engine.store.mcp_workspace_snapshot().unwrap(),before);
        assert!(execute(app.clone(),"update_request",json!({"editorId":id,"expectedRevision":0,"method":"POST"}),true,false).await.is_err());
        assert_eq!(app.engine.store.mcp_workspace_snapshot().unwrap(),before);
        assert!(execute(app.clone(),"update_request",json!({"editorId":id,"expectedRevision":1,"headers":[{"name":"Bad","value":"x\r\nInjected: y"}]}),true,false).await.is_err());
        assert_eq!(app.engine.store.mcp_workspace_snapshot().unwrap(),before);
    }
    #[tokio::test]
    async fn user_scripts_unknown_ui_fields_and_saved_identity_survive_update_and_save_as() {
        let dir=tempfile::tempdir().unwrap();let app=runtime(dir.path());
        let mut draft=serde_json::to_value(captured().request).unwrap();draft["futureUi"]=json!({"doNotErase":true});
        let scripts=draft["scripts"].clone();
        app.engine.store.save_setting("workspace.v1",&json!({"revision":4,"futureWorkspace":"preserve","activeEditor":"user","editors":[{"id":"user","draft":draft,"parentId":null,"selected":null,"savedRequestId":"saved","futureEditor":"preserve"}]})).unwrap();
        app.engine.store.save_setting("library.v1",&json!({"revision":7,"collections":[{"id":"collection","name":"Saved"}],"requests":[{"id":"saved","name":"Old","collectionId":"collection","draft":draft,"futureSaved":"preserve"}],"environments":[{"id":"env","name":"Private","variables":{"password":"never-return"}}],"scripts":[]})).unwrap();
        call(&app,"update_request",json!({"editorId":"user","expectedRevision":4,"method":"PUT"})).await;
        let saved=call(&app,"save_request_to_collection",json!({"editorId":"user","collectionId":"collection","name":"Renamed","expectedRevision":5,"expectedLibraryRevision":7})).await;
        assert_eq!(saved["result"]["savedRequestId"],"saved");assert_eq!(saved["result"]["created"],false);
        let (workspace,library)=app.engine.store.mcp_workspace_snapshot().unwrap();assert_eq!(workspace["futureWorkspace"],"preserve");assert_eq!(workspace["editors"][0]["futureEditor"],"preserve");assert_eq!(workspace["editors"][0]["draft"]["scripts"],scripts);assert_eq!(library["requests"][0]["futureSaved"],"preserve");assert_eq!(library["requests"][0]["draft"]["futureUi"]["doNotErase"],true);
        call(&app,"save_request_to_collection",json!({"editorId":"user","collectionId":"collection","name":"Copy","expectedRevision":6,"expectedLibraryRevision":8,"saveAsNew":true})).await;
        let (_,library)=app.engine.store.mcp_workspace_snapshot().unwrap();assert_eq!(library["requests"].as_array().unwrap().len(),2);assert_eq!(library["requests"][0]["name"],"Renamed");
        let data=call(&app,"get_request_editor",json!({"editorId":"user"})).await.to_string();assert!(!data.contains("never-return"));assert!(!data.contains("never run me"));assert!(!data.contains("private-variable"));assert!(!data.contains("private-proxy"));
    }
    #[tokio::test]
    async fn permission_unknown_fields_bounds_and_tab_limit_are_enforced() {
        let dir=tempfile::tempdir().unwrap();let app=runtime(dir.path());assert!(tools(false).is_empty());assert_eq!(tools(true).len(),8);
        assert!(execute(app.clone(),"get_workspaces",json!({}),false,false).await.is_err());
        for args in [json!({"expectedRevision":0,"method":"GET","url":"https://example.invalid/","scripts":{"enabled":true}}),json!({"expectedRevision":0,"method":"GET","url":"https://user:password@example.invalid/"}),json!({"expectedRevision":0,"method":"GET","url":"https://example.invalid/","bodyBase64":STANDARD.encode(vec![0;MAX_BODY+1])}),json!({"expectedRevision":0,"method":"CONNECT","url":"https://example.invalid/"})] {assert!(execute(app.clone(),"create_request",args,true,false).await.is_err());}
        let draft=serde_json::to_value(captured().request).unwrap();let editors=(0..100).map(|i|json!({"id":format!("e{i}"),"draft":draft,"parentId":null,"selected":null})).collect::<Vec<_>>();
        app.engine.store.save_setting("workspace.v1",&json!({"revision":3,"editors":editors,"activeEditor":"e0"})).unwrap();
        assert!(execute(app.clone(),"create_request",json!({"expectedRevision":3,"method":"GET","url":"https://example.invalid/"}),true,false).await.is_err());
        assert_eq!(app.engine.store.workspace_revisions().unwrap()["workspaceRevision"],3);
    }
    #[test]
    fn body_chunks_preserve_unaligned_offsets_empty_and_padded_binary() {
        for length in 0..20 {
            let bytes=(0..length).map(|i|i as u8).collect::<Vec<_>>();let draft=json!({"method":"POST","url":"https://example.invalid/","headers":[],"bodyBase64":STANDARD.encode(&bytes)});
            for offset in 0..=length+1 {
                let value=project(&draft,BodyRead{include_body:true,body_offset:offset as u64,body_length:7},false).unwrap();let start=offset.min(length);
                assert_eq!(STANDARD.decode(value["body"]["bodyBase64"].as_str().unwrap()).unwrap(),bytes[start..(start+7).min(length)]);
            }
        }
    }
}
