//! Portable configuration snapshots. Machine secrets and live process state are
//! deliberately outside the format and the atomic import boundary.
use anyhow::{ensure,Context,Result};
use base64::{engine::general_purpose::STANDARD,Engine as _};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{collections::{BTreeMap,BTreeSet},io::Write,time::{SystemTime,UNIX_EPOCH}};
use crate::{store::{Store,Library,Workspace},scripts::Scripts,plugins::Plugin};
pub(crate) const CONFIG_KEYS:&[&str]=&["library.v1","workspace.v1","interception.v1","blocking.v1","capture-scripts.v1","plugins.v1","decryption.bypass.v1","upstream.profiles.v1","mcp.server.v1","mcp.clients.v1","ui.preferences.v1"];
pub(crate) struct Snapshot {pub revision:String,pub settings:BTreeMap<String,Value>}

pub const MAX_DOCUMENT_BYTES:usize=16*1024*1024;
const PREF_KEY:&str="ui.preferences.v1";
const SECTION_KEYS:&[(&str,&str)]=&[("preferences",PREF_KEY),("library","library.v1"),("workspace","workspace.v1"),("interception","interception.v1"),("blocking","blocking.v1"),("scripts","capture-scripts.v1"),("plugins","plugins.v1"),("decryption","decryption.bypass.v1"),("upstream","upstream.profiles.v1")];
const PREF_KEYS:&[&str]=&["http-capture.theme","http-capture.language","http-capture.zoom","http-capture.listener.v1","http-capture.body-editor.auto-format-paste.v1","http-capture.capture.columns.v1","http-capture.capture.column-widths","http-capture.capture.id-sort","http-capture.layout.inspector-direction","http-capture.layout.composer-columns","http-capture.layout.capture-list","http-capture.layout.capture-messages-row","http-capture.layout.capture-messages-column","http-capture.code-target","http-capture.clienthello-templates.v1","http-capture.grpc.proto.v1","http-capture.request-modes"];

#[derive(Clone,Default,Deserialize)]
#[serde(rename_all="camelCase",default,deny_unknown_fields)]
pub struct ExportOptions {pub include_request_data:bool,pub include_code:bool,pub include_workspace:bool,pub include_proxy_usernames:bool}
#[derive(Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Package {format:String,schema_version:u32,export_id:String,exported_at:u64,source_version:String,sections:BTreeMap<String,Section>,#[serde(default)]warnings:Vec<String>}
#[derive(Clone,Serialize,Deserialize)]#[serde(deny_unknown_fields)]struct Section {version:u32,data:Value}
#[derive(Clone,Serialize)]pub struct SectionSummary {pub id:String,pub count:usize}
#[derive(Clone,Serialize)]pub struct Summary {pub sections:Vec<SectionSummary>,pub warnings:Vec<String>}
#[derive(Serialize)]pub struct Exported {pub document:String,pub summary:Summary}
#[derive(Serialize)]#[serde(rename_all="camelCase")]
pub struct Preview {pub expected_revision:String,pub sections:Vec<SectionSummary>,pub warnings:Vec<String>,pub requires_restart:bool,pub source_version:String,pub exported_at:u64}
#[derive(Serialize)]#[serde(rename_all="camelCase")]
pub struct Imported {pub epoch:String,pub imported_sections:Vec<String>,pub preferences:BTreeMap<String,String>,pub requires_restart:bool,pub recovery_available:bool,pub warnings:Vec<String>}

// A strict JSON visitor rejects duplicate keys at every nesting level. Parsing a
// Value first with serde_json would already have silently discarded duplicates.
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D:serde::Deserializer<'de>>(deserializer:D)->std::result::Result<Self,D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value=Strict;
            fn expecting(&self,f:&mut std::fmt::Formatter)->std::fmt::Result{f.write_str("JSON without duplicate or prototype keys")}
            fn visit_bool<E:serde::de::Error>(self,v:bool)->std::result::Result<Strict,E>{Ok(Strict(json!(v)))}
            fn visit_i64<E:serde::de::Error>(self,v:i64)->std::result::Result<Strict,E>{Ok(Strict(json!(v)))}
            fn visit_u64<E:serde::de::Error>(self,v:u64)->std::result::Result<Strict,E>{Ok(Strict(json!(v)))}
            fn visit_f64<E:serde::de::Error>(self,v:f64)->std::result::Result<Strict,E>{serde_json::Number::from_f64(v).map(|n|Strict(Value::Number(n))).ok_or_else(||E::custom("Non-finite JSON number"))}
            fn visit_str<E:serde::de::Error>(self,v:&str)->std::result::Result<Strict,E>{Ok(Strict(json!(v)))}
            fn visit_string<E:serde::de::Error>(self,v:String)->std::result::Result<Strict,E>{Ok(Strict(json!(v)))}
            fn visit_none<E:serde::de::Error>(self)->std::result::Result<Strict,E>{Ok(Strict(Value::Null))}
            fn visit_unit<E:serde::de::Error>(self)->std::result::Result<Strict,E>{Ok(Strict(Value::Null))}
            fn visit_seq<A:serde::de::SeqAccess<'de>>(self,mut seq:A)->std::result::Result<Strict,A::Error>{
                let mut values=Vec::new();while let Some(value)=seq.next_element::<Strict>()? {if values.len()>=10000{return Err(serde::de::Error::custom("JSON array exceeds 10000 items"));}values.push(value.0);}Ok(Strict(Value::Array(values)))
            }
            fn visit_map<A:serde::de::MapAccess<'de>>(self,mut map:A)->std::result::Result<Strict,A::Error>{
                let mut values=serde_json::Map::new();while let Some(key)=map.next_key::<String>()? {
                    if key.len()>4096||matches!(key.as_str(),"__proto__"|"prototype"|"constructor")||values.contains_key(&key){return Err(serde::de::Error::custom("Duplicate, oversized or unsupported JSON object key"));}
                    values.insert(key,map.next_value::<Strict>()?.0);
                }Ok(Strict(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
fn strict_json(text:&str)->Result<Value>{Ok(serde_json::from_str::<Strict>(text).context("Invalid configuration JSON")?.0)}
fn now()->u64{SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64}
fn section(data:Value)->Section{Section{version:1,data}}
fn warning(warnings:&mut Vec<String>,value:&str){if !warnings.iter().any(|v|v==value){warnings.push(value.into());}}
fn count(id:&str,data:&Value)->usize {match id {"library"=>data["requests"].as_array().map_or(0,Vec::len),"workspace"=>data["editors"].as_array().map_or(0,Vec::len),"interception"|"blocking"=>data["rules"].as_array().map_or(0,Vec::len),"preferences"=>data.as_object().map_or(0,serde_json::Map::len),"mcp"=>data["clients"].as_array().map_or(0,Vec::len),_=>data.as_array().map_or(1,Vec::len)}}
fn summary(package:&Package)->Summary {Summary{sections:package.sections.iter().map(|(id,s)|SectionSummary{id:id.clone(),count:count(id,&s.data)}).collect(),warnings:package.warnings.clone()}}
fn serialize(package:Package)->Result<Exported>{let result=Exported{summary:summary(&package),document:serde_json::to_string_pretty(&package)?};ensure!(result.document.len()<=MAX_DOCUMENT_BYTES,"Configuration file exceeds 16 MiB; exclude request bodies or workspace");Ok(result)}

#[derive(Deserialize)]#[serde(deny_unknown_fields)]struct Listener {host:String,port:u16}
#[derive(Deserialize)]#[serde(deny_unknown_fields)]struct Template {name:String,hex:String}
pub fn validate_preferences(values:&BTreeMap<String,String>)->Result<()> {
    ensure!(values.len()<=PREF_KEYS.len(),"Too many UI preferences");
    for (key,value) in values {
        ensure!(PREF_KEYS.contains(&key.as_str())&&value.len()<=1024*1024,"Unsupported or oversized UI preference");
        match key.as_str() {
            "http-capture.theme"=>ensure!(["light","dark","cyberpunk","arctic","dusk","paper","sonar","forge","matrix","sakura"].contains(&value.as_str()),"Unknown UI theme"),
            "http-capture.language"=>ensure!(["system","zh-CN","en"].contains(&value.as_str()),"Unknown UI language"),
            "http-capture.zoom"=>ensure!(["75","90","100","110","125","150"].contains(&value.as_str()),"Invalid UI zoom"),
            "http-capture.listener.v1"=>{let listener:Listener=serde_json::from_value(strict_json(value)?)?;ensure!(listener.port>=1024,"Invalid listener port");crate::network::listener_address(&listener.host,listener.port)?;},
            "http-capture.body-editor.auto-format-paste.v1"=>ensure!(["true","false"].contains(&value.as_str()),"Invalid paste-format preference"),
            "http-capture.capture.id-sort"=>ensure!(["asc","desc"].contains(&value.as_str()),"Invalid ID sort preference"),
            "http-capture.layout.inspector-direction"=>ensure!(["row","column"].contains(&value.as_str()),"Invalid inspector layout"),
            "http-capture.capture.columns.v1"=>{let columns:Vec<String>=serde_json::from_value(strict_json(value)?)?;ensure!(columns.len()<=64&&columns.iter().all(|c|!c.is_empty()&&c.len()<=64),"Invalid capture columns");},
            "http-capture.capture.column-widths"=>{let widths:BTreeMap<String,u32>=serde_json::from_value(strict_json(value)?)?;ensure!(widths.len()<=64&&widths.iter().all(|(id,v)|id.len()<=64&&(32..=1200).contains(v)),"Invalid capture column widths");},
            "http-capture.code-target"=>ensure!(!value.is_empty()&&value.len()<=128&&value.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_.".contains(&b)),"Invalid code target preference"),
            "http-capture.clienthello-templates.v1"=>{let templates:Vec<Template>=serde_json::from_value(strict_json(value)?)?;ensure!(templates.len()<=100,"Too many TLS templates");for t in templates{ensure!(!t.name.trim().is_empty()&&t.name.len()<=128&&(18..=131080).contains(&t.hex.len())&&t.hex.len()%2==0&&t.hex.bytes().all(|b|b.is_ascii_hexdigit())&&t.hex[..2].eq_ignore_ascii_case("16")&&t.hex[10..12]==*"01"&&usize::from_str_radix(&t.hex[6..10],16).is_ok_and(|length|length==t.hex.len()/2-5),"Invalid ClientHello template");}},
            "http-capture.grpc.proto.v1"=>{let sources:Vec<String>=serde_json::from_value(strict_json(value)?)?;ensure!(sources.len()<=100&&sources.iter().map(String::len).sum::<usize>()<=1024*1024,"Proto source exceeds migration limits");},
            "http-capture.request-modes"=>{let modes:BTreeMap<String,String>=serde_json::from_value(strict_json(value)?)?;ensure!(modes.len()<=100&&modes.iter().all(|(id,mode)|id.len()<=128&&["http","grpc","websocket"].contains(&mode.as_str())),"Invalid request modes");},
            _=>{let ratio:f64=value.parse().context("Invalid pane ratio")?;ensure!(ratio.is_finite()&&(5.0..=95.0).contains(&ratio),"Invalid pane ratio");}
        }
    }Ok(())
}

#[derive(Clone,Serialize,Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
struct PortableProxy {id:String,name:String,url:String,#[serde(default)]username:String,auth_enabled:bool}
#[derive(Clone,Serialize,Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
struct PortableClient {id:String,name:String,url:String,#[serde(default)]token_required:bool}
#[derive(Clone,Serialize,Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]
struct PortableMcp {port:u16,clients:Vec<PortableClient>}
fn validate_proxies(items:&[PortableProxy])->Result<()> {
    ensure!(items.len()<=100,"At most 100 upstream proxies can be imported");let mut ids=BTreeSet::new();let mut names=BTreeSet::new();
    for item in items {
        ensure!(!item.id.is_empty()&&item.id.len()<=128&&ids.insert(&item.id)&&!item.name.trim().is_empty()&&item.name.chars().count()<=80&&names.insert(&item.name),"Invalid or duplicate upstream proxy ID/name");
        let url=url::Url::parse(&item.url).context("Invalid upstream proxy URL")?;
        ensure!(matches!(url.scheme(),"http"|"socks5")&&url.host_str().is_some()&&url.username().is_empty()&&url.password().is_none()&&url.query().is_none()&&url.fragment().is_none()&&(url.path().is_empty()||url.path()=="/"),"Upstream proxy URL must contain only its scheme, host and port");
        ensure!(item.username.len()<=255&&!item.username.contains([':', '\r','\n','\0']),"Invalid upstream proxy username");
    }Ok(())
}
fn validate_mcp(mcp:&PortableMcp)->Result<()> {
    ensure!(mcp.port>0&&mcp.clients.len()<=32,"Invalid MCP port or too many MCP clients");let mut ids=BTreeSet::new();
    for client in &mcp.clients {
        ensure!(!client.id.is_empty()&&client.id.len()<=128&&client.id.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_.".contains(&b))&&ids.insert(&client.id)&&!client.name.is_empty()&&client.name.len()<=160&&!client.name.chars().any(char::is_control),"Invalid MCP client ID/name");
        let url=url::Url::parse(&client.url).context("Invalid MCP endpoint")?;
        ensure!(client.url.len()<=4096&&matches!(url.scheme(),"http"|"https")&&url.host_str().is_some()&&url.username().is_empty()&&url.password().is_none()&&url.fragment().is_none()&&url.query().is_none(),"Portable MCP endpoints cannot contain credentials, query strings or fragments");
    }Ok(())
}
fn validate_draft(draft:&crate::model::RequestDraft)->Result<()> {
    ensure!(!draft.method.is_empty()&&draft.method.len()<=32&&crate::http1::token(&draft.method),"Invalid saved request method");
    ensure!(draft.url.len()<=8192,"Saved request URL is too long");
    if !draft.url.is_empty(){
        let template=regex::Regex::new(r"\{\{\s*([^{}]+?)\s*\}\}")?;
        let mut sample=draft.url.clone();
        if template.find(&sample).is_some_and(|m|m.start()==0){sample=template.replacen(&sample,1,"https://placeholder.invalid").into_owned();}
        let sample=template.replace_all(&sample,"placeholder");
        ensure!(!sample.contains(['{','}']),"Invalid saved URL template");
        let url=url::Url::parse(&sample).context("Invalid saved request URL")?;ensure!(matches!(url.scheme(),"http"|"https"|"ws"|"wss")&&url.host_str().is_some()&&url.username().is_empty()&&url.password().is_none(),"Invalid saved request URL or embedded credentials");
    }
    ensure!(draft.headers.len()<=512&&draft.pseudo_headers.len()<=16,"Too many saved request headers");
    for header in &draft.headers {ensure!(!header.name.eq_ignore_ascii_case("proxy-authorization"),"Proxy credentials cannot be imported");ensure!((header.name.is_empty()&&header.value.is_empty())||(!header.name.is_empty()&&header.name.len()<=256&&crate::http1::token(&header.name)&&header.value.len()<=65536&&!header.value.contains(['\r','\n','\0'])),"Invalid saved request header");}
    for header in &draft.pseudo_headers {ensure!([":method",":scheme",":authority",":path",":protocol"].contains(&header.name.as_str())&&header.value.len()<=8192&&!header.value.contains(['\r','\n','\0']),"Invalid saved pseudo header");}
    ensure!(draft.body_base64.len()<=12*1024*1024&&STANDARD.decode(draft.body_base64.as_str()).context("Invalid saved request Base64 body")?.len()<=8*1024*1024,"Saved request body exceeds 8 MiB");
    validate_script(&draft.scripts)?;Ok(())
}
fn validate_script(scripts:&Scripts)->Result<()> {scripts.validate()?;if !scripts.before.trim().is_empty()||!scripts.after.trim().is_empty(){crate::scripts::validate_scripts(scripts)?;}Ok(())}
fn validate_library(library:&Library)->Result<()> {
    let mut ids=BTreeSet::new();let entries=library.collections.iter().map(|v|(&v.id,&v.name)).chain(library.requests.iter().map(|v|(&v.id,&v.name))).chain(library.environments.iter().map(|v|(&v.id,&v.name))).chain(library.scripts.iter().map(|v|(&v.id,&v.name))).collect::<Vec<_>>();
    ensure!(entries.len()<=10000,"Library exceeds 10000 items");
    for (id,name) in entries {ensure!(!id.is_empty()&&id.len()<=128&&ids.insert(id.as_str())&&!name.trim().is_empty()&&name.len()<=512,"Invalid or duplicate library ID/name");}
    for request in &library.requests {ensure!(library.collections.iter().any(|c|c.id==request.collection_id),"Saved request refers to a missing collection");validate_draft(&request.draft)?;ensure!(request.request_mode.as_ref().is_none_or(|mode|["http","grpc","websocket"].contains(&mode.as_str())),"Unknown saved request mode");if let Some(grpc)=&request.grpc_draft{ensure!(grpc.method.len()<=8192&&grpc.json.len()<=1024*1024,"gRPC draft is too large");}}
    for script in &library.scripts {validate_script(&script.scripts)?;}
    for env in &library.environments {ensure!(env.variables.len()<=1000&&env.variables.iter().all(|(key,value)|!key.is_empty()&&key.len()<=512&&value.len()<=65536),"Environment variables exceed migration limits");}Ok(())
}
fn validate_workspace(workspace:&Workspace)->Result<()> {
    ensure!(workspace.editors.len()<=100,"At most 100 request tabs can be imported");let mut ids=BTreeSet::new();
    for editor in &workspace.editors {ensure!(!editor.id.is_empty()&&editor.id.len()<=128&&ids.insert(&editor.id),"Invalid or duplicate request tab ID");validate_draft(&editor.draft)?;if let Some(grpc)=&editor.grpc_draft{ensure!(grpc.method.len()<=8192&&grpc.json.len()<=1024*1024,"gRPC draft is too large");}}
    ensure!(workspace.active_editor.is_empty()||ids.iter().any(|id|**id==workspace.active_editor),"Active request tab does not exist");Ok(())
}
fn validate_section(id:&str,data:&Value)->Result<()> {
    match id {
        "preferences"=>validate_preferences(&serde_json::from_value(data.clone())?)?,
        "library"=>validate_library(&serde_json::from_value(data.clone())?)?,
        "workspace"=>validate_workspace(&serde_json::from_value(data.clone())?)?,
        "interception"=>crate::intercept::validate_config(&serde_json::from_value(data.clone())?)?,
        "blocking"=>crate::blocking::validate_config(&serde_json::from_value(data.clone())?)?,
        "scripts"=>validate_script(&serde_json::from_value(data.clone())?)?,
        "plugins"=>{let plugins:Vec<Plugin>=serde_json::from_value(data.clone())?;ensure!(plugins.len()<=32,"At most 32 plugins can be imported");let mut ids=BTreeSet::new();for plugin in plugins{ensure!(ids.insert(plugin.id.clone()),"Duplicate plugin ID");crate::plugins::validate_plugin(&plugin)?;}},
        "decryption"=>{let hosts:Vec<String>=serde_json::from_value(data.clone())?;ensure!(hosts.len()<=256,"Too many passthrough hosts");for host in hosts{let domain=host.strip_prefix("*.").unwrap_or(&host);ensure!(!domain.is_empty()&&domain.len()<=253&&domain.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'.'||b==b'-'),"Invalid passthrough host");}},
        "upstream"=>validate_proxies(&serde_json::from_value::<Vec<PortableProxy>>(data.clone())?)?,
        "mcp"=>validate_mcp(&serde_json::from_value(data.clone())?)?,
        _=>anyhow::bail!("Unsupported configuration category: {id}"),
    }Ok(())
}
fn parse_document(document:&str)->Result<Package> {
    ensure!(!document.is_empty()&&document.len()<=MAX_DOCUMENT_BYTES,"Configuration file must be between 1 byte and 16 MiB");
    let package:Package=serde_json::from_value(strict_json(document)?)?;
    ensure!(package.format=="http-capture-config"&&package.schema_version==1,"Unsupported configuration format/version; update the application before importing");
    uuid::Uuid::parse_str(&package.export_id).context("Invalid configuration export ID")?;
    ensure!(!package.source_version.is_empty()&&package.source_version.len()<=64&&package.warnings.len()<=256&&package.warnings.iter().all(|v|v.len()<=2048),"Invalid source version or warnings");
    ensure!(!package.sections.is_empty()&&package.sections.len()<=10,"Invalid configuration categories");
    for (id,section) in &package.sections {ensure!(section.version==1,"Unsupported {id} category version");validate_section(id,&section.data).with_context(||format!("Invalid {id} configuration"))?;}
    Ok(package)
}
pub fn validate_document(document:&str)->Result<()> {parse_document(document).map(|_|())}

fn sanitized_url(raw:&str,include_data:bool)->String {
    // Preserve environment templates such as {{baseUrl}}/path. Parse their
    // query separately because they are resolved only when the user sends.
    if raw.contains("{{") {
        let raw=raw.split('#').next().unwrap_or_default();
        let mut base=raw.to_owned();
        if let Some(scheme)=base.find("://") {let start=scheme+3;let end=base[start..].find(['/', '?']).map_or(base.len(),|n|start+n);if let Some(at)=base[start..end].rfind('@'){base.replace_range(start..start+at+1,"");}}
        if !include_data {if let Some((path,query))=base.split_once('?'){let query=url::form_urlencoded::Serializer::new(String::new()).extend_pairs(url::form_urlencoded::parse(query.as_bytes()).map(|(name,_)|(name,String::new()))).finish();return format!("{path}?{query}");}}
        return base;
    }
    let Ok(mut url)=url::Url::parse(raw)else{return raw.to_owned();};let _=url.set_username("");let _=url.set_password(None);url.set_fragment(None);
    if !include_data&&url.query().is_some(){let pairs=url.query_pairs().map(|(name,_)|(name.into_owned(),String::new())).collect::<Vec<_>>();url.query_pairs_mut().clear().extend_pairs(pairs);}
    url.to_string()
}
fn sanitize_scripts(scripts:&mut Scripts,options:&ExportOptions){scripts.enabled=false;if !options.include_code{*scripts=Scripts::default();}else if !options.include_request_data{for value in scripts.variables.values_mut(){value.clear();}}}
fn sanitize_draft(draft:&mut crate::model::RequestDraft,options:&ExportOptions){
    draft.url=sanitized_url(&draft.url,options.include_request_data);
    draft.headers.retain(|h|!h.name.eq_ignore_ascii_case("proxy-authorization"));
    if !options.include_request_data {for header in &mut draft.headers{header.value.clear();header.disabled=true;}draft.body_base64=Default::default();}
    // Captured pseudo values can duplicate old URL/query/auth data and are
    // regenerated by the selected sender, not portable connection state.
    draft.pseudo_headers.clear();sanitize_scripts(&mut draft.scripts,options);
}

fn saved<T:serde::de::DeserializeOwned+Default>(snapshot:&Snapshot,key:&str)->Result<T>{snapshot.settings.get(key).cloned().map(serde_json::from_value).transpose().map(|value|value.unwrap_or_default()).map_err(Into::into)}
fn make_package(snapshot:&Snapshot,mut preferences:BTreeMap<String,String>,options:&ExportOptions,version:&str)->Result<Package> {
    validate_preferences(&preferences)?;
    let mut sections=BTreeMap::new();let mut warnings=vec!["普通 JSON 配置文件没有加密，请妥善保管。".into(),"不包含代理密码、CA 私钥、证书信任、本机控制令牌、MCP Token、流量记录和系统代理恢复信息。".into()];
    let mut library:Library=saved(snapshot,"library.v1")?;
    for request in &mut library.requests {sanitize_draft(&mut request.draft,options);if !options.include_request_data {if let Some(grpc)=&mut request.grpc_draft{grpc.json.clear();}}}
    for env in &mut library.environments {if !options.include_request_data {for value in env.variables.values_mut(){value.clear();}}}
    if !options.include_code {library.scripts.clear();}else{for script in &mut library.scripts{sanitize_scripts(&mut script.scripts,options);}}
    library.revision=0;sections.insert("library".into(),section(serde_json::to_value(library)?));
    let mut workspace:Workspace=saved(snapshot,"workspace.v1")?;
    if options.include_workspace {
        for editor in &mut workspace.editors {sanitize_draft(&mut editor.draft,options);editor.selected=None;editor.parent_id=None;if !options.include_request_data {if let Some(grpc)=&mut editor.grpc_draft{grpc.json.clear();}}}
        workspace.revision=0;
        if let Some(raw)=preferences.get_mut("http-capture.request-modes"){let mut modes:BTreeMap<String,String>=serde_json::from_value(strict_json(raw)?)?;modes.retain(|id,_|workspace.editors.iter().any(|e|e.id==*id));*raw=serde_json::to_string(&modes)?;}
        sections.insert("workspace".into(),section(serde_json::to_value(workspace)?));
    } else {preferences.remove("http-capture.request-modes");}
    if !options.include_code {preferences.remove("http-capture.grpc.proto.v1");preferences.remove("http-capture.clienthello-templates.v1");}
    sections.insert("preferences".into(),section(serde_json::to_value(preferences)?));
    let mut interception:crate::intercept::Config=snapshot.settings.get("interception.v1").cloned().map(serde_json::from_value).transpose()?.unwrap_or(crate::intercept::Config{scope:"all".into(),..Default::default()});interception.request=false;interception.response=false;
    sections.insert("interception".into(),section(serde_json::to_value(interception)?));
    let mut blocking:crate::blocking::Config=saved(snapshot,"blocking.v1")?;blocking.enabled=false;sections.insert("blocking".into(),section(serde_json::to_value(blocking)?));
    let hosts:Vec<String>=saved(snapshot,"decryption.bypass.v1")?;sections.insert("decryption".into(),section(json!(hosts)));
    let profiles:Vec<Value>=saved(snapshot,"upstream.profiles.v1")?;
    let profiles=profiles.into_iter().map(|p|PortableProxy{id:p["id"].as_str().unwrap_or_default().into(),name:p["name"].as_str().unwrap_or_default().into(),url:p["url"].as_str().unwrap_or_default().into(),username:if options.include_proxy_usernames{p["username"].as_str().unwrap_or_default().into()}else{String::new()},auth_enabled:p["authEnabled"].as_bool().unwrap_or(false)}).collect::<Vec<_>>();
    if profiles.iter().any(|p|p.auth_enabled){warning(&mut warnings,"需要认证的代理必须在目标设备重新填写密码；未勾选用户名时还需补填用户名。");}
    sections.insert("upstream".into(),section(serde_json::to_value(profiles)?));
    let server=snapshot.settings.get("mcp.server.v1");let clients:Vec<Value>=saved(snapshot,"mcp.clients.v1")?;
    let mut portable=Vec::new();
    for client in clients {
        if client["transport"]!="http"{warning(&mut warnings,"本机 stdio MCP 程序和参数不迁移，请在新设备重新配置。");continue;}
        let raw=client["url"].as_str().unwrap_or_default();let mut url=url::Url::parse(raw).context("Invalid stored MCP endpoint")?;
        if url.query().is_some(){warning(&mut warnings,"MCP 地址的查询参数已排除；如端点依赖参数，请在新设备补填。");}
        let _=url.set_username("");let _=url.set_password(None);url.set_query(None);url.set_fragment(None);
        portable.push(PortableClient{id:client["id"].as_str().unwrap_or_default().into(),name:client["name"].as_str().unwrap_or_default().into(),url:url.to_string(),token_required:client["tokenRequired"].as_bool().unwrap_or(false)});
    }
    sections.insert("mcp".into(),section(serde_json::to_value(PortableMcp{port:server.and_then(|v|v["port"].as_u64()).and_then(|v|u16::try_from(v).ok()).unwrap_or(1422),clients:portable})?));
    if options.include_code {
        let mut scripts:Scripts=saved(snapshot,"capture-scripts.v1")?;sanitize_scripts(&mut scripts,options);sections.insert("scripts".into(),section(serde_json::to_value(scripts)?));
        let mut plugins:Vec<Plugin>=saved(snapshot,"plugins.v1")?;for plugin in &mut plugins{plugin.enabled=false;}sections.insert("plugins".into(),section(serde_json::to_value(plugins)?));
        warning(&mut warnings,"脚本、插件代码和 proto 可能含硬编码业务信息；导入后脚本、插件初始禁用。");
    }
    if !options.include_request_data {warning(&mut warnings,"请求头值、URL 查询值、正文、gRPC 消息和变量值已排除，导入后需要补填再发送；空请求头初始禁用。");}else{warning(&mut warnings,"请求内容和变量值可能含 Cookie、Authorization、Token 或业务凭据，当前文件会明文保留这些数据。");}
    warning(&mut warnings,"规则条件、名称、URL 路径和连接地址仍可能包含业务信息，请在分享前检查文件。");
    let package=Package{format:"http-capture-config".into(),schema_version:1,export_id:uuid::Uuid::new_v4().to_string(),exported_at:now(),source_version:version.into(),sections,warnings};
    for (id,s) in &package.sections{validate_section(id,&s.data).with_context(||format!("Unable to export {id}"))?;}
    Ok(package)
}
pub fn export(store:&Store,preferences:BTreeMap<String,String>,options:ExportOptions,source_version:&str)->Result<Exported>{serialize(make_package(&store.configuration_snapshot()?,preferences,&options,source_version)?)}
pub fn preview(store:&Store,document:&str)->Result<Preview>{
    let package=parse_document(document)?;let snapshot=store.configuration_snapshot()?;
    let mut warnings=package.warnings.clone();warning(&mut warnings,"导入会替换勾选的类别，不会合并；提交后需要重启应用。");
    warning(&mut warnings,"导入不会启用抓包、TUN、系统代理、断点、阻止、脚本、插件或 MCP；这些功能需要在本机重新启用。");
    warning(&mut warnings,"无论是否勾选 MCP 类别，本机 MCP 服务和所有 MCP 操作权限都会关闭，需在重启后重新授权。");
    if package.sections.contains_key("library"){warning(&mut warnings,"替换集合时，现有标签中指向已删除收藏的关联会清除。");}
    if package.sections.contains_key("upstream"){warning(&mut warnings,"替换代理会为导入代理生成新 ID；现有未导入请求的代理关联会清除，需要重新选择代理。");}
    Ok(Preview{expected_revision:snapshot.revision,sections:summary(&package).sections,warnings,requires_restart:true,source_version:package.source_version,exported_at:package.exported_at})
}

pub struct PreparedImport {expected:String,updates:BTreeMap<String,Value>,recovery:Value,result:Imported}
fn remap_proxy(draft:&mut crate::model::RequestDraft,ids:&BTreeMap<String,String>,imported:bool,replace_proxies:bool){
    if imported {draft.upstream_profile_id=draft.upstream_profile_id.as_ref().and_then(|id|ids.get(id).cloned());draft.scripts.enabled=false;}
    else if replace_proxies {draft.upstream_profile_id=None;}
}
pub fn prepare_import(store:&Store,document:&str,selected:&[String],expected:&str,current_preferences:BTreeMap<String,String>,source_version:&str)->Result<PreparedImport>{
    store.configuration_ready()?;validate_preferences(&current_preferences)?;let package=parse_document(document)?;let snapshot=store.configuration_snapshot()?;
    ensure!(snapshot.revision==expected,"Configuration changed after preview; preview the file again before importing");
    ensure!(!selected.is_empty()&&selected.len()<=10,"Choose at least one configuration category");let chosen=selected.iter().map(String::as_str).collect::<BTreeSet<_>>();ensure!(chosen.len()==selected.len()&&chosen.iter().all(|id|package.sections.contains_key(*id)),"Invalid or duplicate import category");
    let restore_preferences=if current_preferences.is_empty(){saved(&snapshot,PREF_KEY)?}else{current_preferences};
    let recovery_package=make_package(&snapshot,restore_preferences,&ExportOptions{include_request_data:true,include_code:true,include_workspace:true,include_proxy_usernames:true},source_version)?;
    let recovery_export=serialize(recovery_package.clone())?;
    let recovery=serde_json::to_value(recovery_package)?;drop(recovery_export);
    let mut updates=BTreeMap::new();let mut warnings=package.warnings.clone();let mut proxy_ids=BTreeMap::new();
    if chosen.contains("upstream") {
        let mut profiles:Vec<PortableProxy>=serde_json::from_value(package.sections["upstream"].data.clone())?;
        for profile in &mut profiles {let id=uuid::Uuid::new_v4().to_string();proxy_ids.insert(profile.id.clone(),id.clone());profile.id=id;}
        // No rememberPassword flag or password can enter this portable DTO.
        updates.insert("upstream.profiles.v1".into(),serde_json::to_value(profiles)?);
    }
    let mut library:Library=if chosen.contains("library"){serde_json::from_value(package.sections["library"].data.clone())?}else{saved(&snapshot,"library.v1")?};
    let mut library_changed=chosen.contains("library");
    for request in &mut library.requests {if chosen.contains("library")||chosen.contains("upstream"){remap_proxy(&mut request.draft,&proxy_ids,chosen.contains("library"),chosen.contains("upstream"));library_changed=true;}}
    if chosen.contains("library"){for script in &mut library.scripts {script.scripts.enabled=false;}}
    if library_changed {library.revision=saved::<Library>(&snapshot,"library.v1")?.revision.checked_add(1).context("Library revision overflow")?;validate_library(&library)?;updates.insert("library.v1".into(),serde_json::to_value(&library)?);}
    let mut workspace:Workspace=if chosen.contains("workspace"){serde_json::from_value(package.sections["workspace"].data.clone())?}else{saved(&snapshot,"workspace.v1")?};
    let mut editor_ids=BTreeMap::new();let mut workspace_changed=chosen.contains("workspace");
    for editor in &mut workspace.editors {
        if chosen.contains("workspace") {let id=uuid::Uuid::new_v4().to_string();editor_ids.insert(editor.id.clone(),id.clone());editor.id=id;editor.selected=None;editor.parent_id=None;}
        if chosen.contains("workspace")||chosen.contains("upstream"){remap_proxy(&mut editor.draft,&proxy_ids,chosen.contains("workspace"),chosen.contains("upstream"));workspace_changed=true;}
        if editor.saved_request_id.as_ref().is_some_and(|id|!library.requests.iter().any(|r|r.id==*id)){editor.saved_request_id=None;workspace_changed=true;}
    }
    if chosen.contains("workspace"){workspace.active_editor=editor_ids.get(&workspace.active_editor).cloned().unwrap_or_default();}
    if workspace_changed {workspace.revision=saved::<Workspace>(&snapshot,"workspace.v1")?.revision.checked_add(1).context("Workspace revision overflow")?;validate_workspace(&workspace)?;updates.insert("workspace.v1".into(),serde_json::to_value(&workspace)?);}
    let mut preferences=BTreeMap::new();
    if chosen.contains("preferences") {
        preferences=serde_json::from_value::<BTreeMap<String,String>>(package.sections["preferences"].data.clone())?;
        if chosen.contains("workspace") {
            if let Some(raw)=preferences.get_mut("http-capture.request-modes") {let modes:BTreeMap<String,String>=serde_json::from_value(strict_json(raw)?)?;let mapped=modes.into_iter().filter_map(|(id,mode)|editor_ids.get(&id).map(|new|(new.clone(),mode))).collect::<BTreeMap<_,_>>();*raw=serde_json::to_string(&mapped)?;}
        }else{preferences.remove("http-capture.request-modes");}
        updates.insert(PREF_KEY.into(),serde_json::to_value(&preferences)?);
    }
    for (id,key) in SECTION_KEYS {
        if !chosen.contains(*id)||matches!(*id,"preferences"|"library"|"workspace"|"upstream"){continue;}
        let mut value=package.sections[*id].data.clone();
        match *id {
            "interception"=>{value["request"]=json!(false);value["response"]=json!(false);},
            "blocking"|"scripts"=>value["enabled"]=json!(false),
            "plugins"=>{for plugin in value.as_array_mut().context("Invalid plugins")?{plugin["enabled"]=json!(false);}},_=>{}
        }updates.insert((*key).into(),value);
    }
    // Even when the MCP category is not selected, an old autostart flag must
    // not reopen an automation listener after configuration import.
    let port=if chosen.contains("mcp"){package.sections["mcp"].data["port"].as_u64().unwrap_or(1422)}else{snapshot.settings.get("mcp.server.v1").and_then(|v|v["port"].as_u64()).unwrap_or(1422)};
    updates.insert("mcp.server.v1".into(),json!({"enabled":false,"port":port,"allowSend":false,"allowCaptureControl":false,"includeSensitiveHeaders":false,"allowWorkspaceWrite":false,"allowRulesWrite":false,"allowScriptsWrite":false}));
    if chosen.contains("mcp") {let mcp:PortableMcp=serde_json::from_value(package.sections["mcp"].data.clone())?;let clients=mcp.clients.into_iter().map(|c|json!({"id":uuid::Uuid::new_v4().to_string(),"name":c.name,"transport":"http","url":c.url,"command":"","args":[],"tokenRequired":c.token_required})).collect::<Vec<_>>();updates.insert("mcp.clients.v1".into(),json!(clients));}
    warning(&mut warnings,"配置已导入。重启后生效；请先补填缺失数据，再手动启用网络、脚本和 MCP 权限。");
    Ok(PreparedImport{expected:expected.into(),updates,recovery,result:Imported{epoch:uuid::Uuid::new_v4().to_string(),imported_sections:selected.to_vec(),preferences,requires_restart:true,recovery_available:true,warnings}})
}
pub fn commit_import(store:&Store,prepared:PreparedImport)->Result<Imported>{
    let mut pending=json!({"epoch":prepared.result.epoch,"importedSections":prepared.result.imported_sections,"warnings":prepared.result.warnings});
    if prepared.result.imported_sections.iter().any(|s|s=="preferences"){pending["preferences"]=serde_json::to_value(&prepared.result.preferences)?;}
    store.commit_configuration(&prepared.expected,prepared.updates,&prepared.recovery,&pending)?;Ok(prepared.result)
}
/// One-time frontend recovery after a lost response or process interruption.
/// The frontend acknowledges the epoch locally; this snapshot is never exported.
pub fn status(store:&Store)->Result<Value>{
    store.configuration_migration_status()
}
pub fn recovery(store:&Store)->Result<Exported>{let value:Value=store.setting("config-migration.recovery.v1")?.context("No configuration recovery point is available")?;serialize(serde_json::from_value(value)?)}

/// Write a user-selected JSON file with one filesystem rename. NamedTempFile
/// removes incomplete output automatically on cancellation/error paths.
pub fn save_atomic(path:&std::path::Path,document:&str)->Result<()> {
    validate_document(document)?;
    let parent=path.parent().context("Invalid configuration file destination")?;
    let mut temporary=tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(document.as_bytes())?;temporary.flush()?;temporary.as_file().sync_all()?;
    ensure!(std::fs::read(temporary.path())?==document.as_bytes(),"Configuration file verification failed");
    temporary.persist(path).map_err(|error|anyhow::anyhow!("Unable to replace configuration file: {}",error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store(path:&std::path::Path)->Store{Store::open(&path.join("sessions.db")).unwrap()}
    fn preferences()->BTreeMap<String,String>{BTreeMap::from([
        ("http-capture.theme".into(),"matrix".into()),("http-capture.zoom".into(),"125".into()),
        ("http-capture.listener.v1".into(),r#"{"host":"127.0.0.1","port":8090}"#.into()),
        ("http-capture.capture.columns.v1".into(),r#"["id","method","url"]"#.into()),
        ("http-capture.capture.column-widths".into(),r#"{"url":400}"#.into()),
        ("http-capture.layout.composer-columns".into(),"50".into()),
        ("http-capture.grpc.proto.v1".into(),r#"["syntax = \"proto3\"; message Example { string text = 1; }"]"#.into()),
        ("http-capture.clienthello-templates.v1".into(),r#"[{"name":"Template","hex":"160303000401000000"}]"#.into()),
        ("http-capture.request-modes".into(),r#"{"editor-old":"grpc","orphan":"http"}"#.into()),
    ])}
    fn populate(store:&Store,label:&str){
        let draft=json!({"method":"POST","url":"{{baseUrl}}/search?token=private-query#private-fragment","headers":[{"name":"X-A","value":"one"},{"name":"Authorization","value":"private-auth"},{"name":"X-A","value":"three","disabled":true}],"bodyBase64":"AAH+/w==","upstreamProfileId":"proxy-old","scripts":{"enabled":true,"before":"throw new Error('private-code');","variables":{"password":"private-script-var"}}});
        store.save_library(serde_json::from_value(json!({"revision":0,"collections":[{"id":"collection","name":label}],"requests":[{"id":"request","name":label,"collectionId":"collection","draft":draft,"requestMode":"http"}],"environments":[{"id":"environment","name":"Development","variables":{"baseUrl":"https://private-host.invalid","password":"private-env-var"}}],"scripts":[{"id":"saved-script","name":"Template","scripts":{"before":"throw new Error('private-saved-code');"}}]})).unwrap()).unwrap();
        store.save_workspace(serde_json::from_value(json!({"revision":0,"activeEditor":"editor-old","editors":[{"id":"editor-old","draft":draft,"selected":"source-flow","parentId":"source-flow","savedRequestId":"request","grpcDraft":{"method":"/example.Echo/Call","json":"{\"text\":\"private-grpc\"}"}}]})).unwrap()).unwrap();
        store.save_setting("upstream.profiles.v1",&json!([{"id":"proxy-old","name":"Local proxy","url":"http://127.0.0.1:7897/","username":"private-user","authEnabled":true,"rememberPassword":true}])).unwrap();
        store.save_setting("interception.v1",&json!({"request":true,"response":true,"scope":"capture","rules":[{"urlContains":"example.invalid"}]})).unwrap();
        store.save_setting("blocking.v1",&json!({"enabled":true,"scope":"capture","rules":[{"id":"block-one","enabled":true,"matchType":"urlContains","pattern":"/ad"}]})).unwrap();
        store.save_setting("capture-scripts.v1",&json!({"enabled":true,"before":"throw new Error('private-global-code');","variables":{"password":"private-global-var"}})).unwrap();
        store.save_setting("plugins.v1",&json!([{"id":"demo","name":"Plugin","version":"1.0.0","apiVersion":1,"enabled":true,"scope":"all","urlPattern":".*","before":"throw new Error('private-plugin-code');"}])).unwrap();
        store.save_setting("decryption.bypass.v1",&vec!["*.example.invalid"]).unwrap();
        store.save_setting("mcp.server.v1",&json!({"enabled":true,"port":1422,"allowSend":true,"allowCaptureControl":true,"includeSensitiveHeaders":true,"allowWorkspaceWrite":true,"allowRulesWrite":true,"allowScriptsWrite":true})).unwrap();
        store.save_setting("mcp.clients.v1",&json!([{"id":"http-client","name":"Remote","transport":"http","url":"https://mcp.example.invalid/mcp?token=private-mcp-query","command":"","args":[],"tokenRequired":true},{"id":"stdio-client","name":"Local","transport":"stdio","url":"","command":"C:\\private-machine\\tool.exe","args":["private-command-arg"],"tokenRequired":false}])).unwrap();
        store.save_setting("mcp.server.token.v1",&format!("{label}-local-only-token")).unwrap();
        store.save_setting("unknown.secret.setting",&"private-unknown-setting").unwrap();
    }
    fn all_options()->ExportOptions{ExportOptions{include_request_data:true,include_code:true,include_workspace:true,include_proxy_usernames:true}}
    #[test]
    fn standard_export_strips_data_credentials_code_and_machine_state_but_preserves_templates(){
        let dir=tempfile::tempdir().unwrap();let store=store(dir.path());populate(&store,"Source");
        let exported=export(&store,preferences(),ExportOptions::default(),"0.1.99").unwrap();let package=parse_document(&exported.document).unwrap();
        for secret in ["private-query","private-fragment","private-auth","AAH+/w==","private-code","private-env-var","private-script-var","private-grpc","private-global-code","private-plugin-code","private-saved-code","private-user","Source-local-only-token","private-mcp-query","private-machine","private-command-arg","private-unknown-setting"] {assert!(!exported.document.contains(secret),"leaked {secret}");}
        assert_eq!(package.source_version,"0.1.99");assert!(!package.sections.contains_key("workspace"));assert!(!package.sections.contains_key("scripts"));assert!(!package.sections.contains_key("plugins"));
        let draft=&package.sections["library"].data["requests"][0]["draft"];assert_eq!(draft["url"],"{{baseUrl}}/search?token=");assert_eq!(draft["headers"][0]["name"],"X-A");assert_eq!(draft["headers"][2]["name"],"X-A");assert_eq!(draft["headers"][0]["disabled"],true);
        let prefs:BTreeMap<String,String>=serde_json::from_value(package.sections["preferences"].data.clone()).unwrap();assert!(!prefs.contains_key("http-capture.request-modes"));assert!(!prefs.contains_key("http-capture.grpc.proto.v1"));assert!(!prefs.contains_key("http-capture.clienthello-templates.v1"));
        assert!(package.warnings.iter().any(|w|w.contains("补填")));assert_eq!(package.sections["mcp"].data["clients"].as_array().unwrap().len(),1);
    }
    #[test]
    fn selected_categories_import_atomically_remap_references_disable_execution_and_survive_restart(){
        let src=tempfile::tempdir().unwrap();let source=store(src.path());populate(&source,"Source");let document=export(&source,preferences(),all_options(),"0.1.99").unwrap().document;
        let dir=tempfile::tempdir().unwrap();let target=store(dir.path());populate(&target,"Target");
        let plan=preview(&target,&document).unwrap();let categories=plan.sections.iter().map(|s|s.id.clone()).collect::<Vec<_>>();
        let prepared=prepare_import(&target,&document,&categories,&plan.expected_revision,BTreeMap::from([("http-capture.theme".into(),"sakura".into())]),"0.1.98").unwrap();let imported=commit_import(&target,prepared).unwrap();
        assert!(imported.requires_restart&&target.configuration_frozen());let library=target.library().unwrap();let workspace=target.workspace().unwrap();assert_eq!(library.revision,2);assert_eq!(workspace.revision,2);assert_eq!(library.collections[0].name,"Source");
        let proxy:Vec<Value>=target.setting("upstream.profiles.v1").unwrap().unwrap();assert_ne!(proxy[0]["id"],"proxy-old");assert_eq!(proxy[0]["username"],"private-user");assert_eq!(library.requests[0].draft.upstream_profile_id.as_deref(),proxy[0]["id"].as_str());
        assert_ne!(workspace.editors[0].id,"editor-old");assert!(workspace.editors[0].selected.is_none()&&workspace.editors[0].parent_id.is_none());assert_eq!(workspace.editors[0].saved_request_id.as_deref(),Some("request"));
        let modes:BTreeMap<String,String>=serde_json::from_str(&imported.preferences["http-capture.request-modes"]).unwrap();assert_eq!(modes.len(),1);assert_eq!(modes[&workspace.editors[0].id],"grpc");
        assert_eq!(library.requests[0].draft.body_base64.as_str(),"AAH+/w==");assert_eq!(library.requests[0].draft.headers[0].value,"one");assert_eq!(library.requests[0].draft.headers[2].value,"three");assert!(library.requests[0].draft.headers[2].disabled);assert!(!library.requests[0].draft.scripts.enabled);
        for key in ["capture-scripts.v1","blocking.v1"]{assert_eq!(target.setting::<Value>(key).unwrap().unwrap()["enabled"],false);}
        assert_eq!(target.setting::<Value>("plugins.v1").unwrap().unwrap()[0]["enabled"],false);let mcp=target.setting::<Value>("mcp.server.v1").unwrap().unwrap();for(key,value)in mcp.as_object().unwrap(){if key!="port"{assert_eq!(value,false);}}
        assert_eq!(target.setting::<String>("mcp.server.token.v1").unwrap().unwrap(),"Target-local-only-token");
        assert!(target.save_setting("capture-scripts.v1",&json!({})).is_err());assert!(target.save_library(library.clone()).is_err());assert!(target.save_workspace(workspace.clone()).is_err());assert!(target.mcp_change_workspace(Some(2),None,|_,_|Ok(())).is_err());
        let restore=recovery(&target).unwrap();let restore=parse_document(&restore.document).unwrap();assert_eq!(restore.sections["library"].data["collections"][0]["name"],"Target");assert_eq!(restore.sections["preferences"].data["http-capture.theme"],"sakura");assert!(serde_json::to_string(&restore).unwrap().contains("private-auth"));
        let pending=status(&target).unwrap();assert_eq!(pending["epoch"],imported.epoch);assert_eq!(pending["preferences"]["http-capture.theme"],"matrix");assert_eq!(pending["frozen"],true);
        drop(target);let engine=crate::Engine::open(dir.path()).unwrap();assert!(!engine.store.configuration_frozen());assert!(!engine.scripts.snapshot().0.enabled);assert!(!engine.blocking.snapshot().enabled);assert!(!engine.intercept.snapshot().config.request);assert!(!engine.plugins.list()[0].enabled);assert!(engine.upstream.list_profiles()[0].needs_password);assert!(!engine.upstream.list_profiles()[0].remember_password);
        let pending=status(&engine.store).unwrap();assert_eq!(pending["epoch"],imported.epoch);assert_eq!(pending["preferences"]["http-capture.theme"],"matrix");assert_eq!(pending["frozen"],false); // Response loss cannot lose the preference projection.
        engine.store.save_library(engine.store.library().unwrap()).unwrap();
    }
    #[test]
    fn preview_rejects_malformed_unknown_duplicate_and_oversized_files_without_writing(){
        let dir=tempfile::tempdir().unwrap();let target=store(dir.path());let before=target.configuration_snapshot().unwrap().revision;
        for document in ["{\"format\":\"a\",\"format\":\"b\"}","{\"__proto__\":{}}","{broken}"]{assert!(preview(&target,document).is_err());}
        let valid=export(&target,BTreeMap::new(),ExportOptions::default(),"0.1.99").unwrap().document;
        for mutate in [0,1,2,3] {let mut value:Value=serde_json::from_str(&valid).unwrap();match mutate{0=>value["schemaVersion"]=json!(99),1=>value["sections"]["privateKey"]=json!({"version":1,"data":"forbidden"}),2=>value["sections"]["blocking"]["data"]=json!({"enabled":true,"scope":"capture","rules":[{"id":"a","enabled":true,"matchType":"urlRegex","pattern":"["}]}),_=>value["sections"]["preferences"]["data"]=json!({"http-capture.updater.skippedVersion":"hide-new-version"})};assert!(preview(&target,&value.to_string()).is_err());}
        assert!(preview(&target,&" ".repeat(MAX_DOCUMENT_BYTES+1)).is_err());assert_eq!(target.configuration_snapshot().unwrap().revision,before);assert!(!target.configuration_frozen());assert!(recovery(&target).is_err());
    }
    #[test]
    fn concurrent_edits_and_write_failure_leave_every_category_and_recovery_unchanged(){
        let dir=tempfile::tempdir().unwrap();let target=store(dir.path());populate(&target,"Target");let document=export(&target,preferences(),all_options(),"0.1.99").unwrap().document;
        let plan=preview(&target,&document).unwrap();let prepared=prepare_import(&target,&document,&["preferences".into()],&plan.expected_revision,BTreeMap::new(),"0.1.99").unwrap();
        target.save_setting("decryption.bypass.v1",&vec!["changed.invalid"]).unwrap();let before=target.configuration_snapshot().unwrap().revision;
        assert!(commit_import(&target,prepared).is_err());assert_eq!(target.configuration_snapshot().unwrap().revision,before);assert!(recovery(&target).is_err());assert!(!target.configuration_frozen());
        let plan=preview(&target,&document).unwrap();let prepared=prepare_import(&target,&document,&["preferences".into()],&plan.expected_revision,BTreeMap::new(),"0.1.99").unwrap();
        rusqlite::Connection::open(dir.path().join("sessions.db")).unwrap().execute_batch("CREATE TRIGGER reject_preferences BEFORE INSERT ON settings WHEN new.key='ui.preferences.v1' BEGIN SELECT RAISE(ABORT,'injected write failure'); END;").unwrap();
        assert!(commit_import(&target,prepared).is_err());assert_eq!(target.configuration_snapshot().unwrap().revision,before);assert!(recovery(&target).is_err());assert!(!target.configuration_frozen());
    }
    #[test]
    fn omitted_categories_are_retained_and_mcp_permissions_are_always_reset(){
        let dir=tempfile::tempdir().unwrap();let target=store(dir.path());populate(&target,"Target");let document=export(&target,preferences(),all_options(),"0.1.99").unwrap().document;let before=serde_json::to_value(target.library().unwrap()).unwrap();
        let plan=preview(&target,&document).unwrap();let prepared=prepare_import(&target,&document,&["preferences".into()],&plan.expected_revision,BTreeMap::new(),"0.1.99").unwrap();let result=commit_import(&target,prepared).unwrap();
        assert_eq!(serde_json::to_value(target.library().unwrap()).unwrap(),before);assert!(!result.preferences.contains_key("http-capture.request-modes"));assert_eq!(target.setting::<Value>("mcp.server.v1").unwrap().unwrap()["allowSend"],false);assert_eq!(target.setting::<String>("mcp.server.token.v1").unwrap().unwrap(),"Target-local-only-token");
    }
    #[test]
    fn old_database_connections_cannot_overwrite_import_but_new_connections_can(){
        let dir=tempfile::tempdir().unwrap();let target=store(dir.path());let older=store(dir.path());let document=export(&target,BTreeMap::new(),ExportOptions::default(),"0.1.99").unwrap().document;let plan=preview(&target,&document).unwrap();
        commit_import(&target,prepare_import(&target,&document,&["preferences".into()],&plan.expected_revision,BTreeMap::new(),"0.1.99").unwrap()).unwrap();
        assert!(older.save_setting("blocking.v1",&json!({})).is_err());drop(target);let reopened=store(dir.path());reopened.save_setting("future.option",&json!(true)).unwrap();assert!(older.save_setting("future.option",&json!(false)).is_err());
    }
    #[test]
    fn a_writer_waiting_for_sqlite_lock_checks_import_epoch_after_the_lock_is_acquired(){
        let dir=tempfile::tempdir().unwrap();let old=store(dir.path());let importer=rusqlite::Connection::open(dir.path().join("sessions.db")).unwrap();
        importer.execute_batch("BEGIN IMMEDIATE;").unwrap();
        let (started,waiting)=std::sync::mpsc::channel();let writer=std::thread::spawn(move||{started.send(()).unwrap();old.save_setting("ui.preferences.v1",&json!({"http-capture.theme":"old-window"}))});
        waiting.recv().unwrap();std::thread::sleep(std::time::Duration::from_millis(100));assert!(!writer.is_finished());
        importer.execute("INSERT INTO settings(key,value) VALUES('config-migration.epoch.v1',?1)",[uuid::Uuid::new_v4().to_string()]).unwrap();
        importer.execute("INSERT INTO settings(key,value) VALUES('ui.preferences.v1',?1)",[r#"{"http-capture.theme":"new-import"}"#]).unwrap();importer.execute_batch("COMMIT;").unwrap();
        assert!(writer.join().unwrap().is_err());let current=store(dir.path());assert_eq!(current.setting::<Value>(PREF_KEY).unwrap().unwrap()["http-capture.theme"],"new-import");
    }
    #[test]
    fn proxy_definitions_move_from_legacy_file_to_sqlite_once_without_replacing_identity(){
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("upstream-profiles.json");
        let original=json!([{"id":"legacy-id","name":"Legacy","url":"http://127.0.0.1:7897/","username":"","authEnabled":false,"rememberPassword":false}]);std::fs::write(&path,original.to_string()).unwrap();
        let target=std::sync::Arc::new(store(dir.path()));let state=crate::upstream::UpstreamState::open_with_store(dir.path(),target.clone()).unwrap();assert_eq!(state.list_profiles()[0].id,"legacy-id");
        state.save_profile(crate::upstream::ProfileInput{id:Some("legacy-id".into()),name:"Updated".into(),remember_password:false,config:crate::upstream::UpstreamInput{enabled:false,url:"http://127.0.0.1:7897/".into(),username:String::new(),auth_enabled:false,password:None}}).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(&path).unwrap()).unwrap(),original);drop(state);
        let reloaded=crate::upstream::UpstreamState::open_with_store(dir.path(),target).unwrap();assert_eq!(reloaded.list_profiles()[0].id,"legacy-id");assert_eq!(reloaded.list_profiles()[0].name,"Updated");
    }
    #[test]
    fn atomic_file_save_replaces_only_complete_valid_documents_and_cleans_failed_temporary_files(){
        let dir=tempfile::tempdir().unwrap();let target=store(dir.path());let document=export(&target,BTreeMap::new(),ExportOptions::default(),"0.1.99").unwrap().document;
        let destination=dir.path().join("config.json");std::fs::write(&destination,"old").unwrap();assert!(save_atomic(&destination,"{broken}").is_err());assert_eq!(std::fs::read_to_string(&destination).unwrap(),"old");save_atomic(&destination,&document).unwrap();assert_eq!(std::fs::read_to_string(&destination).unwrap(),document);
        let invalid=dir.path().join("directory");std::fs::create_dir(&invalid).unwrap();let before=std::fs::read_dir(dir.path()).unwrap().count();assert!(save_atomic(&invalid,&document).is_err());assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(),before);assert!(std::fs::read_dir(invalid).unwrap().next().is_none());
    }
}
