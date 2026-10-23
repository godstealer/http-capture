//! Deterministic merge planning for portable collections. Source identities
//! are namespaced by the export ID so a second import can find its first copy.
use super::{remap_proxy,strict_json,validate_library,validate_mcp,validate_preferences,validate_proxies,validate_script,validate_workspace,MergeConflict,PortableMcp,PortableProxy,Template};
use crate::{intercept,plugins::Plugin,scripts::Scripts,store::{Collection,Environment,Library,SavedScript,Workspace}};
use anyhow::{ensure,Result};
use serde::Serialize;
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use std::collections::{BTreeMap,BTreeSet};
use uuid::Uuid;

pub(super) struct LibraryMerge {
    pub library:Library,
    pub ids:BTreeMap<String,String>,
    pub added:usize,
    pub skipped:usize,
    pub conflicts:Vec<String>,
    pub decisions:Vec<MergeConflict>,
}
pub(super) struct ProxyMerge {
    pub profiles:Vec<Value>,
    pub ids:BTreeMap<String,String>,
    pub added:usize,
    pub skipped:usize,
    pub conflicts:Vec<String>,
    pub decisions:Vec<MergeConflict>,
}
pub(super) struct WorkspaceMerge {
    pub workspace:Workspace,
    pub ids:BTreeMap<String,String>,
    pub added:usize,
    pub skipped:usize,
    pub conflicts:Vec<String>,
    pub decisions:Vec<MergeConflict>,
}
pub(super) struct PreferencesMerge {
    pub preferences:BTreeMap<String,String>,
    pub added:usize,
    pub skipped:usize,
    pub conflicts:Vec<String>,
    pub decisions:Vec<MergeConflict>,
}
pub(super) struct DecryptionMerge {
    pub hosts:Vec<String>,
    pub added:usize,
    pub skipped:usize,
}
pub(super) struct McpMerge {
    pub clients:Vec<Value>,
    pub added:usize,
    pub skipped:usize,
    pub conflicts:Vec<String>,
    pub decisions:Vec<MergeConflict>,
}
pub(super) struct RuleMerge {
    pub config:Value,
    pub added:usize,
    pub skipped:usize,
    pub conflicts:Vec<String>,
    pub decisions:Vec<MergeConflict>,
}
pub(super) struct PluginMerge {
    pub plugins:Vec<Plugin>,
    pub added:usize,
    pub skipped:usize,
    pub conflicts:Vec<String>,
    pub decisions:Vec<MergeConflict>,
}
pub(super) struct InterceptionMerge {
    pub config:intercept::Config,
    pub added:usize,
    pub skipped:usize,
    pub conflicts:Vec<String>,
    pub decisions:Vec<MergeConflict>,
}
pub(super) struct ScriptMerge {
    pub scripts:Scripts,
    pub added:usize,
    pub skipped:usize,
    pub conflicts:Vec<String>,
    pub decisions:Vec<MergeConflict>,
}

fn stable_id(export_id:&str,kind:&str,source_id:&str)->String {
    let hash=Sha256::digest(format!("http-capture/config-migration/{export_id}/{kind}/{source_id}").as_bytes());
    let mut bytes=[0u8;16];bytes.copy_from_slice(&hash[..16]);
    bytes[6]=(bytes[6]&0x0f)|0x80;bytes[8]=(bytes[8]&0x3f)|0x80;
    Uuid::from_bytes(bytes).to_string()
}
fn already_used(library:&Library,id:&str)->bool {
    library.collections.iter().any(|item|item.id==id)
        ||library.requests.iter().any(|item|item.id==id)
        ||library.environments.iter().any(|item|item.id==id)
        ||library.scripts.iter().any(|item|item.id==id)
}
fn unique_name(base:&str,existing:impl Iterator<Item=String>,max_bytes:usize)->String {
    let taken=existing.collect::<BTreeSet<_>>();
    if !taken.contains(base){return base.into();}
    let mut stem=base.to_owned();
    while stem.len()>max_bytes.saturating_sub(50){stem.pop();}
    for index in 1..=10000 {
        let suffix=if index==1{"（导入）".to_owned()}else{format!("（导入 {index}）")};
        let name=format!("{stem}{suffix}");
        if !taken.contains(&name){return name;}
    }
    format!("{stem}（导入 {}）",Uuid::new_v4())
}
fn same_content<T:Serialize>(left:&T,right:&T)->Result<bool>{Ok(serde_json::to_value(left)?==serde_json::to_value(right)?)}

pub(super) fn merge_preferences(mut local:BTreeMap<String,String>,source:&BTreeMap<String,String>,editor_ids:&BTreeMap<String,String>,choices:&BTreeMap<String,String>)->Result<PreferencesMerge>{
    let mut added=0;let mut skipped=0;let mut conflicts=Vec::new();let mut decisions=Vec::new();
    for (key,value) in source {
        match key.as_str(){
            "http-capture.request-modes"=>{
                let modes:BTreeMap<String,String>=serde_json::from_value(strict_json(value)?)?;
                if editor_ids.is_empty(){if !modes.is_empty(){conflicts.push("请求标签模式未合并：请同时选择请求标签类别".into());}continue;}
                let mut current:BTreeMap<String,String>=local.get(key).map(|raw|strict_json(raw).and_then(|value|serde_json::from_value(value).map_err(Into::into))).transpose()?.unwrap_or_default();
                for (source_id,mode) in modes {
                    let Some(target_id)=editor_ids.get(&source_id) else{continue};
                    match current.get(target_id){
                        Some(existing) if existing==&mode=>skipped+=1,
                        Some(_)=>{
                            let id=format!("preferences.request-mode.{:x}",Sha256::digest(target_id.as_bytes()));
                            decisions.push(MergeConflict{id:id.clone(),section:"preferences".into(),label:format!("请求标签“{source_id}”的模式")});
                            if choices.get(&id).is_some_and(|choice|choice=="imported"){
                                current.insert(target_id.clone(),mode);added+=1;
                                conflicts.push(format!("请求标签“{source_id}”的模式与本机不同，已选择导入值"));
                            }else{skipped+=1;conflicts.push(format!("请求标签“{source_id}”的模式与本机不同，保留本机值"));}
                        },
                        None=>{current.insert(target_id.clone(),mode);added+=1;}
                    }
                }
                if !current.is_empty(){local.insert(key.clone(),serde_json::to_string(&current)?);}
            }
            "http-capture.grpc.proto.v1"=>{
                let sources:Vec<String>=serde_json::from_value(strict_json(value)?)?;
                let mut current:Vec<String>=local.get(key).map(|raw|strict_json(raw).and_then(|value|serde_json::from_value(value).map_err(Into::into))).transpose()?.unwrap_or_default();
                for source in sources {if current.contains(&source){skipped+=1}else{current.push(source);added+=1}}
                if !current.is_empty(){local.insert(key.clone(),serde_json::to_string(&current)?);}
            }
            "http-capture.clienthello-templates.v1"=>{
                let sources:Vec<Template>=serde_json::from_value(strict_json(value)?)?;
                let mut current:Vec<Template>=local.get(key).map(|raw|strict_json(raw).and_then(|value|serde_json::from_value(value).map_err(Into::into))).transpose()?.unwrap_or_default();
                for mut source in sources {
                    if current.iter().any(|item|item.hex.eq_ignore_ascii_case(&source.hex)){skipped+=1;continue;}
                    source.name=unique_name(&source.name,current.iter().map(|item|item.name.clone()),128);
                    current.push(source);added+=1;
                }
                if !current.is_empty(){local.insert(key.clone(),serde_json::to_string(&current)?);}
            }
            _=>match local.get(key){
                Some(existing) if existing==value=>skipped+=1,
                Some(_)=>{
                    let id=format!("preferences.value.{:x}",Sha256::digest(key.as_bytes()));
                    decisions.push(MergeConflict{id:id.clone(),section:"preferences".into(),label:format!("界面偏好“{key}”")});
                    if choices.get(&id).is_some_and(|choice|choice=="imported"){
                        local.insert(key.clone(),value.clone());added+=1;
                        conflicts.push(format!("偏好“{key}”与本机不同，已选择导入值"));
                    }else{skipped+=1;conflicts.push(format!("偏好“{key}”与本机不同，保留本机值"));}
                },
                None=>{local.insert(key.clone(),value.clone());added+=1;}
            },
        }
    }
    validate_preferences(&local)?;
    Ok(PreferencesMerge{preferences:local,added,skipped,conflicts,decisions})
}
pub(super) fn merge_decryption(mut local:Vec<String>,source:&[String])->Result<DecryptionMerge>{
    let mut added=0;let mut skipped=0;
    for host in source {
        if local.iter().any(|current|current.eq_ignore_ascii_case(host)){skipped+=1;}else{local.push(host.clone());added+=1;}
    }
    super::validate_section("decryption",&json!(local))?;
    Ok(DecryptionMerge{hosts:local,added,skipped})
}

pub(super) fn merge_mcp(mut local:Vec<Value>,source:&PortableMcp,export_id:&str,choices:&BTreeMap<String,String>)->Result<McpMerge>{
    validate_mcp(source)?;
    let mut result=McpMerge{clients:Vec::new(),added:0,skipped:0,conflicts:Vec::new(),decisions:Vec::new()};
    for incoming in &source.clients {
        let candidate=stable_id(export_id,"mcp-client",&incoming.id);
        if let Some(index)=local.iter().position(|item|item["id"]==candidate) {
            let expected_name=unique_name(&incoming.name,local.iter().filter(|item|item["id"]!=candidate).filter_map(|item|item["name"].as_str().map(str::to_owned)),160);
            let current=&local[index];
            if current["name"]!=expected_name||current["url"]!=incoming.url||current["tokenRequired"]!=incoming.token_required
                ||current["transport"]!="http"||current["command"]!=""||current["args"]!=json!([]) {
                let id=format!("mcp.{candidate}");
                result.decisions.push(MergeConflict{id:id.clone(),section:"mcp".into(),label:format!("MCP 服务“{}”",incoming.name)});
                if choices.get(&id).is_some_and(|choice|choice=="imported"){
                    local[index]=json!({"id":candidate,"name":expected_name,"transport":"http","url":incoming.url,"command":"","args":[],"tokenRequired":incoming.token_required});
                    result.added+=1;result.conflicts.push(format!("MCP 服务“{}”在本机已修改，已选择导入定义；Token 与本机命令未迁移",incoming.name));
                }else{result.skipped+=1;result.conflicts.push(format!("MCP 服务“{}”在本机已修改，保留本机版本",incoming.name));}
            }else{result.skipped+=1;}
            continue;
        }
        if local.iter().any(|item|item["transport"]=="http"&&item["url"]==incoming.url) {result.skipped+=1;continue;}
        let name=unique_name(&incoming.name,local.iter().filter_map(|item|item["name"].as_str().map(str::to_owned)),160);
        local.push(json!({"id":candidate,"name":name,"transport":"http","url":incoming.url,"command":"","args":[],"tokenRequired":incoming.token_required}));
        result.added+=1;
    }
    ensure!(local.len()<=32,"At most 32 MCP clients can be imported");
    let mut ids=BTreeSet::new();
    ensure!(local.iter().all(|item|item["id"].as_str().is_some_and(|id|ids.insert(id.to_owned()))),"Duplicate merged MCP client ID");
    result.clients=local;
    Ok(result)
}

fn rule_content(value:&Value)->Value {
    let mut value=value.clone();
    if let Some(rule)=value.as_object_mut(){rule.remove("id");rule.remove("enabled");}
    value
}
fn plugin_content(value:&Plugin)->Result<Value>{
    let mut value=serde_json::to_value(value)?;
    let item=value.as_object_mut().expect("serialized plugin object");
    item.remove("id");item.remove("enabled");
    Ok(value)
}
pub(super) fn merge_plugins(mut local:Vec<Plugin>,source:&[Plugin],export_id:&str,choices:&BTreeMap<String,String>)->Result<PluginMerge>{
    let mut result=PluginMerge{plugins:Vec::new(),added:0,skipped:0,conflicts:Vec::new(),decisions:Vec::new()};
    for incoming in source {
        let candidate=stable_id(export_id,"plugin",&incoming.id);
        let mut item=incoming.clone();item.id=candidate.clone();item.enabled=false;
        if let Some(index)=local.iter().position(|plugin|plugin.id==candidate){
            item.name=unique_name(&item.name,local.iter().filter(|plugin|plugin.id!=candidate).map(|plugin|plugin.name.clone()),128);
            if plugin_content(&local[index])?!=plugin_content(&item)?{
                let id=format!("plugins.{candidate}");
                result.decisions.push(MergeConflict{id:id.clone(),section:"plugins".into(),label:format!("插件“{}”",incoming.name)});
                if choices.get(&id).is_some_and(|choice|choice=="imported"){
                    local[index]=item;result.added+=1;
                    result.conflicts.push(format!("插件“{}”在本机已修改，已选择导入版本；插件保持禁用",incoming.name));
                }else{result.skipped+=1;result.conflicts.push(format!("插件“{}”在本机已修改，保留本机版本",incoming.name));}
            }else{result.skipped+=1;}
            continue;
        }
        let incoming_content=plugin_content(&item)?;
        if local.iter().any(|plugin|plugin_content(plugin).is_ok_and(|value|value==incoming_content)){result.skipped+=1;continue;}
        item.name=unique_name(&item.name,local.iter().map(|plugin|plugin.name.clone()),128);
        local.push(item);result.added+=1;
    }
    super::validate_section("plugins",&serde_json::to_value(&local)?)?;
    result.plugins=local;
    Ok(result)
}
fn interception_content(rule:&intercept::Rule)->Result<Value>{
    let mut value=serde_json::to_value(rule)?;
    let object=value.as_object_mut().expect("serialized breakpoint rule");
    object.remove("disabled");object.remove("migrationId");
    Ok(value)
}
pub(super) fn merge_interception(mut local:intercept::Config,source:&intercept::Config,export_id:&str,choices:&BTreeMap<String,String>)->Result<InterceptionMerge>{
    intercept::validate_config(&local)?;intercept::validate_config(source)?;
    let mut result=InterceptionMerge{config:intercept::Config::default(),added:0,skipped:0,conflicts:Vec::new(),decisions:Vec::new()};
    if local.scope!=source.scope{result.conflicts.push("断点的来源范围与本机不同；保留本机范围，新增规则默认禁用，请启用前检查适用范围".into());}
    for (index,incoming) in source.rules.iter().enumerate() {
        let content=interception_content(incoming)?;
        let candidate=stable_id(export_id,"interception-rule",&index.to_string());
        if let Some(current)=local.rules.iter_mut().find(|rule|rule.migration_id==candidate){
            if interception_content(current)?==content{result.skipped+=1;continue;}
            let id=format!("interception.{candidate}");
            result.decisions.push(MergeConflict{id:id.clone(),section:"interception".into(),label:format!("断点规则 {}",index+1)});
            if choices.get(&id).is_some_and(|choice|choice=="imported"){
                let mut rule=incoming.clone();rule.migration_id=candidate;rule.disabled=true;
                *current=rule;result.added+=1;
                result.conflicts.push(format!("断点规则 {} 在本机已修改，已选择导入规则并保持禁用",index+1));
            } else {
                result.skipped+=1;
                result.conflicts.push(format!("断点规则 {} 在本机已修改，保留本机规则",index+1));
            }
            continue;
        }
        if local.rules.iter().any(|rule|interception_content(rule).is_ok_and(|value|value==content)){result.skipped+=1;continue;}
        let mut rule=incoming.clone();rule.migration_id=candidate;rule.disabled=true;local.rules.push(rule);result.added+=1;
    }
    intercept::validate_config(&local)?;
    result.config=local;
    Ok(result)
}
pub(super) fn merge_scripts(mut local:Scripts,source:&Scripts,choices:&BTreeMap<String,String>)->Result<ScriptMerge>{
    validate_script(&local)?;validate_script(source)?;
    let mut added=0;let mut skipped=0;let mut conflicts=Vec::new();let mut decisions=Vec::new();
    for (name,current,incoming) in [("before",&mut local.before,&source.before),("after",&mut local.after,&source.after)] {
        if incoming.is_empty(){continue;}
        if current==incoming {skipped+=1;}
        else if current.is_empty(){*current=incoming.clone();added+=1;}
        else {
            let id=format!("scripts.{name}");decisions.push(MergeConflict{id:id.clone(),section:"scripts".into(),label:format!("全局脚本 {name} 代码")});
            if choices.get(&id).is_some_and(|choice|choice=="imported"){*current=incoming.clone();added+=1;conflicts.push(format!("全局脚本 {name} 在本机已修改，已选择导入代码"));}
            else{conflicts.push(format!("全局脚本 {name} 在本机已修改，保留本机代码"));skipped+=1;}
        }
    }
    for (key,value) in &source.variables {
        match local.variables.get(key){
            Some(current) if current==value=>skipped+=1,
            Some(_)=>{
                let id=format!("scripts.variable.{:x}",Sha256::digest(key.as_bytes()));
                decisions.push(MergeConflict{id:id.clone(),section:"scripts".into(),label:format!("全局脚本变量“{}”",key.chars().take(128).collect::<String>())});
                if choices.get(&id).is_some_and(|choice|choice=="imported"){local.variables.insert(key.clone(),value.clone());added+=1;conflicts.push(format!("全局脚本变量“{key}”在本机已修改，已选择导入值"));}
                else{skipped+=1;conflicts.push(format!("全局脚本变量“{key}”在本机已修改，保留本机值"));}
            },
            None=>{local.variables.insert(key.clone(),value.clone());added+=1;}
        }
    }
    if !same_content(&local.modules,&source.modules)? {
        let id="scripts.modules".to_owned();decisions.push(MergeConflict{id:id.clone(),section:"scripts".into(),label:"全局脚本模块设置".into()});
        if choices.get(&id).is_some_and(|choice|choice=="imported"){local.modules=source.modules.clone();added+=1;conflicts.push("全局脚本模块设置在本机已修改，已选择导入设置".into());}
        else{conflicts.push("全局脚本模块设置在本机已修改，保留本机设置；启用前检查导入脚本所需模块".into());skipped+=1;}
    }
    if added>0{local.enabled=false;}
    validate_script(&local)?;
    Ok(ScriptMerge{scripts:local,added,skipped,conflicts,decisions})
}
pub(super) fn merge_id_rules(mut local:Value,source:&Value,export_id:&str,section:&str,choices:&BTreeMap<String,String>)->Result<RuleMerge>{
    ensure!(matches!(section,"blocking"|"faults"|"responseRewrite"),"Unsupported rule category");
    super::validate_section(section,&local)?;super::validate_section(section,source)?;
    let mut result=RuleMerge{config:Value::Null,added:0,skipped:0,conflicts:Vec::new(),decisions:Vec::new()};
    if local["scope"]!=source["scope"] {
        result.conflicts.push(format!("{section} 的来源范围与本机不同；保留本机范围，新增规则默认禁用，请启用前检查适用范围"));
    }
    let incoming=source["rules"].as_array().expect("validated rule list");
    for source_rule in incoming {
        let source_id=source_rule["id"].as_str().expect("validated rule ID");
        let candidate=stable_id(export_id,section,source_id);
        let mut item=source_rule.clone();item["id"]=json!(candidate);item["enabled"]=json!(false);
        let rules=local["rules"].as_array_mut().expect("validated rule list");
        if let Some(index)=rules.iter().position(|rule|rule["id"]==candidate) {
            if section=="responseRewrite" {
                let source_name=item["name"].as_str().unwrap_or_default();
                if !source_name.is_empty(){
                    item["name"]=json!(unique_name(source_name,rules.iter().filter(|rule|rule["id"]!=candidate).filter_map(|rule|rule["name"].as_str().map(str::to_owned)),128));
                }
            }
            if rule_content(&rules[index])!=rule_content(&item){
                let id=format!("{section}.{candidate}");
                result.decisions.push(MergeConflict{id:id.clone(),section:section.into(),label:format!("{section} 规则“{source_id}”")});
                if choices.get(&id).is_some_and(|choice|choice=="imported"){
                    rules[index]=item;result.added+=1;
                    result.conflicts.push(format!("{section} 规则“{source_id}”在本机已修改，已选择导入版本；规则保持禁用"));
                }else{result.skipped+=1;result.conflicts.push(format!("{section} 规则“{source_id}”在本机已修改，保留本机版本"));}
            }else{result.skipped+=1;}
            continue;
        }
        if rules.iter().any(|rule|rule_content(rule)==rule_content(&item)){result.skipped+=1;continue;}
        if section=="responseRewrite" {
            let name=item["name"].as_str().unwrap_or_default();
            if !name.is_empty(){item["name"]=json!(unique_name(name,rules.iter().filter_map(|rule|rule["name"].as_str().map(str::to_owned)),128));}
        }
        rules.push(item);result.added+=1;
    }
    super::validate_section(section,&local)?;
    result.config=local;
    Ok(result)
}

fn portable_profile(value:&Value)->PortableProxy {
    PortableProxy{id:value["id"].as_str().unwrap_or_default().into(),name:value["name"].as_str().unwrap_or_default().into(),url:value["url"].as_str().unwrap_or_default().into(),username:value["username"].as_str().unwrap_or_default().into(),auth_enabled:value["authEnabled"].as_bool().unwrap_or(false)}
}
fn same_proxy(left:&PortableProxy,right:&PortableProxy)->bool {
    left.url==right.url&&left.username==right.username&&left.auth_enabled==right.auth_enabled
}
pub(super) fn merge_proxies(mut local:Vec<Value>,source:&[PortableProxy],export_id:&str,choices:&BTreeMap<String,String>)->Result<ProxyMerge>{
    let mut result=ProxyMerge{profiles:Vec::new(),ids:BTreeMap::new(),added:0,skipped:0,conflicts:Vec::new(),decisions:Vec::new()};
    for incoming in source {
        let candidate=stable_id(export_id,"proxy",&incoming.id);
        if let Some(index)=local.iter().position(|item|item["id"]==candidate) {
            let current=portable_profile(&local[index]);result.ids.insert(incoming.id.clone(),candidate.clone());
            if !same_proxy(&current,incoming){
                let id=format!("upstream.{candidate}");
                result.decisions.push(MergeConflict{id:id.clone(),section:"upstream".into(),label:format!("代理“{}”的连接设置",current.name)});
                if choices.get(&id).is_some_and(|choice|choice=="imported"){
                    // An exported profile never carries its password. Do not
                    // reuse a credential saved for the old URL or username.
                    local[index]=json!({"id":candidate,"name":current.name,"url":incoming.url,"username":incoming.username,"authEnabled":incoming.auth_enabled,"rememberPassword":false});
                    result.added+=1;result.conflicts.push(format!("代理“{}”在本机已修改，已选择导入连接设置；需重新填写密码",incoming.name));
                }else{result.skipped+=1;result.conflicts.push(format!("代理“{}”在本机已修改，保留本机版本",incoming.name));}
            }else{result.skipped+=1;}
            continue;
        }
        if let Some(current)=local.iter().find(|item|{
            let current=portable_profile(item);current.name==incoming.name&&same_proxy(&current,incoming)
        }).filter(|_|!incoming.auth_enabled){result.ids.insert(incoming.id.clone(),current["id"].as_str().unwrap().into());result.skipped+=1;continue;}
        ensure!(!local.iter().any(|item|item["id"]==candidate),"Imported proxy ID conflicts with an existing profile");
        let name=unique_name(&incoming.name,local.iter().map(|item|item["name"].as_str().unwrap_or_default().to_owned()),80);
        local.push(json!({"id":candidate,"name":name,"url":incoming.url,"username":incoming.username,"authEnabled":incoming.auth_enabled,"rememberPassword":false}));
        result.ids.insert(incoming.id.clone(),candidate);result.added+=1;
    }
    ensure!(local.len()<=100,"At most 100 upstream proxies can be imported");
    validate_proxies(&local.iter().map(portable_profile).collect::<Vec<_>>())?;
    result.profiles=local;Ok(result)
}

pub(super) fn merge_workspace(mut local:Workspace,source:&Workspace,export_id:&str,request_ids:&BTreeMap<String,String>,proxy_ids:&BTreeMap<String,String>,choices:&BTreeMap<String,String>)->Result<WorkspaceMerge>{
    let mut result=WorkspaceMerge{workspace:Workspace::default(),ids:BTreeMap::new(),added:0,skipped:0,conflicts:Vec::new(),decisions:Vec::new()};
    for incoming in &source.editors {
        let candidate=stable_id(export_id,"editor",&incoming.id);
        let mut item=incoming.clone();item.id=candidate.clone();item.selected=None;item.parent_id=None;
        item.saved_request_id=item.saved_request_id.as_ref().and_then(|id|request_ids.get(id).cloned());
        remap_proxy(&mut item.draft,proxy_ids,true,false);
        if let Some(index)=local.editors.iter().position(|editor|editor.id==candidate){
            result.ids.insert(incoming.id.clone(),candidate.clone());
            if !same_content(&local.editors[index],&item)?{
                let id=format!("workspace.{candidate}");
                result.decisions.push(MergeConflict{id:id.clone(),section:"workspace".into(),label:format!("请求标签“{}”",incoming.id)});
                if choices.get(&id).is_some_and(|choice|choice=="imported"){
                    local.editors[index]=item;result.added+=1;
                    result.conflicts.push(format!("请求标签“{}”在本机已修改，已选择导入版本；抓包记录关联已清除",incoming.id));
                }else{result.skipped+=1;result.conflicts.push(format!("请求标签“{}”在本机已修改，保留本机版本",incoming.id));}
            }else{result.skipped+=1;}
            continue;
        }
        ensure!(!local.editors.iter().any(|editor|editor.id==candidate),"Imported request tab ID conflicts with an existing tab");
        local.editors.push(item);result.ids.insert(incoming.id.clone(),candidate);result.added+=1;
    }
    if local.active_editor.is_empty(){local.active_editor=result.ids.get(&source.active_editor).cloned().unwrap_or_default();}
    validate_workspace(&local)?;result.workspace=local;Ok(result)
}

pub(super) fn merge_library(mut local:Library,source:&Library,export_id:&str,proxy_ids:&BTreeMap<String,String>,choices:&BTreeMap<String,String>)->Result<LibraryMerge>{
    let mut result=LibraryMerge{library:Library::default(),ids:BTreeMap::new(),added:0,skipped:0,conflicts:Vec::new(),decisions:Vec::new()};
    for incoming in &source.collections {
        let candidate=stable_id(export_id,"collection",&incoming.id);
        if let Some(index)=local.collections.iter().position(|item|item.id==candidate) {
            result.ids.insert(incoming.id.clone(),candidate.clone());
            if local.collections[index].name!=incoming.name{
                let id=format!("library.collection.{candidate}");
                result.decisions.push(MergeConflict{id:id.clone(),section:"library".into(),label:format!("集合“{}”的名称",incoming.name)});
                if choices.get(&id).is_some_and(|choice|choice=="imported"){
                    let name=unique_name(&incoming.name,local.collections.iter().enumerate().filter(|(at,_)|*at!=index).map(|(_,item)|item.name.clone()),512);
                    local.collections[index].name=name;
                    result.added+=1;result.conflicts.push(format!("集合“{}”在本机已修改，已选择导入名称",incoming.name));
                }else{result.skipped+=1;result.conflicts.push(format!("集合“{}”在本机已修改，保留本机名称",incoming.name));}
            }else{result.skipped+=1;}
            continue;
        }
        if let Some(current)=local.collections.iter().find(|item|item.name==incoming.name){result.ids.insert(incoming.id.clone(),current.id.clone());result.skipped+=1;continue;}
        ensure!(!already_used(&local,&candidate),"Imported collection ID conflicts with an existing item");
        let item=Collection{id:candidate.clone(),name:unique_name(&incoming.name,local.collections.iter().map(|item|item.name.clone()),512)};
        local.collections.push(item);result.ids.insert(incoming.id.clone(),candidate);result.added+=1;
    }
    for incoming in &source.environments {
        let candidate=stable_id(export_id,"environment",&incoming.id);
        if let Some(index)=local.environments.iter().position(|item|item.id==candidate){
            result.ids.insert(incoming.id.clone(),candidate.clone());
            if !same_content(&local.environments[index].variables,&incoming.variables)?{
                let id=format!("library.environment.{candidate}");
                result.decisions.push(MergeConflict{id:id.clone(),section:"library".into(),label:format!("环境“{}”的变量",incoming.name)});
                if choices.get(&id).is_some_and(|choice|choice=="imported"){
                    local.environments[index].variables=incoming.variables.clone();result.added+=1;
                    result.conflicts.push(format!("环境“{}”在本机已修改，已选择导入变量",incoming.name));
                }else{result.skipped+=1;result.conflicts.push(format!("环境“{}”在本机已修改，保留本机变量",incoming.name));}
            }else{result.skipped+=1;}
            continue;
        }
        if let Some(current)=local.environments.iter().find(|item|item.name==incoming.name&&item.variables==incoming.variables){result.ids.insert(incoming.id.clone(),current.id.clone());result.skipped+=1;continue;}
        ensure!(!already_used(&local,&candidate),"Imported environment ID conflicts with an existing item");
        let item=Environment{id:candidate.clone(),name:unique_name(&incoming.name,local.environments.iter().map(|item|item.name.clone()),512),variables:incoming.variables.clone()};
        local.environments.push(item);result.ids.insert(incoming.id.clone(),candidate);result.added+=1;
    }
    for incoming in &source.scripts {
        let candidate=stable_id(export_id,"script",&incoming.id);
        let mut script=incoming.scripts.clone();script.enabled=false;
        if let Some(index)=local.scripts.iter().position(|item|item.id==candidate){
            result.ids.insert(incoming.id.clone(),candidate.clone());
            if !same_content(&local.scripts[index].scripts,&script)?{
                let id=format!("library.script.{candidate}");
                result.decisions.push(MergeConflict{id:id.clone(),section:"library".into(),label:format!("脚本模板“{}”",incoming.name)});
                if choices.get(&id).is_some_and(|choice|choice=="imported"){
                    local.scripts[index].scripts=script;result.added+=1;
                    result.conflicts.push(format!("脚本模板“{}”在本机已修改，已选择导入版本；模板保持禁用",incoming.name));
                }else{result.skipped+=1;result.conflicts.push(format!("脚本模板“{}”在本机已修改，保留本机版本",incoming.name));}
            }else{result.skipped+=1;}
            continue;
        }
        if let Some(current)=local.scripts.iter().find(|item|item.name==incoming.name&&same_content(&item.scripts,&script).unwrap_or(false)){result.ids.insert(incoming.id.clone(),current.id.clone());result.skipped+=1;continue;}
        ensure!(!already_used(&local,&candidate),"Imported script ID conflicts with an existing item");
        let item=SavedScript{id:candidate.clone(),name:unique_name(&incoming.name,local.scripts.iter().map(|item|item.name.clone()),512),scripts:script};
        local.scripts.push(item);result.ids.insert(incoming.id.clone(),candidate);result.added+=1;
    }
    for incoming in &source.requests {
        let candidate=stable_id(export_id,"request",&incoming.id);
        let mut item=incoming.clone();
        item.collection_id=result.ids.get(&incoming.collection_id).cloned().ok_or_else(||anyhow::anyhow!("Missing imported collection mapping"))?;
        remap_proxy(&mut item.draft,proxy_ids,true,false);
        if let Some(index)=local.requests.iter().position(|existing|existing.id==candidate){
            result.ids.insert(incoming.id.clone(),candidate.clone());
            item.id=candidate.clone();item.name=local.requests[index].name.clone();
            if !same_content(&local.requests[index],&item)?{
                let id=format!("library.request.{candidate}");
                result.decisions.push(MergeConflict{id:id.clone(),section:"library".into(),label:format!("已保存请求“{}”",incoming.name)});
                if choices.get(&id).is_some_and(|choice|choice=="imported"){
                    local.requests[index]=item;result.added+=1;
                    result.conflicts.push(format!("请求“{}”在本机已修改，已选择导入版本",incoming.name));
                }else{result.skipped+=1;result.conflicts.push(format!("请求“{}”在本机已修改，保留本机版本",incoming.name));}
            }else{result.skipped+=1;}
            continue;
        }
        if let Some(current)=local.requests.iter().find(|existing|{
            if existing.name!=item.name||existing.collection_id!=item.collection_id{return false;}
            let mut normalized=item.clone();normalized.id=existing.id.clone();
            same_content(*existing,&normalized).unwrap_or(false)
        }) {result.ids.insert(incoming.id.clone(),current.id.clone());result.skipped+=1;continue;}
        ensure!(!already_used(&local,&candidate),"Imported request ID conflicts with an existing item");
        item.id=candidate.clone();
        item.name=unique_name(&item.name,local.requests.iter().filter(|existing|existing.collection_id==item.collection_id).map(|existing|existing.name.clone()),512);
        local.requests.push(item);result.ids.insert(incoming.id.clone(),candidate);result.added+=1;
    }
    validate_library(&local)?;
    result.library=local;Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn preferences_keep_local_scalars_and_merge_mapped_modes_and_code_lists(){
        let local=BTreeMap::from([
            ("http-capture.theme".into(),"matrix".into()),
            ("http-capture.request-modes".into(),r#"{"local-tab":"http"}"#.into()),
            ("http-capture.grpc.proto.v1".into(),r#"["syntax = \"proto3\";"]"#.into()),
            ("http-capture.clienthello-templates.v1".into(),r#"[{"name":"Preset","hex":"160303000401000000"}]"#.into()),
        ]);
        let source=BTreeMap::from([
            ("http-capture.theme".into(),"paper".into()),
            ("http-capture.zoom".into(),"125".into()),
            ("http-capture.request-modes".into(),r#"{"source-tab":"grpc"}"#.into()),
            ("http-capture.grpc.proto.v1".into(),r#"["syntax = \"proto3\";","syntax = \"proto3\"; message New {}"]"#.into()),
            ("http-capture.clienthello-templates.v1".into(),r#"[{"name":"Preset","hex":"160303000401000000"}]"#.into()),
        ]);
        let ids=BTreeMap::from([("source-tab".into(),"imported-tab".into())]);
        let merged=merge_preferences(local,&source,&ids,&BTreeMap::new()).unwrap();
        assert_eq!(merged.preferences["http-capture.theme"],"matrix");assert_eq!(merged.preferences["http-capture.zoom"],"125");
        let modes:Value=serde_json::from_str(&merged.preferences["http-capture.request-modes"]).unwrap();assert_eq!(modes["local-tab"],"http");assert_eq!(modes["imported-tab"],"grpc");
        assert_eq!(serde_json::from_str::<Vec<String>>(&merged.preferences["http-capture.grpc.proto.v1"]).unwrap().len(),2);
        assert_eq!(serde_json::from_str::<Vec<Value>>(&merged.preferences["http-capture.clienthello-templates.v1"]).unwrap().len(),1);
        assert_eq!(merged.conflicts.len(),1);
        assert_eq!(merged.decisions.len(),1);
        let choice=BTreeMap::from([(merged.decisions[0].id.clone(),"imported".to_owned())]);
        let applied=merge_preferences(merged.preferences,&source,&ids,&choice).unwrap();
        assert_eq!(applied.preferences["http-capture.theme"],"paper");
        assert_eq!(applied.added,1);
    }
    #[test]
    fn preference_request_mode_conflict_can_use_imported_value(){
        let local=BTreeMap::from([("http-capture.request-modes".into(),r#"{"tab":"http"}"#.into())]);
        let source=BTreeMap::from([("http-capture.request-modes".into(),r#"{"source":"grpc"}"#.into())]);
        let ids=BTreeMap::from([("source".into(),"tab".into())]);
        let preview=merge_preferences(local.clone(),&source,&ids,&BTreeMap::new()).unwrap();
        assert_eq!(preview.decisions.len(),1);
        let choice=BTreeMap::from([(preview.decisions[0].id.clone(),"imported".to_owned())]);
        let applied=merge_preferences(local,&source,&ids,&choice).unwrap();
        let modes:Value=serde_json::from_str(&applied.preferences["http-capture.request-modes"]).unwrap();
        assert_eq!(modes["tab"],"grpc");
    }
    #[test]
    fn decryption_hosts_preserve_local_order_and_skip_case_insensitive_duplicates(){
        let local=vec!["first.invalid".to_string(),"*.Example.invalid".to_string()];
        let source=vec!["*.example.invalid".to_string(),"second.invalid".to_string()];
        let merged=merge_decryption(local.clone(),&source).unwrap();
        assert_eq!(merged.hosts,vec![local[0].clone(),local[1].clone(),source[1].clone()]);
        assert_eq!((merged.added,merged.skipped),(1,1));
        assert_eq!(merge_decryption(merged.hosts,&source).unwrap().added,0);
    }
    #[test]
    fn id_rules_keep_local_order_disable_imports_and_detect_modified_copies(){
        let source=json!({"enabled":true,"scope":"all","rules":[{"id":"incoming","enabled":true,"matchType":"urlContains","pattern":"/new"}]});
        let local=json!({"enabled":true,"scope":"capture","rules":[{"id":"local","enabled":true,"matchType":"urlContains","pattern":"/old"}]});
        let first=merge_id_rules(local,&source,"11111111-1111-4111-8111-111111111111","blocking",&BTreeMap::new()).unwrap();
        assert_eq!((first.added,first.skipped),(1,0));assert_eq!(first.config["enabled"],true);assert_eq!(first.config["scope"],"capture");
        assert_eq!(first.config["rules"][0]["id"],"local");assert_eq!(first.config["rules"][1]["enabled"],false);
        assert!(first.conflicts.iter().any(|item|item.contains("范围")));
        let second=merge_id_rules(first.config.clone(),&source,"11111111-1111-4111-8111-111111111111","blocking",&BTreeMap::new()).unwrap();
        assert_eq!((second.added,second.skipped),(0,1));
        let mut edited=second.config;edited["rules"][1]["pattern"]=json!("/edited");
        let third=merge_id_rules(edited,&source,"11111111-1111-4111-8111-111111111111","blocking",&BTreeMap::new()).unwrap();
        assert_eq!(third.added,0);assert!(third.conflicts.iter().any(|item|item.contains("已修改")));assert_eq!(third.decisions.len(),1);
        let selected=BTreeMap::from([(third.decisions[0].id.clone(),"imported".into())]);
        let accepted=merge_id_rules(third.config,&source,"11111111-1111-4111-8111-111111111111","blocking",&selected).unwrap();
        assert_eq!(accepted.added,1);assert_eq!(accepted.config["rules"][1]["pattern"],"/new");assert_eq!(accepted.config["rules"][1]["enabled"],false);
    }
    #[test]
    fn rewrite_merge_respects_names_and_validation_limits(){
        let source=json!({"enabled":true,"scope":"capture","rules":[{"id":"source","name":"Response","enabled":true,"matchType":"urlContains","pattern":"/api","action":"replace","replacement":{"status":200,"headers":[],"bodyBase64":""}}]});
        let local=json!({"enabled":true,"scope":"capture","rules":[{"id":"local","name":"Response","enabled":true,"matchType":"urlContains","pattern":"/old","action":"replace","replacement":{"status":201,"headers":[],"bodyBase64":""}}]});
        let merged=merge_id_rules(local,&source,"11111111-1111-4111-8111-111111111111","responseRewrite",&BTreeMap::new()).unwrap();
        assert_eq!(merged.added,1);assert_eq!(merged.config["rules"][1]["name"],"Response（导入）");assert_eq!(merged.config["rules"][1]["enabled"],false);
        let repeated=merge_id_rules(merged.config,&source,"11111111-1111-4111-8111-111111111111","responseRewrite",&BTreeMap::new()).unwrap();
        assert_eq!((repeated.added,repeated.skipped),(0,1));assert!(repeated.conflicts.is_empty());
    }
    #[test]
    fn fault_merge_preserves_existing_limits_and_disables_new_rule(){
        let local=json!({"enabled":true,"scope":"capture","rules":[{"id":"local","enabled":true,"matchType":"urlContains","pattern":"/slow","delayMs":500}]});
        let source=json!({"enabled":false,"scope":"capture","rules":[{"id":"source","enabled":true,"matchType":"urlContains","pattern":"/limited","rateLimit":{"requests":3,"windowMs":1000}}]});
        let merged=merge_id_rules(local,&source,"11111111-1111-4111-8111-111111111111","faults",&BTreeMap::new()).unwrap();
        assert_eq!(merged.added,1);assert_eq!(merged.config["enabled"],true);
        assert_eq!(merged.config["rules"][0]["delayMs"],500);assert_eq!(merged.config["rules"][1]["rateLimit"]["requests"],3);
        assert_eq!(merged.config["rules"][1]["enabled"],false);
    }
    #[test]
    fn plugin_merge_is_idempotent_and_preserves_local_code(){
        let local:Plugin=serde_json::from_value(json!({"id":"local","name":"Plugin","version":"1.0.0","apiVersion":1,"enabled":true,"scope":"all","urlPattern":".*","before":"local();"})).unwrap();
        let source:Plugin=serde_json::from_value(json!({"id":"source","name":"Plugin","version":"1.0.0","apiVersion":1,"enabled":true,"scope":"all","urlPattern":".*","before":"imported();"})).unwrap();
        let first=merge_plugins(vec![local.clone()],&[source.clone()],"11111111-1111-4111-8111-111111111111",&BTreeMap::new()).unwrap();
        assert_eq!(first.added,1);assert_eq!(first.plugins[0].before,local.before);assert!(first.plugins[0].enabled);
        assert_eq!(first.plugins[1].name,"Plugin（导入）");assert!(!first.plugins[1].enabled);
        let second=merge_plugins(first.plugins,&[source.clone()],"11111111-1111-4111-8111-111111111111",&BTreeMap::new()).unwrap();
        assert_eq!((second.added,second.skipped),(0,1));assert!(second.conflicts.is_empty());
        let mut changed=second.plugins;changed[1].before="edited();".into();
        let third=merge_plugins(changed,&[source.clone()],"11111111-1111-4111-8111-111111111111",&BTreeMap::new()).unwrap();
        assert_eq!(third.added,0);assert!(third.conflicts.iter().any(|item|item.contains("已修改")));
        assert_eq!(third.decisions.len(),1);assert_eq!(third.decisions[0].section,"plugins");
        let selected=BTreeMap::from([(third.decisions[0].id.clone(),"imported".into())]);
        let accepted=merge_plugins(third.plugins,&[source],"11111111-1111-4111-8111-111111111111",&selected).unwrap();
        assert_eq!(accepted.added,1);assert_eq!(accepted.plugins[1].before,"imported();");
        assert!(!accepted.plugins[1].enabled);assert_eq!(accepted.plugins[1].name,"Plugin（导入）");
    }
    #[test]
    fn breakpoint_merge_preserves_switches_and_deduplicates_conditions(){
        let local:intercept::Config=serde_json::from_value(json!({"request":true,"response":true,"scope":"capture","rules":[{"urlContains":"/local"}]})).unwrap();
        let source:intercept::Config=serde_json::from_value(json!({"request":false,"response":false,"scope":"all","rules":[{"urlContains":"/local"},{"urlContains":"/source"}]})).unwrap();
        let export_id="11111111-1111-4111-8111-111111111111";
        let first=merge_interception(local,&source,export_id,&BTreeMap::new()).unwrap();
        assert_eq!((first.added,first.skipped),(1,1));assert!(first.config.request&&first.config.response);assert_eq!(first.config.scope,"capture");
        assert!(!first.config.rules[0].disabled);assert!(first.config.rules[1].disabled);
        assert!(first.conflicts.iter().any(|item|item.contains("范围")));
        let repeated=merge_interception(first.config,&source,export_id,&BTreeMap::new()).unwrap();
        assert_eq!((repeated.added,repeated.skipped),(0,2));
    }
    #[test]
    fn breakpoint_merge_recognizes_modified_import_and_applies_choice_disabled(){
        let export_id="11111111-1111-4111-8111-111111111111";
        let local:intercept::Config=serde_json::from_value(json!({"request":true,"response":false,"scope":"all","rules":[]})).unwrap();
        let source:intercept::Config=serde_json::from_value(json!({"request":false,"response":true,"scope":"all","rules":[{"urlContains":"/source"}]})).unwrap();
        let first=merge_interception(local,&source,export_id,&BTreeMap::new()).unwrap();
        assert_eq!(first.added,1);
        let mut edited=first.config;
        edited.rules[0].url_contains="/edited".into();edited.rules[0].disabled=false;
        let preview=merge_interception(edited.clone(),&source,export_id,&BTreeMap::new()).unwrap();
        assert_eq!((preview.added,preview.skipped,preview.config.rules.len()),(0,1,1));
        assert_eq!(preview.config.rules[0].url_contains,"/edited");
        assert_eq!(preview.decisions.len(),1);
        let choice=BTreeMap::from([(preview.decisions[0].id.clone(),"imported".into())]);
        let accepted=merge_interception(edited,&source,export_id,&choice).unwrap();
        assert_eq!((accepted.added,accepted.config.rules.len()),(1,1));
        assert_eq!(accepted.config.rules[0].url_contains,"/source");
        assert!(accepted.config.rules[0].disabled);
        assert!(accepted.config.request&&!accepted.config.response);
        let again=merge_interception(accepted.config,&source,export_id,&BTreeMap::new()).unwrap();
        assert_eq!((again.added,again.skipped),(0,1));
    }
    #[test]
    fn global_scripts_fill_empty_fields_preserve_conflicts_and_disable_on_change(){
        let local:Scripts=serde_json::from_value(json!({"enabled":true,"before":"local();","variables":{"same":"local","onlyLocal":"keep"}})).unwrap();
        let source:Scripts=serde_json::from_value(json!({"enabled":false,"before":"source();","after":"importedAfter();","variables":{"same":"source","new":"imported"}})).unwrap();
        let first=merge_scripts(local,&source,&BTreeMap::new()).unwrap();
        assert_eq!(first.added,2);assert!(!first.scripts.enabled);assert_eq!(first.scripts.before,"local();");assert_eq!(first.scripts.after,"importedAfter();");
        assert_eq!(first.scripts.variables["same"],"local");assert_eq!(first.scripts.variables["onlyLocal"],"keep");assert_eq!(first.scripts.variables["new"],"imported");
        assert!(first.conflicts.iter().any(|item|item.contains("before")));assert!(first.conflicts.iter().any(|item|item.contains("same")));
        let mut enabled=first.scripts;enabled.enabled=true;
        let repeated=merge_scripts(enabled,&source,&BTreeMap::new()).unwrap();assert_eq!(repeated.added,0);assert!(repeated.scripts.enabled);
        assert_eq!(repeated.decisions.len(),2);
        let selected=BTreeMap::from([("scripts.before".into(),"imported".into())]);
        let applied=merge_scripts(repeated.scripts,&source,&selected).unwrap();
        assert_eq!(applied.scripts.before,"source();");assert!(!applied.scripts.enabled);assert_eq!(applied.added,1);
    }
    #[test]
    fn mcp_imported_name_suffix_is_not_reported_as_a_conflict_on_reimport(){
        let source:PortableMcp=serde_json::from_value(json!({"port":1422,"clients":[{"id":"remote","name":"Service","url":"https://remote.example.invalid/mcp","tokenRequired":true}]})).unwrap();
        let local=vec![json!({"id":"local","name":"Service","transport":"http","url":"https://local.example.invalid/mcp","command":"","args":[],"tokenRequired":false})];
        let first=merge_mcp(local,&source,"11111111-1111-4111-8111-111111111111",&BTreeMap::new()).unwrap();
        assert_eq!(first.added,1);assert_eq!(first.clients[1]["name"],"Service（导入）");
        let second=merge_mcp(first.clients,&source,"11111111-1111-4111-8111-111111111111",&BTreeMap::new()).unwrap();
        assert_eq!((second.added,second.skipped),(0,1));assert!(second.conflicts.is_empty());
        let mut edited=second.clients;
        let imported_id=edited[1]["id"].clone();
        edited[1]["url"]=json!("https://local.example.invalid/mcp");
        edited[1]["token"]=json!("local-secret");
        edited[1]["command"]=json!("local-command");
        let preview=merge_mcp(edited.clone(),&source,"11111111-1111-4111-8111-111111111111",&BTreeMap::new()).unwrap();
        assert_eq!(preview.decisions.len(),1);assert_eq!(preview.decisions[0].section,"mcp");
        assert_eq!(preview.clients[1]["token"],"local-secret");
        let choices=BTreeMap::from([(preview.decisions[0].id.clone(),"imported".to_owned())]);
        let applied=merge_mcp(edited,&source,"11111111-1111-4111-8111-111111111111",&choices).unwrap();
        assert_eq!(applied.added,1);assert_eq!(applied.clients[1]["id"],imported_id);
        assert_eq!(applied.clients[1]["url"],source.clients[0].url);
        assert_eq!(applied.clients[1]["command"],"");
        assert!(applied.clients[1].get("token").is_none());
    }
    #[test]
    fn merge_preserves_local_items_and_is_idempotent_for_same_export(){
        let local:Library=serde_json::from_value(json!({"revision":3,"collections":[{"id":"local","name":"Flights"}],"requests":[],"environments":[{"id":"local-env","name":"Prod","variables":{"host":"local"}}],"scripts":[]})).unwrap();
        let source:Library=serde_json::from_value(json!({"revision":0,"collections":[{"id":"source","name":"Flights"}],"requests":[{"id":"source-request","name":"Search","collectionId":"source","draft":{"method":"GET","url":"https://example.invalid/","headers":[],"bodyBase64":""}}],"environments":[{"id":"source-env","name":"Prod","variables":{"host":"source"}}],"scripts":[]})).unwrap();
        let first=merge_library(local,&source,"11111111-1111-4111-8111-111111111111",&BTreeMap::new(),&BTreeMap::new()).unwrap();
        assert_eq!(first.added,2);assert_eq!(first.library.collections.len(),1);assert_eq!(first.library.environments[0].variables["host"],"local");
        assert_eq!(first.library.environments[1].name,"Prod（导入）");
        assert_eq!(first.library.requests[0].collection_id,"local");
        let second=merge_library(first.library.clone(),&source,"11111111-1111-4111-8111-111111111111",&BTreeMap::new(),&BTreeMap::new()).unwrap();
        assert_eq!(second.added,0);assert_eq!(second.library.requests.len(),1);assert_eq!(second.library.environments.len(),2);assert!(second.conflicts.is_empty());
        let mut changed=second.library;changed.environments[1].variables.insert("host".into(),"changed locally".into());
        let third=merge_library(changed,&source,"11111111-1111-4111-8111-111111111111",&BTreeMap::new(),&BTreeMap::new()).unwrap();
        assert_eq!(third.added,0);assert_eq!(third.library.environments.len(),2);assert_eq!(third.conflicts.len(),1);
        assert_eq!(third.decisions.len(),1);assert_eq!(third.decisions[0].section,"library");
        let choices=BTreeMap::from([(third.decisions[0].id.clone(),"imported".to_owned())]);
        let accepted=merge_library(third.library,&source,"11111111-1111-4111-8111-111111111111",&BTreeMap::new(),&choices).unwrap();
        assert_eq!(accepted.added,1);assert_eq!(accepted.library.environments[1].variables["host"],"source");
    }
    #[test]
    fn library_conflicts_can_restore_each_imported_item_without_changing_ids(){
        let export_id="11111111-1111-4111-8111-111111111111";
        let source:Library=serde_json::from_value(json!({"revision":0,
            "collections":[{"id":"source-collection","name":"Flights"}],
            "environments":[{"id":"source-env","name":"Production","variables":{"host":"source"}}],
            "scripts":[{"id":"source-script","name":"Template","scripts":{"enabled":true,"before":"request.method='POST';"}}],
            "requests":[{"id":"source-request","name":"Search","collectionId":"source-collection","draft":{"method":"GET","url":"https://example.invalid/","headers":[],"bodyBase64":""}}]
        })).unwrap();
        let first=merge_library(Library::default(),&source,export_id,&BTreeMap::new(),&BTreeMap::new()).unwrap();
        let ids=first.ids.clone();let mut changed=first.library;
        changed.collections[0].name="Local flights".into();
        changed.environments[0].variables.insert("host".into(),"local".into());
        changed.scripts[0].scripts.before="request.method='PUT';".into();
        changed.requests[0].draft.url="https://local.invalid/".into();
        let preview=merge_library(changed.clone(),&source,export_id,&BTreeMap::new(),&BTreeMap::new()).unwrap();
        assert_eq!(preview.decisions.len(),4);assert_eq!(preview.added,0);
        let choices=preview.decisions.iter().map(|decision|(decision.id.clone(),"imported".to_owned())).collect();
        let applied=merge_library(changed,&source,export_id,&BTreeMap::new(),&choices).unwrap();
        assert_eq!(applied.added,4);assert_eq!(applied.ids,ids);
        assert_eq!(applied.library.collections[0].name,"Flights");
        assert_eq!(applied.library.environments[0].variables["host"],"source");
        assert_eq!(applied.library.scripts[0].scripts.before,"request.method='POST';");
        assert!(!applied.library.scripts[0].scripts.enabled);
        assert_eq!(applied.library.requests[0].draft.url,"https://example.invalid/");
        assert_eq!(applied.library.requests[0].collection_id,ids["source-collection"]);
    }
    #[test]
    fn proxy_and_workspace_merge_remap_references_without_reusing_same_source_ids(){
        let export_id="11111111-1111-4111-8111-111111111111";
        let profiles=vec![json!({"id":"local-proxy","name":"Local","url":"http://127.0.0.1:7897/","username":"","authEnabled":false,"rememberPassword":true})];
        let incoming=vec![PortableProxy{id:"source-proxy".into(),name:"Local".into(),url:"socks5://127.0.0.1:7897/".into(),username:String::new(),auth_enabled:false}];
        let merged=merge_proxies(profiles,&incoming,export_id,&BTreeMap::new()).unwrap();
        assert_eq!(merged.added,1);assert_eq!(merged.profiles[0]["rememberPassword"],true);assert_eq!(merged.profiles[1]["name"],"Local（导入）");
        let repeated=merge_proxies(merged.profiles.clone(),&incoming,export_id,&BTreeMap::new()).unwrap();assert_eq!(repeated.added,0);assert!(repeated.conflicts.is_empty());
        let source:Workspace=serde_json::from_value(json!({"revision":0,"activeEditor":"source-tab","editors":[{"id":"source-tab","draft":{"method":"GET","url":"https://example.invalid/","headers":[],"bodyBase64":"","upstreamProfileId":"source-proxy"},"selected":"old-flow","parentId":"old-flow","savedRequestId":"source-request"}]})).unwrap();
        let first=merge_workspace(Workspace::default(),&source,export_id,&BTreeMap::from([("source-request".into(),"local-request".into())]),&merged.ids,&BTreeMap::new()).unwrap();
        assert_eq!(first.added,1);assert_eq!(first.workspace.editors[0].selected,None);assert_eq!(first.workspace.editors[0].saved_request_id.as_deref(),Some("local-request"));
        assert_eq!(first.workspace.editors[0].draft.upstream_profile_id.as_deref(),merged.ids.get("source-proxy").map(String::as_str));
        let second=merge_workspace(first.workspace,&source,export_id,&BTreeMap::from([("source-request".into(),"local-request".into())]),&merged.ids,&BTreeMap::new()).unwrap();
        assert_eq!(second.added,0);assert_eq!(second.workspace.editors.len(),1);
        let mut changed=second.workspace;
        changed.editors[0].draft.url="https://edited.invalid/".into();
        changed.editors[0].selected=Some("old-captured-flow".into());
        let mapping=BTreeMap::from([("source-request".into(),"local-request".into())]);
        let preview=merge_workspace(changed.clone(),&source,export_id,&mapping,&merged.ids,&BTreeMap::new()).unwrap();
        assert_eq!(preview.decisions.len(),1);assert_eq!(preview.decisions[0].section,"workspace");
        assert_eq!(preview.workspace.editors[0].draft.url,"https://edited.invalid/");
        let choices=BTreeMap::from([(preview.decisions[0].id.clone(),"imported".to_owned())]);
        let applied=merge_workspace(changed,&source,export_id,&mapping,&merged.ids,&choices).unwrap();
        assert_eq!(applied.added,1);
        assert_eq!(applied.workspace.editors[0].draft.url,"https://example.invalid/");
        assert!(applied.workspace.editors[0].selected.is_none());
        assert!(applied.workspace.editors[0].parent_id.is_none());
    }
    #[test]
    fn proxy_conflict_choice_clears_saved_credentials_and_keeps_identity(){
        let export_id="11111111-1111-4111-8111-111111111111";
        let source=vec![PortableProxy{id:"source".into(),name:"Imported".into(),url:"http://127.0.0.1:7897/".into(),username:"new-user".into(),auth_enabled:true}];
        let first=merge_proxies(Vec::new(),&source,export_id,&BTreeMap::new()).unwrap();
        let id=first.ids["source"].clone();
        let mut edited=first.profiles;
        edited[0]["name"]=json!("Renamed locally");
        edited[0]["url"]=json!("http://127.0.0.1:8000/");
        edited[0]["username"]=json!("old-user");
        edited[0]["password"]=json!("old-secret");
        edited[0]["rememberPassword"]=json!(true);
        let preview=merge_proxies(edited.clone(),&source,export_id,&BTreeMap::new()).unwrap();
        assert_eq!(preview.added,0);assert_eq!(preview.decisions.len(),1);
        assert_eq!(preview.decisions[0].section,"upstream");
        assert_eq!(preview.profiles[0]["password"],"old-secret");
        let choices=BTreeMap::from([(preview.decisions[0].id.clone(),"imported".to_owned())]);
        let applied=merge_proxies(edited,&source,export_id,&choices).unwrap();
        assert_eq!(applied.added,1);assert_eq!(applied.profiles[0]["id"],id);
        assert_eq!(applied.profiles[0]["name"],"Renamed locally");
        assert_eq!(applied.profiles[0]["url"],source[0].url);
        assert_eq!(applied.profiles[0]["username"],source[0].username);
        assert_eq!(applied.profiles[0]["rememberPassword"],false);
        assert!(applied.profiles[0].get("password").is_none());
        let repeated=merge_proxies(applied.profiles,&source,export_id,&BTreeMap::new()).unwrap();
        assert_eq!((repeated.added,repeated.skipped),(0,1));assert!(repeated.decisions.is_empty());
    }
}
