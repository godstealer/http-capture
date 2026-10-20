//! Persistent, post-upstream response edits. These rules never replay requests
//! and never act as local mocks. Each rule either commits completely or leaves
//! the response produced by earlier rules unchanged.
use crate::{http1, model::{CapturedResponse, Flow, Header, SharedBody}, store::Store};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use regex::{Captures, Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, io::{Read, Write}, sync::{Arc, Mutex}};

pub(crate) const KEY: &str = "response-rewrite.v1";
const MAX_BODY: usize = 8 * 1024 * 1024;
const MAX_HEADERS: usize = 64 * 1024;
const MAX_CONFIG: usize = 16 * 1024 * 1024;
fn scope() -> String { "capture".into() }
fn yes() -> bool { true }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default)] pub enabled: bool,
    #[serde(default = "scope")] pub scope: String,
    #[serde(default)] pub rules: Vec<Rule>,
}
impl Default for Config {
    fn default() -> Self { Self { enabled: false, scope: scope(), rules: vec![] } }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub enabled: bool,
    pub match_type: String,
    pub pattern: String,
    #[serde(default)] pub method: String,
    pub action: String,
    #[serde(default)] pub replacement: Option<Replacement>,
    #[serde(default)] pub modifications: Vec<Modification>,
    #[serde(skip)] url_regex: Option<Regex>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Replacement {
    pub status: u16,
    pub headers: Vec<Header>,
    pub body_base64: SharedBody,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Modification {
    pub target: String,
    pub operation: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub find: String,
    #[serde(default)] pub value: String,
    #[serde(default)] pub regex: bool,
    #[serde(default = "yes")] pub case_sensitive: bool,
    #[serde(default = "yes")] pub all: bool,
    #[serde(skip)] compiled: Option<Regex>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot { pub config: Config, pub version: String, pub hits: Vec<u64> }
struct State { config: Arc<Config>, hits: Vec<u64> }
pub struct ResponseRewrite { store: Arc<Store>, state: Mutex<State> }

struct JsonBudget(usize);
impl Write for JsonBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.0 { return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "响应重写配置不能超过 16 MiB")); }
        self.0 -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

pub(crate) fn version(config: &Config) -> String {
    // Typed structs have fixed key order; compiled regular expressions are not
    // serialized. Versioning never includes counters or transient response data.
    format!("{:x}", Sha256::digest(serde_json::to_vec(config).expect("serializable response rewrite config")))
}

fn header_value(value: &str) -> Result<()> {
    ensure!(value.chars().all(|c| c as u32 <= 255 && (c >= ' ' || c == '\t') && c != '\u{7f}'),
        "响应头值必须为单行 Latin-1 字节，不能包含控制字符（Tab 除外）");
    Ok(())
}
fn validate_headers(headers: &[Header]) -> Result<()> {
    ensure!(headers.len() <= 256, "响应头不能超过 256 项");
    let mut size = 0usize;
    for header in headers {
        ensure!(http1::token(&header.name), "响应头名称无效");
        header_value(&header.value)?;
        size = size.checked_add(header.name.len() + header.value.len() + 4).context("响应头过大")?;
    }
    ensure!(size <= MAX_HEADERS, "响应头不能超过 64 KiB");
    Ok(())
}
fn protected_header(name: &str) -> bool {
    ["content-length", "transfer-encoding", "connection", "proxy-connection", "keep-alive", "te", "trailer", "upgrade", "proxy-authenticate", "proxy-authorization"]
        .iter().any(|item| name.eq_ignore_ascii_case(item))
}
fn regex(pattern: &str, case_sensitive: bool) -> Result<Regex> {
    RegexBuilder::new(pattern).case_insensitive(!case_sensitive).size_limit(1024 * 1024)
        .build().context("响应重写正则表达式无效或超过 1 MiB 编译预算")
}
pub(crate) fn compile(config: &mut Config) -> Result<()> {
    serde_json::to_writer(&mut JsonBudget(MAX_CONFIG), &*config).context("响应重写配置不能超过 16 MiB")?;
    ensure!(["capture", "all", "replay"].contains(&config.scope.as_str()), "响应重写范围必须为 capture、all 或 replay");
    ensure!(config.rules.len() <= 32, "最多支持 32 条响应重写规则");
    let mut ids = HashSet::new();
    for rule in &mut config.rules {
        ensure!(rule.id.len() <= 128 && http1::token(&rule.id) && ids.insert(rule.id.clone()), "响应重写规则 ID 无效或重复");
        ensure!(rule.name.chars().count() <= 128, "响应重写规则名称不能超过 128 个字符");
        ensure!(!rule.pattern.trim().is_empty() && rule.pattern.len() <= 2048, "响应重写匹配内容需要 1–2048 字节");
        ensure!(rule.method.is_empty() || (rule.method.len() <= 32 && http1::token(&rule.method)), "响应重写方法无效");
        rule.url_regex = match rule.match_type.as_str() {
            "urlContains" => None,
            "urlRegex" => Some(regex(&rule.pattern, true)?),
            "host" => {
                ensure!(rule.pattern.len() <= 253 && rule.pattern.trim() == rule.pattern && !rule.pattern.contains(['*', '/', '@', '?', '#']),
                    "请填写精确主机名，不含协议、端口或通配符");
                rule.pattern = url::Host::parse(&rule.pattern).context("响应重写主机名无效")?.to_string().trim_end_matches('.').to_ascii_lowercase();
                None
            }
            _ => anyhow::bail!("响应重写匹配类型无效"),
        };
        ensure!(rule.modifications.len() <= 32, "每条规则最多支持 32 个修改操作");
        match rule.action.as_str() {
            "replace" => {
                ensure!(rule.modifications.is_empty(), "整份响应替换不能同时包含局部修改");
                let value = rule.replacement.as_ref().context("整份响应替换需要 replacement")?;
                ensure!((200..=599).contains(&value.status), "替换响应状态码必须为 200–599");
                validate_headers(&value.headers)?;
                decode_base64(&value.body_base64)?;
            }
            "modify" => {
                ensure!(rule.replacement.is_none() && !rule.modifications.is_empty(), "局部修改需要操作列表，不能同时包含整份替换");
                for modification in &mut rule.modifications {
                    ensure!(modification.find.len() <= 1024 * 1024 && modification.value.len() <= 1024 * 1024, "查找和替换内容各不能超过 1 MiB");
                    ensure!(modification.target == "body" || modification.target == "header", "响应修改目标无效");
                    ensure!(["replace", "set", "remove"].contains(&modification.operation.as_str()), "响应修改操作无效");
                    if modification.target == "body" {
                        ensure!(modification.operation == "replace", "正文局部修改仅支持查找替换");
                    } else {
                        ensure!(http1::token(&modification.name) && modification.name.len() <= 256, "响应头修改名称无效");
                        ensure!(!protected_header(&modification.name), "长度与逐跳响应头由程序推导，不能直接修改");
                        if modification.operation == "set" {
                            header_value(&modification.value)?;
                            ensure!(modification.name.len() + modification.value.len() + 4 <= MAX_HEADERS, "响应头修改结果超过 64 KiB");
                        }
                    }
                    modification.compiled = if modification.operation == "replace" {
                        ensure!(!modification.find.is_empty(), "查找内容不能为空");
                        let pattern = if modification.regex { modification.find.clone() } else { regex::escape(&modification.find) };
                        Some(regex(&pattern, modification.case_sensitive)?)
                    } else { None };
                }
            }
            _ => anyhow::bail!("响应重写动作必须为 replace 或 modify"),
        }
    }
    Ok(())
}
pub fn validate_config(config: &Config) -> Result<()> { compile(&mut config.clone()) }

impl ResponseRewrite {
    pub fn open(store: Arc<Store>) -> Result<Self> {
        let mut config = store.setting(KEY)?.unwrap_or_default();
        compile(&mut config)?;
        let hits = vec![0; config.rules.len()];
        Ok(Self { store, state: Mutex::new(State { config: Arc::new(config), hits }) })
    }
    pub fn snapshot(&self) -> Snapshot {
        let state = self.state.lock().unwrap();
        Snapshot { version: version(&state.config), config: (*state.config).clone(), hits: state.hits.clone() }
    }
    pub fn configure(&self, mut config: Config, expected_version: &str) -> Result<Snapshot> {
        compile(&mut config)?;
        let mut state = self.state.lock().unwrap();
        ensure!(version(&state.config) == expected_version, "响应重写配置已更改，请重新加载后保存");
        self.store.save_response_rewrite(&config, expected_version)?;
        state.hits = vec![0; config.rules.len()];
        state.config = Arc::new(config);
        Ok(Snapshot { version: version(&state.config), config: (*state.config).clone(), hits: state.hits.clone() })
    }
    pub fn apply(&self, flow: &mut Flow) {
        if flow.response.is_none() || flow.error.is_some() { return; }
        let (config, selected) = {
            let mut state = self.state.lock().unwrap();
            if !state.config.enabled || (state.config.scope != "all" && state.config.scope != flow.source) { return; }
            let host = url::Url::parse(&flow.request.url).ok().and_then(|value| value.host().map(|h| h.to_string().trim_end_matches('.').to_ascii_lowercase()));
            let selected: Vec<usize> = state.config.rules.iter().enumerate().filter(|(_, rule)| rule.enabled
                && (rule.method.is_empty() || rule.method.eq_ignore_ascii_case(&flow.request.method))
                && match rule.match_type.as_str() {
                    "host" => host.as_deref() == Some(rule.pattern.as_str()),
                    "urlRegex" => rule.url_regex.as_ref().is_some_and(|value| value.is_match(&flow.request.url)),
                    _ => flow.request.url.contains(&rule.pattern),
                }).map(|(index, _)| index).collect();
            if selected.is_empty() { return; }
            if unsupported_stream(flow) {
                flow.notes.push("自动响应重写：WebSocket、SSE 与 gRPC 流式响应不支持自动修改，已保留原响应。".into());
                return;
            }
            for &index in &selected { state.hits[index] = state.hits[index].saturating_add(1); }
            (state.config.clone(), selected)
        };
        for index in selected {
            let rule = &config.rules[index];
            let original = flow.response.as_ref().unwrap();
            match apply_rule(rule, original, &flow.request.method) {
                Ok(Some(candidate)) => {
                    if flow.original_response.is_none() { flow.original_response = Some(original.clone()); }
                    flow.response = Some(candidate);
                    flow.notes.push(format!("自动响应重写：规则 {} 已{}；请求已访问上游。", rule.id, if rule.action == "replace" { "替换完整响应" } else { "修改响应" }));
                }
                Ok(None) => flow.notes.push(format!("自动响应重写：规则 {} 匹配，但内容没有变化。", rule.id)),
                Err(error) => {
                    let reason: String = error.to_string().chars().take(384).collect();
                    flow.notes.push(format!("自动响应重写：规则 {} 失败，保留本规则前的响应：{reason}", rule.id));
                }
            }
        }
    }
}

fn unsupported_stream(flow: &Flow) -> bool {
    if flow.websocket.is_some() || flow.response.as_ref().is_some_and(|value| value.status == 101)
        || flow.notes.iter().any(|value| value == "SSE 接收中") { return true; }
    flow.response.as_ref().into_iter().flat_map(|value| &value.headers).chain(flow.request.headers.iter())
        .filter(|value| !value.disabled && value.name.eq_ignore_ascii_case("content-type"))
        .any(|value| { let mime = value.value.split(';').next().unwrap_or("").trim().to_ascii_lowercase(); mime == "text/event-stream" || mime.starts_with("application/grpc") })
}
fn decode_base64(body: &str) -> Result<Vec<u8>> {
    ensure!(body.len() <= MAX_BODY.div_ceil(3) * 4, "响应正文超过 8 MiB");
    let bytes = STANDARD.decode(body).context("响应正文不是有效 Base64")?;
    ensure!(bytes.len() <= MAX_BODY, "响应正文超过 8 MiB");
    Ok(bytes)
}
fn body_length(body: &str) -> Result<u64> {
    let tail = body.get(body.len().saturating_sub(4)..).context("响应正文编码无效")?;
    Ok(crate::body_read::Base64ReadPlan::new(body.len(), tail, 0, 1)?.body.total_bytes)
}
fn limited_decode(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_BODY as u64 + 1).read_to_end(&mut bytes).context("响应压缩正文解码失败")?;
    ensure!(bytes.len() <= MAX_BODY, "解压后的响应正文超过 8 MiB");
    Ok(bytes)
}
fn text_body(response: &CapturedResponse) -> Result<String> {
    for header in response.headers.iter().filter(|header| !header.disabled && header.name.eq_ignore_ascii_case("content-type")) {
        let mut parts = header.value.split(';');
        let mime = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        ensure!(!(mime.starts_with("image/") && mime != "image/svg+xml") && !mime.starts_with("audio/")
            && !mime.starts_with("video/") && !mime.starts_with("font/")
            && !["application/octet-stream", "application/pdf"].contains(&mime.as_str()),
            "此 Content-Type 为二进制内容，正文局部修改仅支持 UTF-8 文本");
        for parameter in parts {
            if let Some((name, value)) = parameter.split_once('=') {
                if name.trim().eq_ignore_ascii_case("charset") {
                    let value = value.trim();
                    let value = value.strip_prefix('"').and_then(|value| value.strip_suffix('"')).unwrap_or(value);
                    ensure!(value.eq_ignore_ascii_case("utf-8") || value.eq_ignore_ascii_case("utf8"),
                        "响应声明非 UTF-8 字符集，未修改正文；请保持原编码或使用整份响应替换");
                }
            }
        }
    }
    let mut bytes = decode_base64(&response.body_base64)?;
    let codings: Vec<_> = response.headers.iter().filter(|header| !header.disabled && header.name.eq_ignore_ascii_case("content-encoding"))
        .flat_map(|header| header.value.split(',')).map(|coding| coding.trim().to_ascii_lowercase()).collect();
    ensure!(codings.len() <= 8, "响应编码层数超过 8 层");
    for coding in codings.into_iter().rev() {
        bytes = match coding.as_str() {
            "identity" => bytes,
            "gzip" | "x-gzip" => limited_decode(flate2::read::MultiGzDecoder::new(&bytes[..]))?,
            "deflate" => {
                // HTTP deflate is zlib-wrapped. Support old raw-deflate peers,
                // without retrying corrupt streams that advertise a zlib header.
                let zlib = bytes.len() >= 2 && bytes[0] & 15 == 8 && (u16::from(bytes[0]) * 256 + u16::from(bytes[1])) % 31 == 0;
                if zlib { limited_decode(flate2::read::ZlibDecoder::new(&bytes[..]))? }
                else { limited_decode(flate2::read::DeflateDecoder::new(&bytes[..]))? }
            }
            "br" => limited_decode(brotli::Decompressor::new(&bytes[..], 4096))?,
            "zstd" => {
                let mut decoder = zstd::stream::read::Decoder::new(&bytes[..]).context("Zstd 响应解码失败")?;
                decoder.window_log_max(23).context("Zstd 响应窗口超过预算")?;
                limited_decode(decoder)?
            }
            _ => anyhow::bail!("不支持此响应 Content-Encoding，未修改正文"),
        };
    }
    let text = String::from_utf8(bytes).context("响应正文不是有效 UTF-8 文本，未修改二进制数据")?;
    ensure!(!text.contains('\0'), "响应正文含 NUL 二进制字节，未进行文本修改");
    Ok(text)
}
fn append(output: &mut String, value: &str, limit: usize) -> Result<()> {
    ensure!(value.len() <= limit.saturating_sub(output.len()), "替换结果超过正文或响应头预算");
    output.push_str(value);
    Ok(())
}
// Implements Rust regex $1/${name}/$$ expansion with a checked append for each
// capture. Expanding one large capture many times cannot allocate unboundedly.
fn expand(output: &mut String, mut value: &str, captures: &Captures<'_>, limit: usize) -> Result<()> {
    while let Some(position) = value.find('$') {
        append(output, &value[..position], limit)?;
        value = &value[position..];
        if value.starts_with("$$") { append(output, "$", limit)?; value = &value[2..]; continue; }
        let reference = if let Some(rest) = value.strip_prefix("${") {
            rest.find('}').map(|end| (&rest[..end], end + 3))
        } else {
            let end = value.as_bytes()[1..].iter().take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_').count() + 1;
            (end > 1).then_some((&value[1..end], end))
        };
        if let Some((name, consumed)) = reference {
            let matched = if let Ok(index) = name.parse::<usize>() { captures.get(index) } else { captures.name(name) };
            if let Some(matched) = matched { append(output, matched.as_str(), limit)?; }
            value = &value[consumed..];
        } else { append(output, "$", limit)?; value = &value[1..]; }
    }
    append(output, value, limit)
}
fn replace_text(input: &str, modification: &Modification, limit: usize) -> Result<String> {
    let expression = modification.compiled.as_ref().context("响应修改正则未编译")?;
    let mut output = String::new();
    let mut end = 0;
    for captures in expression.captures_iter(input) {
        let matched = captures.get(0).unwrap();
        append(&mut output, &input[end..matched.start()], limit)?;
        if modification.regex { expand(&mut output, &modification.value, &captures, limit)?; }
        else { append(&mut output, &modification.value, limit)?; }
        end = matched.end();
        if !modification.all { break; }
    }
    append(&mut output, &input[end..], limit)?;
    Ok(output)
}
fn modify_header(headers: &mut Vec<Header>, modification: &Modification) -> Result<bool> {
    ensure!(headers.len() <= 256, "响应头不能超过 256 项");
    let mut selected = false;
    let mut changed = false;
    let mut output = Vec::with_capacity(headers.len());
    let mut size = 0usize;
    for header in headers.iter() {
        if header.disabled || !header.name.eq_ignore_ascii_case(&modification.name) || (selected && !modification.all) {
            ensure!(header.name.len() + header.value.len() + 4 <= MAX_HEADERS.saturating_sub(size), "响应头不能超过 64 KiB");
            size += header.name.len() + header.value.len() + 4;
            output.push(header.clone()); continue;
        }
        selected = true;
        if modification.operation == "remove" { changed = true; continue; }
        let overhead = header.name.len() + 4;
        ensure!(overhead <= MAX_HEADERS.saturating_sub(size), "响应头不能超过 64 KiB");
        let remaining = MAX_HEADERS - size - overhead;
        let value = if modification.operation == "set" {
            ensure!(modification.value.len() <= remaining, "响应头修改结果超过 64 KiB");
            modification.value.clone()
        } else { replace_text(&header.value, modification, remaining)? };
        header_value(&value)?;
        changed |= value != header.value;
        size += overhead + value.len();
        output.push(Header { disabled: header.disabled, name: header.name.clone(), value });
    }
    if !selected && modification.operation == "set" {
        ensure!(output.len() < 256 && modification.name.len() + modification.value.len() + 4 <= MAX_HEADERS.saturating_sub(size),
            "响应头修改结果超过 256 项或 64 KiB");
        output.push(Header { disabled: false, name: modification.name.clone(), value: modification.value.clone() });
        changed = true;
    }
    validate_headers(&output)?;
    *headers = output;
    Ok(changed)
}
fn invalidated(name: &str) -> bool {
    ["content-encoding", "etag", "content-md5", "digest", "content-digest", "repr-digest", "last-modified", "content-range", "accept-ranges"]
        .iter().any(|value| name.eq_ignore_ascii_case(value))
}
fn normalize(response: &mut CapturedResponse, method: &str, replacement_length: Option<u64>, body_changed: bool) -> Result<()> {
    let no_body = method.eq_ignore_ascii_case("HEAD") || [204, 205, 304].contains(&response.status);
    let length = if let Some(length) = replacement_length { length } else { body_length(&response.body_base64)? };
    response.headers = http1::end_to_end(&response.headers);
    if body_changed { response.headers.retain(|header| !invalidated(&header.name)); response.trailers.clear(); }
    // HEAD and 304 may carry an upstream representation length, which is not
    // the zero bytes sent on this response. Preserve it for header-only edits.
    let preserve_length = replacement_length.is_none() && !body_changed && (method.eq_ignore_ascii_case("HEAD") || response.status == 304);
    if !preserve_length { response.headers.retain(|header| !header.name.eq_ignore_ascii_case("content-length")); }
    if response.status == 204 {
        response.headers.retain(|header| !header.name.eq_ignore_ascii_case("content-length"));
    } else if !preserve_length && response.status != 304 {
        let value = if response.status == 205 { "0".into() } else { length.to_string() };
        let bytes: usize = response.headers.iter().map(|header| header.name.len() + header.value.len() + 4).sum();
        ensure!(response.headers.len() < 256 && "Content-Length".len() + value.len() + 4 <= MAX_HEADERS.saturating_sub(bytes),
            "重算长度后响应头超过 256 项或 64 KiB");
        response.headers.push(Header { disabled: false, name: "Content-Length".into(), value });
    }
    if no_body { response.body_base64 = Default::default(); }
    response.raw_head_base64 = None;
    validate_headers(&response.headers)
}
fn apply_rule(rule: &Rule, original: &CapturedResponse, method: &str) -> Result<Option<CapturedResponse>> {
    let mut candidate = original.clone();
    if rule.action == "replace" {
        let value = rule.replacement.as_ref().context("整份响应替换缺少内容")?;
        let length = body_length(&value.body_base64)?;
        candidate.status = value.status;
        candidate.headers = value.headers.clone();
        candidate.body_base64 = value.body_base64.clone();
        candidate.trailers.clear();
        normalize(&mut candidate, method, Some(length), false)?;
        return Ok(Some(candidate));
    }
    let mut changed = false;
    let mut body_changed = false;
    for modification in &rule.modifications {
        if modification.target == "header" {
            // Connection-nominated hop-by-hop fields are also derived fields.
            ensure!(!candidate.headers.iter().filter(|header| !header.disabled && header.name.eq_ignore_ascii_case("connection"))
                .flat_map(|header| header.value.split(',')).any(|name| name.trim().eq_ignore_ascii_case(&modification.name)),
                "不能修改 Connection 声明的逐跳响应头");
            changed |= modify_header(&mut candidate.headers, modification)?;
        } else {
            ensure!(!method.eq_ignore_ascii_case("HEAD") && ![204, 205, 206, 304].contains(&candidate.status), "HEAD、204、205、206 或 304 响应不支持正文局部修改");
            let text = text_body(&candidate)?;
            let result = replace_text(&text, modification, MAX_BODY)?;
            ensure!(!result.contains('\0'), "替换结果包含 NUL，不能作为 UTF-8 文本正文");
            if result != text {
                candidate.body_base64 = STANDARD.encode(result.as_bytes()).into();
                candidate.headers.retain(|header| !invalidated(&header.name));
                candidate.trailers.clear();
                changed = true;
                body_changed = true;
            }
        }
    }
    if !changed { return Ok(None); }
    normalize(&mut candidate, method, None, body_changed)?;
    Ok(Some(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::io::Write;

    fn h(name: &str, value: &str) -> Header {
        Header { disabled: false, name: name.into(), value: value.into() }
    }
    fn sample(body: &[u8], headers: Vec<Header>) -> Flow {
        serde_json::from_value(json!({
            "id":"flow","startedAt":1,"durationMs":5,"source":"capture","state":"sending",
            "parentId":null,"error":null,"notes":[],
            "request":{"method":"POST","url":"https://example.test/api?x=1","headers":[],"bodyBase64":""},
            "response":{"status":200,"version":"HTTP/2","tlsVersion":"TLS 1.3","headers":headers,
                "bodyBase64":STANDARD.encode(body),"rawHeadBase64":"cmF3"}
        })).unwrap()
    }
    fn setup() -> (tempfile::TempDir, Arc<Store>, ResponseRewrite) {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("test.db")).unwrap());
        let rewrite = ResponseRewrite::open(store.clone()).unwrap();
        (directory, store, rewrite)
    }
    fn modify(id: &str, modifications: Value) -> Value {
        json!({"id":id,"name":id,"enabled":true,"matchType":"urlContains","pattern":"example.test",
            "method":"POST","action":"modify","replacement":null,"modifications":modifications})
    }
    fn body(find: &str, value: &str) -> Value {
        json!({"target":"body","operation":"replace","find":find,"value":value,"regex":false,"caseSensitive":true,"all":true})
    }
    fn replacement(id: &str, status: u16, bytes: &[u8], headers: Vec<Header>) -> Value {
        json!({"id":id,"enabled":true,"matchType":"urlContains","pattern":"example.test","method":"",
            "action":"replace","replacement":{"status":status,"headers":headers,"bodyBase64":STANDARD.encode(bytes)},"modifications":[]})
    }
    fn config(rules: Value) -> Config {
        serde_json::from_value(json!({"enabled":true,"scope":"capture","rules":rules})).unwrap()
    }
    fn configure(rewrite: &ResponseRewrite, rules: Value) {
        rewrite.configure(config(rules), &rewrite.snapshot().version).unwrap();
    }
    fn bytes(flow: &Flow) -> Vec<u8> { STANDARD.decode(&flow.response.as_ref().unwrap().body_base64).unwrap() }
    fn header_values(flow: &Flow, name: &str) -> Vec<String> {
        flow.response.as_ref().unwrap().headers.iter().filter(|header| header.name.eq_ignore_ascii_case(name)).map(|header| header.value.clone()).collect()
    }

    #[test]
    fn replacement_preserves_actual_transport_metadata_and_earliest_original() {
        let (_directory, _store, rewrite) = setup();
        let mut flow = sample(b"original", vec![h("ETag", "old")]);
        let response = flow.response.as_mut().unwrap();
        response.upstream_tls = Some(crate::model::TlsDetails::default());
        response.sent_request_headers = Some(vec![h("X-Sent", "kept")]);
        response.trailers = vec![h("X-Trailer", "old")];
        let original = serde_json::to_value(&flow.response).unwrap();
        let binary = [0, 255, 128, 13, 10];
        let mut disabled = h("X-Disabled", "not sent"); disabled.disabled = true;
        configure(&rewrite, json!([replacement("whole", 201, &binary, vec![
            h("X-Order", "first"), h("Connection", "X-Hop"), h("X-Hop", "drop"),
            h("X-Middle", "middle"), h("x-order", "last"), h("content-length", "999"),
            h("Transfer-Encoding", "chunked"), disabled,
        ])]));
        rewrite.apply(&mut flow);
        assert_eq!(bytes(&flow), binary);
        let response = flow.response.as_ref().unwrap();
        assert_eq!(response.status, 201);
        assert_eq!(response.version, "HTTP/2");
        assert_eq!(response.tls_version.as_deref(), Some("TLS 1.3"));
        assert!(response.upstream_tls.is_some());
        assert_eq!(response.sent_request_headers.as_ref().unwrap()[0].value, "kept");
        assert_eq!(response.headers.iter().map(|header| header.name.as_str()).collect::<Vec<_>>(), ["X-Order", "X-Middle", "x-order", "Content-Length"]);
        assert_eq!(header_values(&flow, "content-length"), ["5"]);
        assert!(response.raw_head_base64.is_none() && response.trailers.is_empty());
        assert_eq!(serde_json::to_value(&flow.original_response).unwrap(), original);
        assert_eq!(rewrite.snapshot().hits, [1]);
    }

    #[test]
    fn matching_rules_run_in_order_and_each_failure_is_atomic() {
        let (_directory, _store, rewrite) = setup();
        configure(&rewrite, json!([
            modify("first", json!([body("one", "two")])),
            modify("bad", json!([
                {"target":"header","operation":"set","name":"X-Before-Failure","value":"must roll back"},
                body("two", &"x".repeat(1024 * 1024)),body("x", &"x".repeat(1024 * 1024))
            ])),
            modify("last", json!([body("two", "three")]))
        ]));
        let mut flow = sample(b"one", vec![h("Content-Length", "3")]);
        rewrite.apply(&mut flow);
        assert_eq!(bytes(&flow), b"three");
        assert!(header_values(&flow, "X-Before-Failure").is_empty());
        assert_eq!(STANDARD.decode(&flow.original_response.as_ref().unwrap().body_base64).unwrap(), b"one");
        assert!(flow.notes.iter().any(|note| note.contains("bad 失败")));
        assert_eq!(rewrite.snapshot().hits, [1, 1, 1]);
    }

    #[test]
    fn scope_disabled_method_host_and_url_regex_are_respected() {
        let (_directory, _store, rewrite) = setup();
        let mut rule = modify("match", json!([body("old", "new")]));
        rule["matchType"] = json!("host"); rule["pattern"] = json!("EXAMPLE.TEST."); rule["method"] = json!("post");
        let mut value = config(json!([rule]));
        value.scope = "replay".into();
        rewrite.configure(value.clone(), &rewrite.snapshot().version).unwrap();
        let mut flow = sample(b"old", vec![]);
        rewrite.apply(&mut flow); assert_eq!(bytes(&flow), b"old");
        flow.source = "replay".into(); flow.request.method = "GET".into();
        rewrite.apply(&mut flow); assert_eq!(bytes(&flow), b"old");
        flow.request.method = "POST".into(); rewrite.apply(&mut flow); assert_eq!(bytes(&flow), b"new");
        value.enabled = false;
        rewrite.configure(value.clone(), &rewrite.snapshot().version).unwrap();
        let mut disabled = sample(b"old", vec![]); disabled.source = "replay".into();
        rewrite.apply(&mut disabled); assert_eq!(bytes(&disabled), b"old");
        value.enabled = true; value.scope = "all".into(); value.rules[0].enabled = false;
        rewrite.configure(value.clone(), &rewrite.snapshot().version).unwrap();
        rewrite.apply(&mut disabled); assert_eq!(bytes(&disabled), b"old");
        value.rules[0].enabled = true; value.rules[0].match_type = "urlRegex".into(); value.rules[0].pattern = r"^https://example\.test/api\?x=1$".into();
        rewrite.configure(value, &rewrite.snapshot().version).unwrap();
        rewrite.apply(&mut disabled); assert_eq!(bytes(&disabled), b"new");
    }

    #[test]
    fn duplicate_header_operations_preserve_positions_and_first_or_all_semantics() {
        let (_directory, _store, rewrite) = setup();
        configure(&rewrite, json!([modify("heads", json!([
            {"target":"header","operation":"set","name":"x-test","value":"first","all":false},
            {"target":"header","operation":"replace","name":"X-Test","find":"alpha","value":"$1","regex":false,"all":true},
            {"target":"header","operation":"remove","name":"X-Remove","all":false},
            {"target":"header","operation":"set","name":"X-New","value":"added","all":true}
        ]))]));
        let mut flow = sample(b"body", vec![h("X-Test", "alpha one"),h("X-Middle", "keep"),h("x-test", "alpha two"),h("X-Remove", "one"),h("x-remove", "two")]);
        rewrite.apply(&mut flow);
        assert_eq!(header_values(&flow, "x-test"), ["first", "$1 two"]);
        assert_eq!(header_values(&flow, "x-remove"), ["two"]);
        assert_eq!(flow.response.as_ref().unwrap().headers.iter().map(|header| header.name.as_str()).collect::<Vec<_>>(),
            ["X-Test", "X-Middle", "x-test", "x-remove", "X-New", "Content-Length"]);
        assert_eq!(bytes(&flow), b"body");
    }

    #[test]
    fn regex_captures_literal_dollars_unicode_and_case_selection() {
        let (_directory, _store, rewrite) = setup();
        let mut replacement = body(r"(?P<word>hello) (?P<place>世界)", "${place}:$1:$$:$missing");
        replacement["regex"] = json!(true); replacement["caseSensitive"] = json!(false);
        configure(&rewrite, json!([modify("regex", json!([
            body("$1", "$2 $$"), replacement,
            {"target":"body","operation":"replace","find":"world","value":"earth","regex":false,"caseSensitive":false,"all":false}
        ]))]));
        let mut flow = sample("$1 HELLO 世界 WORLD world".as_bytes(), vec![]);
        rewrite.apply(&mut flow);
        assert_eq!(String::from_utf8(bytes(&flow)).unwrap(), "$2 $$ 世界:HELLO:$: earth world");
        // Keep Rust regex's expansion edge cases, while checking each append.
        let expression = Regex::new("(?P<name>x)").unwrap();
        let captures = expression.captures("x").unwrap();
        for template in ["$0/$1/${name}/$$/$missing", "${}/$/$!/${unterminated", "$9999999999999999999999999999", "${名字}"] {
            let mut expected = String::new(); captures.expand(template, &mut expected);
            let mut actual = String::new(); expand(&mut actual, template, &captures, 1024).unwrap();
            assert_eq!(actual, expected, "{template}");
        }
    }

    #[test]
    fn replacement_expansion_checks_small_budgets_before_copying() {
        let mut value = config(json!([modify("bounded", json!([body("a", "1234567")]))]));
        compile(&mut value).unwrap();
        assert!(replace_text("aaaa", &value.rules[0].modifications[0], 8).is_err());
        let expression = Regex::new("(abcdef)").unwrap();
        let captures = expression.captures("abcdef").unwrap();
        let mut expanded = String::new();
        assert!(expand(&mut expanded, "$1$1$1$1$1", &captures, 13).is_err());
        assert!(expanded.len() <= 13);
        let mut modification = body("(a)", "$1$1$1$1$1");
        modification["target"] = json!("header"); modification["name"] = json!("X-Value"); modification["regex"] = json!(true);
        let mut value = config(json!([modify("header-budget", json!([modification]))]));
        compile(&mut value).unwrap();
        let mut headers = vec![h("X-Filler", &"x".repeat(MAX_HEADERS - 64)), h("X-Value", "aaaaaaaaaa")];
        let original = headers.clone();
        assert!(modify_header(&mut headers, &value.rules[0].modifications[0]).is_err());
        assert_eq!(headers, original);
    }

    fn compress(coding: &str, input: &[u8]) -> Vec<u8> {
        match coding {
            "gzip" => { let mut writer = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()); writer.write_all(input).unwrap(); writer.finish().unwrap() }
            "deflate" => { let mut writer = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast()); writer.write_all(input).unwrap(); writer.finish().unwrap() }
            "raw" => { let mut writer = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast()); writer.write_all(input).unwrap(); writer.finish().unwrap() }
            "br" => { let mut bytes = Vec::new(); brotli::CompressorReader::new(input, 4096, 4, 22).read_to_end(&mut bytes).unwrap(); bytes }
            "zstd" => zstd::stream::encode_all(input, 1).unwrap(),
            _ => unreachable!(),
        }
    }
    #[test]
    fn compressed_text_edits_decode_in_reverse_order_and_invalidate_only_when_changed() {
        let (_directory, _store, rewrite) = setup();
        configure(&rewrite, json!([modify("compressed", json!([body("before", "after 世界")]))]));
        for coding in ["gzip", "deflate", "raw", "br", "zstd", "gzip, br"] {
            let encoded = if coding == "gzip, br" { compress("br", &compress("gzip", b"before")) } else { compress(coding, b"before") };
            let header_coding = if coding == "raw" { "deflate" } else { coding };
            let mut flow = sample(&encoded, vec![h("Content-Encoding", header_coding),h("ETag", "stale"),h("Digest", "stale"),h("Content-MD5", "stale"),h("Content-Length", &encoded.len().to_string())]);
            rewrite.apply(&mut flow);
            assert_eq!(bytes(&flow), "after 世界".as_bytes(), "{coding}: {:?}", flow.notes);
            assert!(header_values(&flow, "content-encoding").is_empty());
            assert!(header_values(&flow, "etag").is_empty() && header_values(&flow, "digest").is_empty());
            assert!(header_values(&flow, "content-md5").is_empty());
            assert_eq!(header_values(&flow, "content-length"), ["after 世界".len().to_string()]);
            assert_eq!(STANDARD.decode(&flow.original_response.as_ref().unwrap().body_base64).unwrap(), encoded);
        }
        let encoded = compress("gzip", b"no match");
        let mut no_change = sample(&encoded, vec![h("Content-Encoding", "gzip"),h("ETag", "keep")]);
        let original = serde_json::to_value(&no_change.response).unwrap();
        rewrite.apply(&mut no_change);
        assert_eq!(serde_json::to_value(&no_change.response).unwrap(), original);
        assert!(no_change.original_response.is_none());
    }

    #[test]
    fn decoding_errors_binary_partial_responses_and_expansion_limits_keep_exact_original() {
        let (_directory, _store, rewrite) = setup();
        configure(&rewrite, json!([modify("safe", json!([
            {"target":"header","operation":"set","name":"X-Rollback","value":"discard"},body("before", "after")
        ]))]));
        let mut cases = vec![sample(b"before", vec![h("Content-Encoding", "unknown")]), sample(&[255, 0, 128], vec![]),
            sample(b"not compressed", vec![h("Content-Encoding", "gzip")]), sample(b"before", vec![])];
        cases.last_mut().unwrap().response.as_mut().unwrap().status = 206;
        let inflated = compress("gzip", &vec![b'a'; MAX_BODY + 1]);
        cases.push(sample(&inflated, vec![h("Content-Encoding", "gzip")]));
        for mut flow in cases {
            let original = serde_json::to_value(&flow.response).unwrap();
            rewrite.apply(&mut flow);
            assert_eq!(serde_json::to_value(&flow.response).unwrap(), original);
            assert!(flow.original_response.is_none());
            assert!(flow.notes.iter().any(|note| note.contains("失败")));
        }
        let mut expansion = body("^(.*)$", &"$1".repeat(64));
        expansion["regex"] = json!(true); expansion["all"] = json!(false);
        configure(&rewrite, json!([modify("expanded", json!([expansion]))]));
        let mut flow = sample(&vec![b'x'; 128 * 1024 + 1], vec![]);
        let original = serde_json::to_value(&flow.response).unwrap();
        rewrite.apply(&mut flow);
        assert_eq!(serde_json::to_value(&flow.response).unwrap(), original);
        assert!(flow.notes.iter().any(|note| note.contains("预算")));
    }

    #[test]
    fn text_edits_reject_binary_nul_and_non_utf8_charset_but_header_edits_still_work() {
        let (_directory, _store, rewrite) = setup();
        configure(&rewrite, json!([modify("text-boundary", json!([body("before", "after 世界")]))]));
        for (mime, data) in [
            ("text/plain", &b"before\0binary"[..]),
            ("text/plain; charset=Shift_JIS", &b"before"[..]),
            ("application/json; charset=ISO-8859-1", &b"before"[..]),
            ("image/png", &b"before"[..]), ("audio/wav", &b"before"[..]),
            ("video/mp4", &b"before"[..]), ("application/pdf", &b"before"[..]),
        ] {
            let mut flow = sample(data, vec![h("Content-Type", mime)]);
            let original = serde_json::to_value(&flow.response).unwrap();
            rewrite.apply(&mut flow);
            assert_eq!(serde_json::to_value(&flow.response).unwrap(), original);
            assert!(flow.original_response.is_none());
            assert!(flow.notes.iter().any(|note| note.contains("失败")));
            assert!(flow.notes.iter().all(|note| note.chars().count() <= 600));
        }
        for mime in ["text/plain; charset=\"UTF-8\"", "application/json; charset=utf8", "image/svg+xml", "text/plain"] {
            let mut flow = sample(b"before", vec![h("Content-Type", mime)]);
            rewrite.apply(&mut flow);
            assert_eq!(bytes(&flow), "after 世界".as_bytes());
        }
        configure(&rewrite, json!([modify("headers-only", json!([
            {"target":"header","operation":"set","name":"X-Checked","value":"yes"}
        ]))]));
        let mut binary = sample(b"before\0binary", vec![h("Content-Type", "text/plain; charset=Shift_JIS")]);
        rewrite.apply(&mut binary);
        assert_eq!(header_values(&binary, "X-Checked"), ["yes"]);
        assert_eq!(bytes(&binary), b"before\0binary");
    }

    #[test]
    fn head_and_no_body_statuses_have_legal_framing_and_no_payload() {
        let (_directory, _store, rewrite) = setup();
        for (method, status, length) in [("HEAD", 200, Some("3")), ("GET", 204, None), ("GET", 205, Some("0")), ("GET", 304, None)] {
            configure(&rewrite, json!([replacement("empty", status, b"new", vec![h("Content-Length", "999"),h("Transfer-Encoding", "chunked")])]));
            let mut flow = sample(&[], vec![]); flow.request.method = method.into();
            rewrite.apply(&mut flow);
            assert!(bytes(&flow).is_empty());
            assert_eq!(header_values(&flow, "content-length"), length.into_iter().map(str::to_owned).collect::<Vec<_>>());
            assert!(header_values(&flow, "transfer-encoding").is_empty());
        }
        configure(&rewrite, json!([modify("head-only", json!([
            {"target":"header","operation":"set","name":"X-Head","value":"yes"}
        ]))]));
        let mut head = sample(&[], vec![h("Content-Length", "1234")]); head.request.method = "HEAD".into();
        // The fixture rule otherwise matches POST only.
        let mut value = rewrite.snapshot().config; value.rules[0].method.clear();
        rewrite.configure(value, &rewrite.snapshot().version).unwrap();
        rewrite.apply(&mut head);
        assert_eq!(header_values(&head, "Content-Length"), ["1234"]);
        assert!(bytes(&head).is_empty());
    }

    #[test]
    fn streaming_flows_are_not_rewritten_or_counted() {
        let (_directory, _store, rewrite) = setup();
        configure(&rewrite, json!([replacement("stream", 200, b"unsafe", vec![])]));
        for mime in ["text/event-stream; charset=utf-8", "application/grpc", "application/grpc+proto"] {
            let mut flow = sample(b"stream", vec![h("Content-Type", mime)]);
            rewrite.apply(&mut flow);
            assert_eq!(bytes(&flow), b"stream");
            assert!(flow.original_response.is_none());
            assert!(flow.notes.iter().any(|note| note.contains("已保留原响应")));
        }
        let mut websocket = sample(&[], vec![]); websocket.response.as_mut().unwrap().status = 101;
        rewrite.apply(&mut websocket);
        assert_eq!(websocket.response.as_ref().unwrap().status, 101);
        assert_eq!(rewrite.snapshot().hits, [0]);
    }

    #[test]
    fn config_cas_persistence_other_process_and_migration_epoch_are_enforced() {
        let (directory, store, rewrite) = setup();
        let other_store = Arc::new(Store::open(&directory.path().join("test.db")).unwrap());
        let other = ResponseRewrite::open(other_store).unwrap();
        let old = rewrite.snapshot().version;
        let value = config(json!([modify("saved", json!([body("a", "b")]))]));
        let saved = rewrite.configure(value.clone(), &old).unwrap();
        assert!(rewrite.configure(Config::default(), &old).is_err());
        assert!(other.configure(Config::default(), &old).is_err());
        let reopened = ResponseRewrite::open(store).unwrap();
        assert_eq!(reopened.snapshot().version, saved.version);
        assert_eq!(reopened.snapshot().hits, [0]);
        let mut flow = sample(b"a", vec![]); reopened.apply(&mut flow); assert_eq!(bytes(&flow), b"b");
        rusqlite::Connection::open(directory.path().join("test.db")).unwrap().execute(
            "INSERT INTO settings(key,value) VALUES('config-migration.epoch.v1','new-epoch') ON CONFLICT(key) DO UPDATE SET value=excluded.value", [],
        ).unwrap();
        assert!(reopened.configure(Config::default(), &saved.version).is_err());
        assert_eq!(reopened.snapshot().version, saved.version);
    }

    #[test]
    fn invalid_configs_cannot_replace_working_rules_or_mutate_framing_headers() {
        let (_directory, _store, rewrite) = setup();
        let old = rewrite.snapshot().version;
        let mut examples = vec![
            modify("bad", json!([{"target":"body","operation":"replace","find":"[","value":"x","regex":true}])),
            replacement("bad", 199, b"bad", vec![]),
            replacement("bad", 200, b"bad", vec![h("X-Test", "injected\r\nHeader: value")]),
            modify("bad", json!([body("", "x")])),
        ];
        for name in ["Content-Length", "transfer-encoding", "Connection", "TE", "Trailer", "Keep-Alive"] {
            examples.push(modify("bad", json!([{"target":"header","operation":"set","name":name,"value":"0"}])));
        }
        for example in examples { assert!(rewrite.configure(config(json!([example])), &old).is_err()); assert_eq!(rewrite.snapshot().version, old); }
        let rule = modify("one", json!([body("a", "b")]));
        assert!(validate_config(&config(json!([rule.clone(), rule.clone()]))).is_err());
        let mut too_many = Vec::new();
        for index in 0..33 { let mut value = rule.clone(); value["id"] = json!(index.to_string()); too_many.push(value); }
        assert!(validate_config(&config(json!(too_many))).is_err());
        let mut too_many = rule; too_many["modifications"] = json!(vec![body("a", "b"); 33]);
        assert!(validate_config(&config(json!([too_many]))).is_err());
    }

    #[test]
    fn header_control_bytes_are_rejected_before_sending_and_candidate_errors_roll_back() {
        let (_directory, _store, rewrite) = setup();
        for invalid in ["bad\u{1}", "bad\u{7f}", "bad\u{b}"] {
            let previous = rewrite.snapshot().version;
            assert!(rewrite.configure(config(json!([replacement("control", 200, b"body", vec![h("X-Test", invalid)])])), &previous).is_err());
            assert!(rewrite.configure(config(json!([modify("control", json!([
                {"target":"header","operation":"set","name":"X-Test","value":invalid}
            ]))])), &previous).is_err());
            configure(&rewrite, json!([modify("control", json!([
                {"target":"header","operation":"set","name":"X-First","value":"must roll back"},
                {"target":"header","operation":"replace","name":"X-Test","find":"old","value":invalid}
            ]))]));
            let mut flow = sample(b"body", vec![h("X-Test", "old")]);
            let original = serde_json::to_value(&flow.response).unwrap();
            rewrite.apply(&mut flow);
            assert_eq!(serde_json::to_value(&flow.response).unwrap(), original);
            assert!(flow.original_response.is_none());
            assert!(flow.notes.iter().any(|note| note.contains("失败")));
        }
        assert!(header_value("allowed\ttab\u{ff}").is_ok());
    }
}
