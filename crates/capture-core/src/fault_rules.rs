//! Declarative request fault simulation. Limits count requests in a fixed window;
//! they do not throttle body bytes or alter captured/forwarded payloads.
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, sync::{Arc, Mutex}, time::{Duration, Instant}};
use crate::{http1, model::{CapturedResponse, Flow, Header}, store::Store};

const KEY: &str = "fault-rules.v1";
fn scope() -> String { "capture".into() }
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default)] pub enabled: bool,
    #[serde(default="scope")] pub scope: String,
    #[serde(default)] pub rules: Vec<Rule>,
}
impl Default for Config { fn default()->Self { Self{enabled:false,scope:scope(),rules:vec![]} } }
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct RateLimit { pub requests:u32, pub window_ms:u32 }
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Rule {
    pub id:String,
    #[serde(default)] pub enabled:bool,
    pub match_type:String,
    pub pattern:String,
    #[serde(default)] pub method:String,
    #[serde(default)] pub delay_ms:u32,
    #[serde(default)] pub rate_limit:Option<RateLimit>,
    #[serde(skip)] compiled:Option<regex::Regex>,
}
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct Snapshot { pub config:Config, pub version:String }
struct Window { start:Instant, count:u32 }
struct State { config:Config, windows:HashMap<String,Window> }
pub struct FaultRules { store:Arc<Store>, state:Mutex<State> }

pub(crate) fn version(config:&Config)->String {
    format!("{:x}",Sha256::digest(serde_json::to_vec(config).expect("serializable fault rules")))
}
pub(crate) fn compile(config:&mut Config)->Result<()> {
    ensure!(["capture","all"].contains(&config.scope.as_str()),"模拟范围必须为 capture 或 all");
    ensure!(config.rules.len()<=100,"最多支持 100 条模拟规则");
    let mut ids=HashSet::new();
    for rule in &mut config.rules {
        ensure!(rule.id.len()<=128&&http1::token(&rule.id)&&ids.insert(rule.id.clone()),"模拟规则 ID 无效或重复");
        ensure!(!rule.pattern.trim().is_empty()&&rule.pattern.len()<=2048,"模拟规则匹配内容需要 1–2048 字节");
        ensure!(rule.method.is_empty()||(rule.method.len()<=32&&http1::token(&rule.method)),"模拟规则的方法无效");
        ensure!(rule.delay_ms<=30_000,"请求延迟不能超过 30000 ms");
        if let Some(limit)=&rule.rate_limit {ensure!((1..=10_000).contains(&limit.requests)&&(100..=60_000).contains(&limit.window_ms),"频率限制需要 1–10000 次 / 100–60000 ms");}
        ensure!(rule.delay_ms>0||rule.rate_limit.is_some(),"请至少设置请求延迟或频率限制");
        rule.compiled=match rule.match_type.as_str() {
            "urlContains"=>None,
            "urlRegex"=>Some(regex::RegexBuilder::new(&rule.pattern).size_limit(1024*1024).build().context("模拟规则 URL 正则表达式无效")?),
            "host"=>{
                ensure!(rule.pattern.len()<=253&&rule.pattern.trim()==rule.pattern&&!rule.pattern.contains(['*','/','@','?','#']),"请填写精确主机名，不含协议、端口或通配符");
                rule.pattern=url::Host::parse(&rule.pattern).context("无效的模拟规则主机名")?.to_string().trim_end_matches('.').to_ascii_lowercase();None
            },
            _=>anyhow::bail!("无效的模拟规则匹配类型"),
        };
    }Ok(())
}
pub fn validate_config(config:&Config)->Result<()> {let mut candidate=config.clone();compile(&mut candidate)}

impl FaultRules {
    pub fn open(store:Arc<Store>)->Result<Self> {
        let mut config=store.setting(KEY)?.unwrap_or_default();compile(&mut config)?;
        Ok(Self{store,state:Mutex::new(State{config,windows:HashMap::new()})})
    }
    pub fn snapshot(&self)->Snapshot {let state=self.state.lock().unwrap();Snapshot{version:version(&state.config),config:state.config.clone()}}
    pub fn configure(&self,mut config:Config,expected_version:&str)->Result<Snapshot> {
        compile(&mut config)?;
        let mut state=self.state.lock().unwrap();
        ensure!(version(&state.config)==expected_version,"模拟配置已更改，请重新加载后再保存");
        self.store.save_fault_rules(&config,expected_version)?;
        state.config=config;state.windows.clear();
        Ok(Snapshot{version:version(&state.config),config:state.config.clone()})
    }
    /// Applied exactly once, after request scripts/interception and blocking.
    /// Dropping this future cancels the delay without a background send task.
    pub(crate) async fn apply(&self,flow:&mut Flow)->Result<bool> {
        let selected={
            let mut state=self.state.lock().unwrap();
            if !state.config.enabled||(state.config.scope=="capture"&&flow.source!="capture") {return Ok(false);}
            let host=url::Url::parse(&flow.request.url).ok().and_then(|url|url.host().map(|h|h.to_string().trim_end_matches('.').to_ascii_lowercase()));
            let rule=state.config.rules.iter().find(|rule|rule.enabled&&(rule.method.is_empty()||rule.method.eq_ignore_ascii_case(&flow.request.method))&&match rule.match_type.as_str(){
                "host"=>host.as_deref()==Some(rule.pattern.as_str()),
                "urlRegex"=>rule.compiled.as_ref().is_some_and(|r|r.is_match(&flow.request.url)),
                _=>flow.request.url.contains(&rule.pattern),
            }).cloned();
            rule.map(|rule|{
                let retry=rule.rate_limit.as_ref().and_then(|limit|{
                    let now=Instant::now();let duration=Duration::from_millis(u64::from(limit.window_ms));
                    let window=state.windows.entry(rule.id.clone()).or_insert(Window{start:now,count:0});
                    if now.duration_since(window.start)>=duration {window.start=now;window.count=0;}
                    if window.count>=limit.requests {Some(duration.saturating_sub(now.duration_since(window.start)))}else{window.count+=1;None}
                });(rule,retry)
            })
        };
        let Some((rule,retry))=selected else{return Ok(false)};
        if let Some(retry)=retry {
            let text="HTTP Capture simulated a request rate limit.\n";
            let version=flow.client_protocol.as_deref().filter(|p|matches!(*p,"HTTP/2"|"HTTP/3")).unwrap_or("HTTP/1.1");
            flow.response=Some(CapturedResponse{status:429,version:version.into(),headers:vec![
                Header{disabled:false,name:"Content-Type".into(),value:"text/plain; charset=utf-8".into()},
                Header{disabled:false,name:"Content-Length".into(),value:text.len().to_string()},
                Header{disabled:false,name:"Retry-After".into(),value:retry.as_millis().div_ceil(1000).max(1).to_string()},
                Header{disabled:false,name:"Cache-Control".into(),value:"no-store".into()},
                Header{disabled:false,name:"X-HTTP-Capture-Simulated".into(),value:rule.id.clone()},
            ],body_base64:if flow.request.method.eq_ignore_ascii_case("HEAD"){Default::default()}else{STANDARD.encode(text).into()},trailers:vec![],raw_head_base64:None,upstream_tls:None,tls_version:None,sent_request_headers:None});
            flow.state="blocked".into();flow.notes.push(format!("请求频率模拟：规则 {} 返回本地 429，未向上游发送。",rule.id));return Ok(true);
        }
        if rule.delay_ms>0 {
            flow.notes.push(format!("请求延迟模拟：规则 {} 等待 {} ms。",rule.id,rule.delay_ms));
            tokio::time::sleep(Duration::from_millis(u64::from(rule.delay_ms))).await;
        }
        Ok(false)
    }
}
