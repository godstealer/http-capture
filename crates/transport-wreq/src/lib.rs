//! Browser transport independent of capture, storage and UI.
use transport_api::*;
use anyhow::{ensure, Result};
use base64::{Engine,engine::general_purpose::STANDARD};
use std::time::Duration;
const MAX_BODY:usize=8*1024*1024;
fn latin1(bytes:&[u8])->String {bytes.iter().map(|b|char::from(*b)).collect()}
fn header_bytes(value:&str)->Result<Vec<u8>> {value.chars().map(|c|{ensure!((c as u32)<=255&&c!='\r'&&c!='\n'&&c!='\0',"Invalid header value");Ok(c as u8)}).collect()}
pub async fn send<S>(draft: &RequestDraft<S>, headers: Vec<Header>, body: Vec<u8>, proxy: Option<&UpstreamProxyConfig>) -> Result<(CapturedResponse, Vec<String>, Vec<Vec<u8>>)> {
    ensure!(draft.tls.version.is_none() || draft.tls.version.as_deref() == Some("auto"), "Explicit TLS version is currently supported only by native/auto/h2/h3 engines");
    use wreq::{header::{HeaderMap, HeaderName, HeaderValue, OrigHeaderMap}, IntoEmulation};
    use futures_util::StreamExt;
    let url = url::Url::parse(&draft.url)?;
    let mut notes = Vec::new();
    let (browser_version, preset) = browser_profiles::resolve(draft)?;
    notes.push(format!("TLS/HTTP2 profile: {} {}; User-Agent preserved", draft.tls.preset, browser_version));
    let mut emulation = preset.into_emulation();
    // Applying a TLS preset must never overwrite the user's HTTP headers.
    emulation.headers = HeaderMap::new();
    emulation.orig_headers = OrigHeaderMap::new();
    let tls = emulation.tls_options.get_or_insert_with(Default::default);
    if let Some(v) = &draft.tls.cipher_list { tls.cipher_list = Some(v.clone().into()); }
    if let Some(v) = &draft.tls.sigalgs_list { tls.sigalgs_list = Some(v.clone().into()); }
    if let Some(v) = &draft.tls.curves_list { tls.curves_list = Some(v.clone().into()); }
    if let Some(v) = draft.tls.grease { tls.grease_enabled = Some(v); }
    if let Some(v) = draft.tls.permute_extensions { tls.permute_extensions = Some(v); }
    let mut original = OrigHeaderMap::new();
    let mut map = HeaderMap::new();
    for h in headers {
        let name = HeaderName::from_bytes(h.name.as_bytes())?;
        original.insert(h.name);
        map.append(name, HeaderValue::from_bytes(&header_bytes(&h.value)?)?);
    }
    // A fresh client per replay guarantees a new ClientHello and no cookie/session leakage.
    let mut builder = wreq::Client::builder().emulation(emulation).no_proxy();
    if let Some(p)=proxy {
        let proxy_url=if p.url.starts_with("socks5://") {p.url.replacen("socks5://","socks5h://",1)}else{p.url.clone()};
        let mut configured=wreq::Proxy::all(proxy_url)?;
        if !p.username.is_empty(){configured=configured.basic_auth(&p.username,&p.password);}
        builder=builder.proxy(configured);
    }
    let client = builder
        .tls_info(true).orig_headers(original).redirect(wreq::redirect::Policy::none())
        .timeout(Duration::from_secs(30)).build()?;
    let response = client.request(wreq::Method::from_bytes(draft.method.as_bytes())?, url.as_str())
        .headers(map).body(body).send().await?;
    let status = response.status().as_u16();
    let version = format!("{:?}", response.version());
    let chain = response.extensions().get::<wreq::tls::TlsInfo>().map(|info| {
        info.peer_certificate_chain().map(|chain| chain.map(|der| der.to_vec()).collect::<Vec<_>>()).unwrap_or_else(|| info.peer_certificate().map(|der| vec![der.to_vec()]).unwrap_or_default())
    }).unwrap_or_default();

    let headers = response.headers().iter().map(|(n, v)| Header { name: n.as_str().into(), value: latin1(v.as_bytes()) }).collect();
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk?;
        ensure!(body.len() + chunk.len() <= MAX_BODY, "Response exceeds 8 MiB limit");
        body.extend_from_slice(&chunk);
    }
    notes.push("浏览器预设使用独立连接；响应头来自客户端解析结果，不宣称原始响应字段顺序。HTTP/2 字段名按协议小写。".into());
    notes.push("浏览器发送的线上头部顺序及 ClientHello 仍需抓取验证；当前不标记为已认证保真。".into());
    Ok((CapturedResponse { upstream_tls: None, sent_request_headers: None, tls_version: None, status, version, headers, body_base64: STANDARD.encode(body), raw_head_base64: None }, notes, chain))
}
