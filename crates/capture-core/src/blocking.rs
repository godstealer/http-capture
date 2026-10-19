//! Local HTTP request blocking. This does not filter DNS or tunnelled TLS bytes.
use crate::{http1, model::*, store::Store};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::{Arc, RwLock}};
use tokio::io::{AsyncWrite, AsyncWriteExt};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default)] pub enabled: bool,
    #[serde(default = "capture_scope")] pub scope: String,
    #[serde(default)] pub rules: Vec<Rule>,
}
fn capture_scope() -> String { "capture".into() }
impl Default for Config {
    fn default() -> Self { Self { enabled:false, scope:capture_scope(), rules:vec![] } }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    #[serde(default)] pub enabled: bool,
    pub match_type: String,
    pub pattern: String,
    #[serde(default)] pub method: String,
    #[serde(skip)] compiled: Option<regex::Regex>,
}

pub struct Blocking { store:Arc<Store>, config:RwLock<Config> }

#[derive(Clone, Debug)]
pub struct Match { pub rule_id:String, pub match_type:String }

impl Match {
    pub(crate) fn apply(&self, flow:&mut Flow) {
        let version = flow.client_protocol.as_deref().filter(|p|matches!(*p,"HTTP/2"|"HTTP/3"))
            .unwrap_or(match flow.request.engine.as_str() { "h2"=>"HTTP/2", "h3"=>"HTTP/3", _=>"HTTP/1.1" });
        flow.response = Some(self.response(version));
        // HEAD has the same representation headers as GET, without payload bytes.
        if flow.request.method == "HEAD" { flow.response.as_mut().unwrap().body_base64 = Default::default(); }
        flow.error = None;
        flow.state = "blocked".into();
        flow.notes.push(self.reason());
    }
    fn reason(&self) -> String {
        let condition = match self.match_type.as_str() {
            "host" => "域名精确匹配", "urlRegex" => "URL 正则匹配", _ => "URL 包含匹配",
        };
        format!("请求已阻止，未向上游发送 HTTP 请求。规则 {}（{}）。", self.rule_id, condition)
    }
    fn response(&self, version:&str) -> CapturedResponse {
        // Do not duplicate a potentially sensitive URL/query in local error text.
        let text = format!("HTTP Capture blocked this request.\nRule: {}\nMatch type: {}\n", self.rule_id, self.match_type);
        CapturedResponse {
            status:403, version:version.into(), body_base64:STANDARD.encode(text.as_bytes()).into(),
            headers:vec![
                Header { disabled:false, name:"Content-Type".into(), value:"text/plain; charset=utf-8".into() },
                Header { disabled:false, name:"Content-Length".into(), value:text.len().to_string() },
                Header { disabled:false, name:"Cache-Control".into(), value:"no-store".into() },
                Header { disabled:false, name:"X-HTTP-Capture-Blocked".into(), value:self.rule_id.clone() },
            ],
            trailers:vec![], raw_head_base64:None, upstream_tls:None, tls_version:None, sent_request_headers:None,
        }
    }
}

fn canonical_host(value:&str) -> Result<String> {
    ensure!(!value.is_empty() && value.len()<=253 && !value.contains(['*','/','@','?','#']) && value.trim()==value,
        "阻止域名必须是精确主机名，不含协议、端口或通配符");
    Ok(url::Host::parse(value).context("无效的阻止域名")?.to_string().trim_end_matches('.').to_ascii_lowercase())
}

fn compile(config:&mut Config) -> Result<()> {
    ensure!(matches!(config.scope.as_str(), "capture" | "all"), "阻止范围必须为 capture 或 all");
    ensure!(config.rules.len()<=100, "最多支持 100 条阻止规则");
    let mut ids = HashSet::new();
    for rule in &mut config.rules {
        ensure!(rule.id.len()<=128 && http1::token(&rule.id) && ids.insert(rule.id.clone()), "阻止规则 ID 无效或重复");
        ensure!(!rule.pattern.trim().is_empty() && rule.pattern.len()<=2048, "阻止规则不能为空，且最多 2048 字节");
        ensure!(rule.method.is_empty() || (rule.method.len()<=32 && http1::token(&rule.method)), "阻止规则的方法无效");
        rule.compiled = match rule.match_type.as_str() {
            "urlContains" => None,
            "urlRegex" => Some(regex::RegexBuilder::new(&rule.pattern).size_limit(1024*1024).build().context("URL 阻止正则表达式无效")?),
            "host" => { rule.pattern = canonical_host(&rule.pattern)?; None },
            _ => anyhow::bail!("无效的阻止匹配类型"),
        };
    }
    Ok(())
}
/// Offline migration validation; never changes the active configuration.
pub fn validate_config(config:&Config)->Result<()> {let mut value=config.clone();compile(&mut value)}

impl Blocking {
    pub fn open(store:Arc<Store>) -> Result<Self> {
        let mut config = store.setting("blocking.v1")?.unwrap_or_default();
        compile(&mut config)?;
        Ok(Self { store, config:RwLock::new(config) })
    }
    pub fn snapshot(&self) -> Config { self.config.read().unwrap().clone() }
    pub fn configure(&self, mut config:Config) -> Result<()> {
        compile(&mut config)?;
        let mut current = self.config.write().unwrap();
        self.store.save_setting("blocking.v1", &config)?;
        *current = config;
        Ok(())
    }
    pub fn check(&self, request:&RequestDraft, source:&str) -> Option<Match> {
        let config = self.config.read().unwrap();
        if !config.enabled || (config.scope=="capture" && source!="capture") { return None; }
        let host = url::Url::parse(&request.url).ok().and_then(|u|u.host().map(|h|h.to_string().trim_end_matches('.').to_ascii_lowercase()));
        config.rules.iter().find(|rule|rule.enabled && (rule.method.is_empty() || rule.method.eq_ignore_ascii_case(&request.method)) && match rule.match_type.as_str() {
            "host" => host.as_deref()==Some(rule.pattern.as_str()),
            "urlRegex" => rule.compiled.as_ref().is_some_and(|r|r.is_match(&request.url)),
            _ => request.url.contains(&rule.pattern),
        }).map(|rule|Match { rule_id:rule.id.clone(), match_type:rule.match_type.clone() })
    }
    /// A method-free exact host rule can be decided before reading HTTP inside TLS.
    pub(crate) fn host_match_before_tls(&self, url:&url::Url, source:&str) -> Option<Match> {
        let config = self.config.read().unwrap();
        if !config.enabled || (config.scope=="capture" && source!="capture") { return None; }
        let host = url.host().map(|h|h.to_string().trim_end_matches('.').to_ascii_lowercase());
        config.rules.iter().find(|rule|rule.enabled && rule.match_type=="host" && rule.method.is_empty() && host.as_deref()==Some(rule.pattern.as_str()))
            .map(|rule|Match { rule_id:rule.id.clone(), match_type:rule.match_type.clone() })
    }
    pub(crate) fn blocks_host_before_tls(&self, url:&url::Url, source:&str) -> bool {
        self.host_match_before_tls(url, source).is_some()
    }
    pub(crate) fn apply(&self, flow:&mut Flow) -> bool {
        let Some(hit) = self.check(&flow.request, &flow.source) else { return false; };
        hit.apply(flow);
        true
    }
}

/// Return a local denial before a WebSocket Upgrade is sent upstream.
pub(crate) async fn write_response<W:AsyncWrite+Unpin>(stream:&mut W, response:&CapturedResponse) -> Result<()> {
    stream.write_all(b"HTTP/1.1 403 Forbidden\r\n").await?;
    for header in &response.headers {
        stream.write_all(format!("{}: {}\r\n", header.name, header.value).as_bytes()).await?;
    }
    stream.write_all(b"Connection: close\r\n\r\n").await?;
    stream.write_all(&STANDARD.decode(&response.body_base64)?).await?;
    stream.flush().await?;
    Ok(())
}
