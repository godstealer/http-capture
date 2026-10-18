//! HTTP/2 records decoded HPACK field order using the vendored h2 extension.
use crate::{body_spool::BodySpool, http1::*, model::*};
use anyhow::{ensure, Result, Context};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use bytes::{Buf, Bytes};
use std::sync::Arc;

pub const HEADER_NOTE: &str = "HTTP/3 尚未记录 QPACK 字段顺序；此列表仅为解析结果。";

pub async fn h2_capture<T>(io: T, engine: Arc<crate::Engine>, origin: url::Url) -> Result<()>
where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin {
    h2_capture_with_tls(io, engine, origin, None).await
}
pub async fn h2_capture_with_tls<T>(io: T, engine: Arc<crate::Engine>, origin: url::Url, client_tls: Option<TlsDetails>) -> Result<()>
where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin {
    h2_capture_with_upstream(io, engine, origin, client_tls, None).await
}
pub(crate) async fn h2_capture_with_upstream<T>(io: T, engine: Arc<crate::Engine>, origin: url::Url, client_tls: Option<TlsDetails>, upstream_tls: Option<Arc<crate::capture_tls::CaptureTls>>) -> Result<()>
where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin {
    let mut connection = h2::server::Builder::new().max_concurrent_streams(32).max_header_list_size(MAX_HEAD as u32).handshake::<_, Bytes>(io).await?;
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            item = connection.accept() => match item {
                Some(Ok((request, mut respond))) => {
                    let engine = engine.clone(); let origin = origin.clone(); let client_tls = client_tls.clone(); let upstream_tls = upstream_tls.clone();
                    let traffic_origin = crate::traffic_origin::current("capture");
                    tasks.spawn(crate::traffic_origin::scope(traffic_origin, async move {
                        let result: Result<()> = async {
                            let (parts, mut recv) = request.into_parts();
                            ensure!(parts.method != http::Method::CONNECT, "Extended CONNECT is unsupported");
                            if let Some(authority) = parts.uri.authority() { ensure!(authority.as_str() == crate::http1::authority(&origin), "Authority differs from CONNECT target"); }
                            let path = parts.uri.path_and_query().map(|v| v.as_str()).unwrap_or("/");
                            let target = format!("https://{}{path}", crate::http1::authority(&origin));
                            let ordered = ordered_headers(&parts.headers, &parts.extensions, Some(&parts.uri), Some(&parts.method))?;
                            let (pseudo_headers, fields): (Vec<_>, Vec<_>) = ordered.into_iter().partition(|h| h.name.starts_with(':'));
                            let body = recv_h2_body_spooled(&mut recv).await?;
                            ensure!(recv.trailers().await?.is_none(), "Request trailers are not supported yet");
                            // Downstream h2 does not imply upstream h2 support. Negotiate each leg independently.
                            let draft = RequestDraft { upstream_profile_id: None, scripts: Default::default(), pseudo_headers, engine: "auto".into(), method: parts.method.to_string(), url: target,
                                headers: fields, body_base64: body.into_base64().await?.into(), tls: TlsProfile::default() };
                            let (tx, mut rx) = tokio::sync::mpsc::channel(8);
                            let work = crate::sse::DOWNSTREAM.scope(tx, crate::capture_tls::scope(upstream_tls, engine.execute_with_tls(draft, None, "capture", None, Some("HTTP/2".into()), client_tls)));
                            tokio::pin!(work);
                            let mut outgoing = None;
                            let mut completed: Option<Result<Flow>> = None;
                            let flow = loop {
                                if completed.is_some() && rx.is_empty() { break completed.take().unwrap()?; }
                                tokio::select! {
                                    biased;
                                    Some(event) = rx.recv() => match event {
                                        crate::sse::Event::Head(response) => {
                                            let mut head = http::Response::builder().status(response.status).body(())?;
                                            for field in end_to_end(&response.headers).into_iter().filter(|h| !h.name.eq_ignore_ascii_case("content-length")) {
                                                head.headers_mut().append(http::header::HeaderName::from_bytes(field.name.as_bytes())?, http::HeaderValue::from_bytes(&header_bytes(&field.value)?)?);
                                            }
                                            outgoing = Some(respond.send_response(head, false)?);
                                        }
                                        crate::sse::Event::Data(data) => if let Some(send) = &mut outgoing { send_h2_chunk(send, Bytes::from(data), false).await?; },
                                    },
                                    result = &mut work, if completed.is_none() => completed = Some(result),
                                }
                            };
                            if let Some(mut send) = outgoing {
                                if flow.error.is_some() { send.send_reset(h2::Reason::INTERNAL_ERROR); }
                                else { send.send_data(Bytes::new(), true)?; }
                                return Ok(());
                            }
                            let (response, mut body) = response_head_for_flow(flow).await?;
                            let (mut parts, ()) = response.into_parts();
                            let trailers=parts.extensions.remove::<http::HeaderMap>();
                            let mut send = respond.send_response(http::Response::from_parts(parts, ()), body.len == 0 && trailers.is_none())?;
                            let mut remaining = body.len;
                            while remaining > 0 {
                                let chunk = body.next_chunk().await?;
                                ensure!(!chunk.is_empty() && chunk.len() as u64 <= remaining, "Incomplete temporary response body");
                                remaining -= chunk.len() as u64;
                                send_h2_chunk(&mut send, Bytes::from(chunk), remaining == 0 && trailers.is_none()).await?;
                            }
                            if let Some(trailers)=trailers {send.send_trailers(trailers)?;}
                            Ok(())
                        }.await;
                        if let Err(error) = result { tracing::warn!(%error, "h2 stream failed"); respond.send_reset(h2::Reason::INTERNAL_ERROR); }
                    }));
                }
                Some(Err(error)) => return Err(error.into()),
                None => break,
            },
            _ = tasks.join_next(), if !tasks.is_empty() => {},
        }
    }
    Ok(())
}

pub fn response_for_flow(flow: Flow) -> Result<http::Response<Vec<u8>>> {
    if let Some(error) = flow.error {
        let body = if flow.request.method == "HEAD" { Vec::new() } else { error.into_bytes() };
        return Ok(http::Response::builder().status(502).body(body)?);
    }
    let response = flow.response.ok_or_else(|| anyhow::anyhow!("Missing response"))?;
    let output = response_head(&response)?;
    let body = if response_has_body(&flow.request.method, response.status) { STANDARD.decode(&response.body_base64)? } else { Vec::new() };
    Ok(output.map(|()| body))
}

/// Capture forwarding uses the validated, file-backed body instead of rebuilding
/// an entire decoded Vec. Public JSON continues to contain the same Base64 text.
pub(crate) async fn response_head_for_flow(flow: Flow) -> Result<(http::Response<()>, BodySpool)> {
    if let Some(error) = flow.error {
        let body = if flow.request.method == "HEAD" { BodySpool::empty() } else { BodySpool::decode(&STANDARD.encode(error.as_bytes())).await? };
        return Ok((http::Response::builder().status(502).body(())?, body));
    }
    let response = flow.response.ok_or_else(|| anyhow::anyhow!("Missing response"))?;
    let output = response_head(&response)?;
    let body = if response_has_body(&flow.request.method, response.status) { BodySpool::decode(&response.body_base64).await? } else { BodySpool::empty() };
    Ok((output, body))
}

#[cfg(test)]
mod response_spool_tests {
    use super::*;

    fn flow(method: &str, status: u16, body: &str) -> Flow {
        serde_json::from_value(serde_json::json!({
            "id":"response-spool", "startedAt":0, "durationMs":0, "source":"capture",
            "request":{"method":method,"url":"https://example.invalid/","headers":[],"bodyBase64":""},
            "response":{"status":status,"version":"HTTP/2","headers":[
                {"name":"Content-Length","value":"12345"},
                {"name":"X-Repeat","value":"first"}, {"name":"X-Repeat","value":"second"}
            ],"trailers":[{"name":"X-Trailer","value":"done"}],"bodyBase64":body},
            "error":null,"notes":[]
        })).unwrap()
    }

    #[tokio::test]
    async fn response_spool_preserves_binary_headers_and_trailers() {
        let bytes: Vec<_> = (0..128*1024+2).map(|index| (index%251) as u8).collect();
        let original = flow("GET", 200, &STANDARD.encode(&bytes));
        let compatibility = response_for_flow(original.clone()).unwrap();
        let (response, mut body) = response_head_for_flow(original).await.unwrap();
        assert_eq!(response.status(), compatibility.status());
        assert_eq!(response.headers(), compatibility.headers());
        assert_eq!(response.headers().get_all("x-repeat").iter().count(), 2);
        assert_eq!(response.extensions().get::<http::HeaderMap>().unwrap()["x-trailer"], "done");
        assert_eq!(body.len, bytes.len() as u64);
        let mut output = Vec::new();
        while output.len() < bytes.len() {
            let chunk = body.next_chunk().await.unwrap();
            assert!(!chunk.is_empty() && chunk.len() <= 64*1024);
            output.extend_from_slice(&chunk);
        }
        assert!(body.next_chunk().await.unwrap().is_empty());
        assert_eq!(output, bytes);
        assert_eq!(compatibility.body(), &bytes);
    }

    #[tokio::test]
    async fn head_no_content_and_not_modified_keep_headers_without_payload() {
        for (method, status) in [("HEAD", 200), ("GET", 204), ("GET", 304)] {
            let mut original = flow(method, status, "YWJj");
            // A 204 has no representation length; HEAD and 304 may retain it.
            if status == 204 { original.response.as_mut().unwrap().headers.retain(|header| !header.name.eq_ignore_ascii_case("content-length")); }
            let compatibility = response_for_flow(original.clone()).unwrap();
            let (response, mut body) = response_head_for_flow(original).await.unwrap();
            assert_eq!(response.status().as_u16(), status);
            if status == 204 { assert!(!response.headers().contains_key("content-length")); }
            else { assert_eq!(response.headers()["content-length"], "12345"); }
            assert_eq!(body.len, 0);
            assert!(body.next_chunk().await.unwrap().is_empty());
            assert!(compatibility.body().is_empty());
        }
    }

    #[tokio::test]
    async fn failed_flow_is_502_and_invalid_or_missing_response_is_rejected() {
        let mut failed = flow("GET", 200, "!");
        failed.error = Some("origin failed".into());
        let compatibility = response_for_flow(failed.clone()).unwrap();
        let (response, mut body) = response_head_for_flow(failed).await.unwrap();
        assert_eq!(response.status(), 502);
        assert!(response.extensions().get::<http::HeaderMap>().is_none());
        assert_eq!(body.next_chunk().await.unwrap(), b"origin failed");
        assert_eq!(compatibility.body(), b"origin failed");
        let mut failed_head = flow("HEAD", 200, "!");
        failed_head.error = Some("origin failed".into());
        assert!(response_for_flow(failed_head.clone()).unwrap().body().is_empty());
        let (head, mut body) = response_head_for_flow(failed_head).await.unwrap();
        assert_eq!(head.status(), 502);
        assert_eq!(body.len, 0);
        assert!(body.next_chunk().await.unwrap().is_empty());
        assert!(response_head_for_flow(flow("GET", 200, "!")).await.is_err());
        let mut missing = flow("GET", 200, ""); missing.response = None;
        assert!(response_head_for_flow(missing).await.is_err());
    }

    #[test]
    fn declared_content_length_requires_one_decimal_u64() {
        let mut headers = http::HeaderMap::new();
        assert_eq!(declared_body_length(&headers).unwrap(), None);
        for (value, expected) in [("0", 0), ("0012", 12), ("18446744073709551615", u64::MAX)] {
            headers.insert(http::header::CONTENT_LENGTH, http::HeaderValue::from_static(value));
            assert_eq!(declared_body_length(&headers).unwrap(), Some(expected));
        }
        for invalid in ["", "+1", "-1", "1.0", "1,1", "1 2", " 1", "1 ", "18446744073709551616"] {
            headers.insert(http::header::CONTENT_LENGTH, http::HeaderValue::from_static(invalid));
            assert!(declared_body_length(&headers).is_err(), "accepted {invalid:?}");
        }
        headers.insert(http::header::CONTENT_LENGTH, http::HeaderValue::from_static("4"));
        headers.append(http::header::CONTENT_LENGTH, http::HeaderValue::from_static("4"));
        assert!(declared_body_length(&headers).is_err());
    }

    #[test]
    fn body_length_rejects_overflow_and_incomplete_fin() {
        assert!(check_body_length(None, u64::MAX, true).is_ok());
        assert!(check_body_length(Some(3), 2, false).is_ok());
        assert!(check_body_length(Some(3), 3, false).is_ok());
        assert!(check_body_length(Some(3), 3, true).is_ok());
        assert!(check_body_length(Some(3), 4, false).is_err());
        assert!(check_body_length(Some(3), 4, true).is_err());
        assert!(check_body_length(Some(3), 2, true).is_err());
        assert!(check_body_length(Some(0), 0, true).is_ok());
        assert!(check_body_length(Some(0), 1, false).is_err());
        for (method, status) in [("HEAD", 200), ("GET", 101), ("GET", 204), ("GET", 304)] {
            let expected = if response_has_body(method, status) { Some(12345) } else { Some(0) };
            assert!(check_body_length(expected, 0, true).is_ok());
            assert!(check_body_length(expected, 1, false).is_err());
        }
    }
}

fn response_has_body(method: &str, status: u16) -> bool {
    method != "HEAD" && !(100..200).contains(&status) && !matches!(status, 204 | 304)
}

/// H3 does not validate message length for us. Reuse the same strict parser for
/// upstream responses and captured requests, without accepting ambiguous values.
pub(crate) fn declared_body_length(headers: &http::HeaderMap) -> Result<Option<u64>> {
    let mut values = headers.get_all(http::header::CONTENT_LENGTH).iter();
    let Some(value) = values.next() else { return Ok(None); };
    ensure!(values.next().is_none(), "Duplicate Content-Length is not accepted");
    let bytes = value.as_bytes();
    ensure!(!bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit), "Invalid Content-Length");
    let length = value.to_str()?.parse::<u64>().context("Content-Length exceeds u64 range")?;
    Ok(Some(length))
}

pub(crate) fn check_body_length(expected: Option<u64>, received: u64, complete: bool) -> Result<()> {
    if let Some(expected) = expected {
        ensure!(received <= expected, "Body exceeds Content-Length: expected {expected}, received {received}");
        ensure!(!complete || received == expected, "Incomplete body: Content-Length {expected}, received {received}");
    }
    Ok(())
}

fn response_head(response: &CapturedResponse) -> Result<http::Response<()>> {
    let mut output = http::Response::builder().status(response.status).body(())?;
    for field in end_to_end(&response.headers) {
        output.headers_mut().append(http::header::HeaderName::from_bytes(field.name.as_bytes())?, http::HeaderValue::from_bytes(&header_bytes(&field.value)?)?);
    }
    if !response.trailers.is_empty() {
        let mut trailers=http::HeaderMap::new();
        for field in response.trailers.iter().filter(|h| !h.disabled) { trailers.append(http::header::HeaderName::from_bytes(field.name.as_bytes())?,http::HeaderValue::from_bytes(&header_bytes(&field.value)?)?); }
        output.extensions_mut().insert(trailers);
    }
    Ok(output)
}

pub fn headers(map: &http::HeaderMap) -> Vec<Header> {
    map.iter().map(|(name, value)| Header { disabled: false, name: name.to_string(), value: value.as_bytes().iter().map(|b| *b as char).collect() }).collect()
}

fn ordered_headers(map: &http::HeaderMap, extensions: &http::Extensions, uri: Option<&http::Uri>, method: Option<&http::Method>) -> Result<Vec<Header>> {
    let order = extensions.get::<h2::ext::HeaderOrder>().ok_or_else(|| anyhow::anyhow!("Missing decoded HTTP/2 header order"))?;
    let mut used = std::collections::HashMap::<&str, usize>::new();
    let mut fields = Vec::new();
    for name in &order.0 {
        let value = if name.starts_with(':') {
            match name.as_str() {
                ":method" => method.map(|m| m.as_str()).unwrap_or("").to_owned(),
                ":scheme" => uri.and_then(|u| u.scheme_str()).unwrap_or("").into(),
                ":authority" => uri.and_then(|u| u.authority()).map(|a| a.as_str()).unwrap_or("").into(),
                ":path" => uri.and_then(|u| u.path_and_query()).map(|p| p.as_str()).unwrap_or("").into(),
                _ => continue,
            }
        } else {
            let index = used.entry(name.as_str()).or_default();
            let value = map.get_all(name).iter().nth(*index).ok_or_else(|| anyhow::anyhow!("HTTP/2 field order mismatch"))?;
            *index += 1;
            latin1(value.as_bytes())
        };
        fields.push(Header { disabled: false, name: name.clone(), value });
    }
    Ok(fields)
}

pub(crate) async fn request_h2(draft: &RequestDraft) -> Result<(http::Request<()>, crate::body_spool::BodySpool, Vec<String>, Vec<Header>)> {

    // Reuse framing/URL validation, but do not inject H1-only Host/length fields.
    let body=crate::body_spool::BodySpool::decode(&draft.body_base64).await?;
    let (url, _, _) = prepare_fields(draft,body.len)?;
    ensure!(matches!(url.scheme(), "https" | "http"), "h2 requires HTTP or HTTPS");
    let mut request = http::Request::builder().method(draft.method.as_str()).uri(url.as_str()).body(())?;
    let pseudo_names: Vec<&str> = if draft.pseudo_headers.is_empty() { vec![":method", ":scheme", ":authority", ":path"] }
        else { draft.pseudo_headers.iter().filter(|h| !h.disabled).map(|h| h.name.as_str()).collect() };
    let mut fields = Vec::new();
    for name in pseudo_names {
        let value = match name {
            ":method" => draft.method.clone(), ":scheme" => url.scheme().into(),
            ":authority" => authority(&url), ":path" => url[url::Position::BeforePath..url::Position::AfterQuery].into(),
            _ => anyhow::bail!("Unsupported HTTP/2 pseudo header: {name}"),
        };
        ensure!(!fields.iter().any(|h: &Header| h.name == name), "Repeated HTTP/2 pseudo header");
        fields.push(Header { disabled: false, name: name.into(), value });
    }
    // The h2 URI encoder supplies these fields. Never silently invent one for a captured request.
    ensure!([":method", ":scheme", ":path"].iter().all(|n| fields.iter().any(|h| h.name == *n)), "Missing required HTTP/2 pseudo header");
    ensure!(fields.iter().any(|h| h.name == ":authority") || draft.headers.iter().any(|h| !h.disabled && h.name.eq_ignore_ascii_case("host")), "Missing HTTP/2 authority/Host");
    let retained = end_to_end(&draft.headers);
    let mut notes = vec!["HTTP/2 按客户端/编辑列表的字段顺序编码，包含伪头部和交错重复字段；HPACK 压缩字节、帧边界及 TLS 握手不作原样复制。".into()];
    for original in draft.headers.iter().filter(|h| !h.disabled) {
        let name = original.name.to_ascii_lowercase();
        if name == "expect" || (!retained.contains(original) && !(name == "te" && original.value == "trailers")) {
            notes.push(format!("移除字段 {}。", original.name)); continue;
        }
        let value = if name == "host" { authority(&url) } else if name == "content-length" { body.len.to_string() } else { original.value.clone() };
        if value != original.value { notes.push(format!("{} 按目标/正文修正，位置不变。", original.name)); }
        request.headers_mut().append(http::header::HeaderName::from_bytes(name.as_bytes())?, http::HeaderValue::from_bytes(&header_bytes(&value)?)?);
        fields.push(Header { disabled: false, name, value });
    }
    request.extensions_mut().insert(h2::ext::HeaderOrder(fields.iter().map(|h| h.name.clone()).collect()));
    Ok((request, body, notes, fields))
}
async fn request(draft: &RequestDraft) -> Result<(http::Request<()>, crate::body_spool::BodySpool, Vec<String>)> {

    let body=crate::body_spool::BodySpool::decode(&draft.body_base64).await?;
    let (url, fields, mut notes) = prepare_fields(draft,body.len)?;
    ensure!(url.scheme() == "https", "h2/h3 require HTTPS");
    let mut request = http::Request::builder().method(draft.method.as_str()).uri(url.as_str()).body(())?;
    for field in fields {
        if field.name.eq_ignore_ascii_case("host") { continue; }
        request.headers_mut().append(http::header::HeaderName::from_bytes(field.name.as_bytes())?, http::HeaderValue::from_bytes(&header_bytes(&field.value)?)?);
    }
    notes.push(HEADER_NOTE.into());
    Ok((request, body, notes))
}
struct Abort(tokio::task::JoinHandle<()>);
impl Drop for Abort { fn drop(&mut self) { self.0.abort(); } }

#[cfg(test)]
mod quic_connection_tests {
    use super::*;

    #[tokio::test]
    async fn quic_rejects_unknown_ca_and_wrong_hostname() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let directory=tempfile::tempdir().unwrap();
            let _=rustls::crypto::ring::default_provider().install_default();
            let ca=crate::ca::CertificateAuthority::load_or_create(directory.path()).unwrap();
            for trust_ca in [false,true] {
                let mut tls=(*ca.server_config("localhost").unwrap()).clone(); tls.alpn_protocols=vec![b"h3".to_vec()];
                let server=quinn::Endpoint::server(quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap())),"127.0.0.1:0".parse().unwrap()).unwrap();
                let mut roots=rustls::RootCertStore::empty();
                if trust_ca {
                    let pem=std::fs::read(&ca.cert_path).unwrap();
                    for cert in rustls_pemfile::certs(&mut &pem[..]) { roots.add(cert.unwrap()).unwrap(); }
                }
                let tls=crate::tls_config::config(&TlsProfile::default(),&roots,"h3").unwrap();
                let client=connect_quic(if trust_ca {"wrong.invalid"} else {"localhost"},vec![server.local_addr().unwrap()],quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap())));
                let accepting=async { let _=server.accept().await.unwrap().await; };
                tokio::pin!(accepting);
                let result=tokio::select! { result=client => result, _=&mut accepting => panic!("server finished before certificate rejection") };
                let error=format!("{:#}",result.err().expect("untrusted certificate must fail"));
                assert!(error.contains("invalid peer certificate"),"{error}");
            }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn unreachable_first_address_does_not_block_reachable_address() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let directory = tempfile::tempdir().unwrap();
            let _ = rustls::crypto::ring::default_provider().install_default();
            let ca = crate::ca::CertificateAuthority::load_or_create(directory.path()).unwrap();
            let mut server_tls = (*ca.server_config("localhost").unwrap()).clone();
            server_tls.alpn_protocols = vec![b"h3".to_vec()];
            let server = quinn::Endpoint::server(quinn::ServerConfig::with_crypto(Arc::new(
                quinn::crypto::rustls::QuicServerConfig::try_from(server_tls).unwrap()
            )), "127.0.0.1:0".parse().unwrap()).unwrap();
            let address = server.local_addr().unwrap();
            // A bound socket that never replies models an unreachable QUIC path.
            let blackhole = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            let mut roots = rustls::RootCertStore::empty();
            let pem = std::fs::read(&ca.cert_path).unwrap();
            for cert in rustls_pemfile::certs(&mut &pem[..]) {
                roots.add(cert.unwrap()).unwrap();
            }
            let tls = crate::tls_config::config(&TlsProfile::default(), &roots, "h3").unwrap();
            let client = connect_quic("localhost", vec![blackhole.local_addr().unwrap(), address],
                quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap())));
            let accepting = async { server.accept().await.unwrap().await.unwrap() };
            let (connected, peer) = tokio::join!(client, accepting);
            let (_endpoint, connection) = connected.unwrap();
            assert_eq!(connection.remote_address(), address);
            connection.close(0u32.into(), b"test complete");
            drop(peer);
        }).await.unwrap();
    }
}

pub async fn h2_send(draft: &RequestDraft, roots: &rustls::RootCertStore) -> Result<(CapturedResponse, Vec<String>)> {
    h2_send_via(draft, roots, None).await
}
pub async fn h2_send_via(draft: &RequestDraft, roots: &rustls::RootCertStore, proxy: Option<&crate::upstream::UpstreamProxy>) -> Result<(CapturedResponse, Vec<String>)> {
    request_h2(draft).await?;
    let url = validate_url(&draft.url)?;
    if url.scheme() == "http" {
        let tcp = crate::upstream::connect(&url, proxy).await?;
        ensure!(!tcp.absolute_form, "h2c requires a direct connection or SOCKS5 tunnel");
        return h2_exchange(draft, tcp.stream).await;
    }
    let config = crate::tls_config::config(&draft.tls, roots, "h2")?;
    let tcp = crate::upstream::connect(&url, proxy).await?.stream;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(rustls::pki_types::ServerName::try_from(hostname(&url))?, tcp).await?;
    ensure!(tls.get_ref().1.alpn_protocol() == Some(b"h2"),
        "Server did not negotiate h2 (ALPN: {}). The selected h2 engine requires HTTP/2; select Auto to allow HTTP/1.1.",
        tls.get_ref().1.alpn_protocol().map(|p| String::from_utf8_lossy(p).into_owned()).unwrap_or_else(|| "none".into()));
    let mut details = crate::tls_details::negotiated(tls.get_ref().1);
    details.server_name = Some(hostname(&url));
    let version = tls.get_ref().1.protocol_version().map(|v| format!("{v:?}").replace("TLSv", "TLS ").replace('_', "."));
    let (mut response, notes) = h2_exchange(draft, tls).await?;
    response.tls_version = version;
    response.upstream_tls = Some(details);
    Ok((response, notes))
}
pub async fn h2_exchange<T>(draft: &RequestDraft, tls: T) -> Result<(CapturedResponse, Vec<String>)>
where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static {
    let (request, mut body, notes, sent_fields) = request_h2(draft).await?;
    let (sender, connection) = h2::client::Builder::new().max_header_list_size(MAX_HEAD as u32).enable_push(false).handshake::<_, Bytes>(tls).await?;
    let _driver = Abort(tokio::spawn(async move { let _ = connection.await; }));
    let mut sender = sender.ready().await?;
    let (response, mut stream) = sender.send_request(request, body.len==0)?;
    let mut remaining=body.len;
    while remaining>0 {
        let chunk=body.next_chunk().await?;ensure!(!chunk.is_empty(),"Incomplete temporary body");remaining-=chunk.len() as u64;
        send_h2_chunk(&mut stream,Bytes::from(chunk),remaining==0).await?;
    }
    drop(body);
    let response = response.await?;
    let status = response.status().as_u16();
    let fields: Vec<Header> = ordered_headers(response.headers(), response.extensions(), None, None)?.into_iter().filter(|h| !h.name.starts_with(':')).collect();
    let streaming = draft.method != "HEAD" && status != 204 && status != 304 && crate::sse::head(&CapturedResponse { trailers: vec![],  upstream_tls: None, sent_request_headers: Some(sent_fields.clone()), tls_version: None, status, version: "HTTP/2".into(), headers: fields.clone(), body_base64: Default::default(), raw_head_base64: None }).await?;
    let mut recv = response.into_body();
    let body_base64 = if streaming {
        STANDARD.encode(recv_h2_body_observed(&mut recv, true).await?)
    } else {
        recv_h2_body_spooled(&mut recv).await?.into_base64().await?
    };
    let trailers = recv.trailers().await?.map(|map| headers(&map)).unwrap_or_default();
    Ok((CapturedResponse { trailers, upstream_tls: None, sent_request_headers: Some(sent_fields), tls_version: None, status, version: "HTTP/2".into(), headers: fields, body_base64: body_base64.into(), raw_head_base64: None }, notes))
}

async fn recv_h2_body_spooled(stream: &mut h2::RecvStream) -> Result<BodySpool> {
    let mut body = BodySpool::empty();
    while let Some(chunk) = stream.data().await {
        let chunk = chunk?;
        body.append(&chunk).await?;
        // Do not advertise space for more DATA until the current bytes are in
        // bounded memory or on disk. Disk pressure therefore propagates to h2.
        stream.flow_control().release_capacity(chunk.len())?;
    }
    Ok(body)
}

/// Compatibility helper for callers that explicitly need decoded bytes. Capture
/// and ordinary replay use recv_h2_body_spooled instead.
pub async fn recv_h2_body(stream: &mut h2::RecvStream) -> Result<Vec<u8>> {
    let body=recv_h2_body_observed(stream, false).await?;
    ensure!(stream.trailers().await?.is_none(), "Request trailers are not supported yet");
    Ok(body)
}
async fn recv_h2_body_observed(stream: &mut h2::RecvStream, streaming: bool) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = stream.data().await {
        let chunk = chunk?;
        ensure!(!streaming || body.len() + chunk.len() <= MAX_BODY, "SSE body exceeds 8 MiB limit");
        body.try_reserve(chunk.len()).context("Unable to allocate response body")?;
        stream.flow_control().release_capacity(chunk.len())?;
        body.extend_from_slice(&chunk);
        if streaming { crate::sse::emit(crate::sse::Event::Data(chunk.to_vec())).await?; }
    }
    Ok(body)
}
pub async fn send_h2_body(stream: &mut h2::SendStream<Bytes>, body: Bytes) -> Result<()> {
    send_h2_chunk(stream, body, true).await
}
pub(crate) async fn send_h2_chunk(stream: &mut h2::SendStream<Bytes>, mut body: Bytes, end: bool) -> Result<()> {
    if body.is_empty() { if end { stream.send_data(Bytes::new(), true)?; } return Ok(()); }
    while !body.is_empty() {
        stream.reserve_capacity(body.len().min(16384));
        let capacity = std::future::poll_fn(|cx| stream.poll_capacity(cx)).await.transpose()?.ok_or_else(|| anyhow::anyhow!("Stream closed"))?;
        if capacity == 0 { continue; }
        let chunk = body.split_to(capacity.min(body.len()));
        stream.send_data(chunk, end && body.is_empty())?;
    }
    Ok(())
}

async fn connect_quic(host: &str, addresses: Vec<std::net::SocketAddr>, config: quinn::ClientConfig) -> Result<(quinn::Endpoint, quinn::Connection)> {
    ensure!(!addresses.is_empty(), "HTTP/3 DNS returned no addresses");
    let mut attempts = tokio::task::JoinSet::new();
    let mut seen = std::collections::HashSet::new();
    for address in addresses.into_iter().filter(|address| seen.insert(*address)).take(16) {
        let host = host.to_owned();
        let config = config.clone();
        attempts.spawn(async move {
            let result: Result<_> = async {
                let mut endpoint = crate::quic_diagnostics::client(if address.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }.parse()?)?;
                endpoint.set_default_client_config(config);
                let connection = tokio::time::timeout(std::time::Duration::from_secs(10), endpoint.connect(address, &host)?)
                    .await.context("QUIC handshake timed out after 10 seconds (no HTTP response)")??;
                Ok((endpoint, connection))
            }.await;
            result.with_context(|| format!("HTTP/3 QUIC endpoint {address}"))
        });
    }
    let mut errors = Vec::new();
    while let Some(result) = attempts.join_next().await {
        match result {
            Ok(Ok(connection)) => return Ok(connection),
            Ok(Err(error)) => errors.push(format!("{error:#}")),
            Err(error) => errors.push(error.to_string()),
        }
    }
    anyhow::bail!("HTTP/3 connection failed for all resolved addresses: {}", errors.join("; "))
}

pub async fn h3_send(draft: &RequestDraft, roots: &rustls::RootCertStore) -> Result<(CapturedResponse, Vec<String>)> {
    h3_send_via(draft, roots, None).await
}
pub async fn h3_send_via(draft: &RequestDraft, roots: &rustls::RootCertStore, proxy: Option<&crate::upstream::UpstreamProxy>) -> Result<(CapturedResponse, Vec<String>)> {
    let (request, mut body, notes) = request(draft).await?;
    let url = validate_url(&draft.url)?;
    let host = hostname(&url);
    let mut tls = crate::tls_config::config(&draft.tls, roots, "h3")?;
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let mut config = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls)?));
    let mut tunnel = if let Some(proxy) = proxy {
        Some(tokio::time::timeout(std::time::Duration::from_secs(10), crate::socks_udp::open(&url, proxy)).await.context("SOCKS5 UDP association timed out")??)
    } else { None };
    let (_endpoint, connection) = if let Some(tunnel) = &mut tunnel {
        let mut transport = quinn::TransportConfig::default();
        transport.initial_mtu(1200).min_mtu(1200).mtu_discovery_config(None);
        config.transport_config(Arc::new(transport));
        tunnel.endpoint.set_default_client_config(config);
        let connecting = tunnel.endpoint.connect(tunnel.peer, &host)?;
        let connection = tokio::select! {
            result = tokio::time::timeout(std::time::Duration::from_secs(10), connecting) => result.context("HTTP/3 QUIC handshake through SOCKS5 UDP timed out")??,
            error = tunnel.stopped() => return Err(error),
        };
        (tunnel.endpoint.clone(), connection)
    } else {
        let addresses = tokio::net::lookup_host((host.as_str(), url.port_or_known_default().unwrap())).await.context("HTTP/3 DNS lookup failed")?.collect();
        connect_quic(&host, addresses, config).await?
    };
    let exchange = async {
    let mut upstream_tls = crate::tls_details::quic(&connection);
    upstream_tls.server_name = Some(host.clone());
    let (mut driver, mut sender) = h3::client::builder().max_field_section_size(MAX_HEAD as u64).build(h3_quinn::Connection::new(connection.clone())).await?;
    let _driver = Abort(tokio::spawn(async move { let _ = driver.wait_idle().await; }));
    let mut stream = sender.send_request(request).await?;
    loop {let chunk=body.next_chunk().await?;if chunk.is_empty(){break;}stream.send_data(Bytes::from(chunk)).await?;}
    drop(body);
    stream.finish().await?;
    let response = stream.recv_response().await?;
    let declared = declared_body_length(response.headers())?;
    let expected = if response_has_body(&draft.method, response.status().as_u16()) { declared } else { Some(0) };
    let mut body = BodySpool::empty();
    while let Some(mut chunk) = stream.recv_data().await? {
        while chunk.has_remaining() {
            let count = chunk.chunk().len();
            ensure!(count > 0, "Empty HTTP/3 receive buffer");
            let received = body.len.checked_add(count as u64).context("HTTP/3 response body length overflow")?;
            check_body_length(expected, received, false)?;
            body.append(chunk.chunk()).await?;
            check_body_length(expected, body.len, false)?;
            chunk.advance(count);
        }
    }
    check_body_length(expected, body.len, true)?;
    ensure!(stream.recv_trailers().await?.is_none(), "Trailers are not supported yet");
    let result = CapturedResponse { trailers: vec![],  upstream_tls: Some(upstream_tls), sent_request_headers: None, tls_version: Some("TLS 1.3".into()), status: response.status().as_u16(), version: "HTTP/3".into(), headers: headers(response.headers()), body_base64: body.into_base64().await?.into(), raw_head_base64: None };
    connection.close(0u32.into(), b"complete");
    Ok((result, notes))
    };
    if let Some(tunnel) = &mut tunnel {
        tokio::select! { result = exchange => result, error = tunnel.stopped() => Err(error) }
    } else { exchange.await }
}
