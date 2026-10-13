use crate::{Engine,model::Flow,store::Store};
use anyhow::{Result,ensure};
use serde::{Serialize,Deserialize};
use std::sync::{Arc,Mutex};
#[derive(Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Plugin {pub id:String,pub name:String,pub version:String,pub api_version:u32,#[serde(default)]pub enabled:bool,pub scope:String,pub url_pattern:String,#[serde(default)]pub before:String,#[serde(default)]pub after:String}
impl Plugin {fn validate(&self)->Result<()>{ensure!(self.api_version==1,"Unsupported plugin API version");ensure!(!self.id.is_empty()&&self.id.len()<=80&&self.id.bytes().all(|c|c.is_ascii_alphanumeric()||b".-_".contains(&c)),"Invalid plugin id");ensure!(!self.name.is_empty()&&self.name.len()<=128&&!self.version.is_empty()&&self.version.len()<=40,"Invalid plugin name/version");ensure!(["capture","replay","all"].contains(&self.scope.as_str()),"Invalid plugin scope");ensure!(self.url_pattern.len()<=2048,"URL pattern too long");regex::Regex::new(&self.url_pattern)?;ensure!(self.before.len()<=65536&&self.after.len()<=65536,"Plugin hook exceeds 64 KiB");Ok(())}}
pub struct Plugins {store:Arc<Store>,items:Mutex<Vec<Plugin>>,logs:Mutex<Vec<String>>}
impl Plugins {
 pub fn open(store:Arc<Store>)->Result<Self>{let items:Vec<Plugin>=store.setting("plugins.v1")?.unwrap_or_default();validate(&items)?;Ok(Self{store,items:Mutex::new(items),logs:Mutex::new(vec![])})}
 pub fn list(&self)->Vec<Plugin>{self.items.lock().unwrap().clone()}
 pub fn logs(&self)->Vec<String>{self.logs.lock().unwrap().clone()}
 pub fn save(&self,items:Vec<Plugin>)->Result<()>{validate(&items)?;let mut lock=self.items.lock().unwrap();self.store.save_setting("plugins.v1",&items)?;*lock=items;Ok(())}
 fn log(&self,text:String){let mut logs=self.logs.lock().unwrap();logs.push(text);if logs.len()>200{logs.remove(0);}}
}
fn validate(items:&[Plugin])->Result<()>{ensure!(items.len()<=32,"At most 32 plugins");let mut ids=std::collections::HashSet::new();for p in items{p.validate()?;ensure!(ids.insert(&p.id),"Duplicate plugin id");}Ok(())}
pub fn matching(items:&[Plugin],flow:&Flow)->Vec<Plugin>{items.iter().filter(|p|p.enabled&&(p.scope=="all"||p.scope==flow.source)&&regex::Regex::new(&p.url_pattern).is_ok_and(|r|r.is_match(&flow.request.url))).cloned().collect()}
pub async fn run(engine:&Engine,items:&[Plugin],flow:&mut Flow,after:bool){
 for p in items {let code=if after{&p.after}else{&p.before};if code.trim().is_empty(){continue}
 let stage=if after{"response"}else{"request"};
 let result=engine.scripts.execute(code.clone(),flow,Default::default(),Default::default(),after).await;
 let message=match result {Ok(output) if output.error.is_none()=>{if after{if flow.original_response.is_none(){flow.original_response=flow.response.clone()}flow.response=output.response}else{flow.original_request.get_or_insert_with(||flow.request.clone());flow.request=output.request}format!("[plugin {} {}] OK {}",p.id,stage,output.logs.join(" | "))},Ok(output)=>format!("[plugin {} {}] {}",p.id,stage,output.error.unwrap()),Err(e)=>format!("[plugin {} {}] {e:#}",p.id,stage)};
 let message=message.chars().take(8192).collect::<String>();engine.plugins.log(format!("{} {}",flow.id,message));flow.notes.push(message);
 }
}
