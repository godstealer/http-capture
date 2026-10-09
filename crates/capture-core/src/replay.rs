use crate::{http1::*, model::*};
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::sync::Arc;
use tokio::{io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader}};
use tokio_rustls::{rustls, TlsConnector};

pub trait IoStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> IoStream for T {}
pub type Stream = Box<dyn IoStream>;

pub async fn send(draft: &RequestDraft) -> Result<(CapturedResponse, Vec<String>)> {
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    send_with_roots(draft, &roots).await
}
pub async fn send_with_roots(draft: &RequestDraft, roots: &rustls::RootCertStore) -> Result<(CapturedResponse, Vec<String>)> {
    crate::transport::SendEngines::with_roots(roots.clone()).send(draft).await
}

pub(crate) async fn native(draft: &RequestDraft, roots: &rustls::RootCertStore) -> Result<(CapturedResponse, Vec<String>)> {
    native_via(draft, roots, None).await
}
pub(crate) async fn native_via(draft: &RequestDraft, roots: &rustls::RootCertStore, proxy: Option<&crate::upstream::UpstreamProxy>) -> Result<(CapturedResponse, Vec<String>)> {
    let config = crate::tls_config::config(&draft.tls, roots, &draft.engine)?;
    let (url, headers, body, mut notes) = prepare(draft)?;
    let connection = crate::upstream::connect(&url, proxy).await?;
    let tcp = connection.stream;
    let mut tls_version = None;
    let mut upstream_tls = None;
    let mut stream: Stream = if url.scheme() == "https" {
        let name = rustls::pki_types::ServerName::try_from(hostname(&url))?;
        let tls = TlsConnector::from(Arc::new(config)).connect(name, tcp).await?;
        let mut details = crate::tls_details::negotiated(tls.get_ref().1);
        details.server_name = Some(hostname(&url));
        upstream_tls = Some(details);
        tls_version = tls.get_ref().1.protocol_version().map(|v| format!("{v:?}").replace("TLSv", "TLS ").replace('_', "."));
        if tls.get_ref().1.alpn_protocol() == Some(b"h2") {
            let (mut response, notes) = crate::multiplex::h2_exchange(draft, tls).await?;
            response.tls_version = tls_version;
            response.upstream_tls = upstream_tls;
            return Ok((response, notes));
        }
        Box::new(tls)
    } else { tcp };
    let proxy_authenticated = connection.absolute_form && connection.authorization.is_some();
    let mut wire = encode_request(&draft.method, &url, &headers, &body)?;
    if connection.absolute_form {
        let end = wire.windows(2).position(|b| b == b"\r\n").unwrap();
        let mut absolute = format!("{} {} HTTP/1.1\r\n", draft.method, url.as_str()).into_bytes();
        if let Some(auth) = connection.authorization { absolute.extend_from_slice(format!("Proxy-Authorization: {auth}\r\n").as_bytes()); }
        absolute.extend_from_slice(&wire[end + 2..]); wire = absolute;
        notes.push("HTTP 请求使用 absolute-form 发送给上游代理；代理认证仅发送到上游，不保存在请求记录中。".into());
    }
    stream.write_all(&wire).await?;
    stream.flush().await?;
    let mut sent_request_headers = headers.clone();
    if proxy_authenticated { sent_request_headers.insert(0, Header { name: "Proxy-Authorization".into(), value: "[redacted]".into() }); }
    sent_request_headers.push(Header { name: "Connection".into(), value: "close".into() });
    notes.push("native 模式保留端到端头部的顺序、大小写和重复字段；连接使用 HTTP/1.1，追加 Connection: close。".into());
    let mut reader = BufReader::new(stream);
    for _ in 0..16 {
        let raw = read_head(&mut reader).await?;
        let (status, version, headers) = parse_response(&raw)?;
        ensure!(status != 101, "WebSocket / protocol upgrade is not supported yet");
        if status < 200 { continue; }
        let no_body = draft.method == "HEAD" || status == 204 || status == 304;
        let streaming = !no_body && crate::sse::head(&CapturedResponse { upstream_tls: upstream_tls.clone(), sent_request_headers: Some(sent_request_headers.clone()), tls_version: tls_version.clone(), status, version: version.clone(), headers: headers.clone(), body_base64: String::new(), raw_head_base64: Some(STANDARD.encode(&raw)) }).await?;
        let (body, trailers) = if no_body { (Vec::new(), false) } else { read_body_observed(&mut reader, &headers, true, streaming).await? };
        ensure!(!trailers, "Response trailers are not supported yet; refusing to silently discard them");
        return Ok((CapturedResponse { upstream_tls, sent_request_headers: Some(sent_request_headers), tls_version, status, version, headers, body_base64: STANDARD.encode(body), raw_head_base64: Some(STANDARD.encode(raw)) }, notes));
    }
    anyhow::bail!("Too many interim responses")
}

#[cfg(not(feature = "browser-replay"))]
pub(crate) async fn browser_via(_: &RequestDraft, _: Option<&crate::upstream::UpstreamProxy>) -> Result<(CapturedResponse, Vec<String>)> {
    anyhow::bail!("This build does not include browser-replay")
}

#[cfg(feature = "browser-replay")]
pub(crate) async fn browser_via(draft: &RequestDraft, proxy: Option<&crate::upstream::UpstreamProxy>) -> Result<(CapturedResponse, Vec<String>)> {
    let (url,headers,body,mut notes)=prepare(draft)?;
    ensure_browser_header_order(&headers)?;
    let proxy_config=match proxy {Some(p)=>Some(p.for_helper().await?),None=>None};
    let (mut response,extra,chain)=transport_wreq::send(draft,headers,body,proxy_config.as_ref()).await?;
    if !chain.is_empty(){let chain:Vec<_>=chain.into_iter().map(rustls::pki_types::CertificateDer::from).collect();response.upstream_tls=Some(TlsDetails{server_name:url.host_str().map(str::to_owned),certificates:crate::tls_details::certificates(&chain),..Default::default()});}
    notes.extend(extra);Ok((response,notes))
}
