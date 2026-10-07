//! HTTP/2 records decoded HPACK field order using the vendored h2 extension.
use crate::{http1::*, model::*};
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
    let mut connection = h2::server::Builder::new().max_concurrent_streams(32).max_header_list_size(MAX_HEAD as u32).handshake::<_, Bytes>(io).await?;
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            item = connection.accept() => match item {
                Some(Ok((request, mut respond))) => {
                    let engine = engine.clone(); let origin = origin.clone(); let client_tls = client_tls.clone();
                    tasks.spawn(async move {
                        let result: Result<()> = async {
                            let (parts, mut recv) = request.into_parts();
                            ensure!(parts.method != http::Method::CONNECT, "Extended CONNECT is unsupported");
                            if let Some(authority) = parts.uri.authority() { ensure!(authority.as_str() == crate::http1::authority(&origin), "Authority differs from CONNECT target"); }
                            let path = parts.uri.path_and_query().map(|v| v.as_str()).unwrap_or("/");
                            let target = format!("https://{}{path}", crate::http1::authority(&origin));
                            let ordered = ordered_headers(&parts.headers, &parts.extensions, Some(&parts.uri), Some(&parts.method))?;
                            let (pseudo_headers, fields): (Vec<_>, Vec<_>) = ordered.into_iter().partition(|h| h.name.starts_with(':'));
                            let body = recv_h2_body(&mut recv).await?;
                            let draft = RequestDraft { upstream_profile_id: None, scripts: Default::default(), pseudo_headers, engine: "h2".into(), method: parts.method.to_string(), url: target,
                                headers: fields, body_base64: STANDARD.encode(body), tls: TlsProfile::default() };
                            let flow = engine.execute_with_tls(draft, None, "capture", None, Some("HTTP/2".into()), client_tls).await?;
                            let response = response_for_flow(flow)?;
                            let (parts, body) = response.into_parts();
                            let mut send = respond.send_response(http::Response::from_parts(parts, ()), body.is_empty())?;
                            if !body.is_empty() { send_h2_body(&mut send, Bytes::from(body)).await?; }
                            Ok(())
                        }.await;
                        if let Err(error) = result { tracing::warn!(%error, "h2 stream failed"); respond.send_reset(h2::Reason::INTERNAL_ERROR); }
                    });
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
    if let Some(error) = flow.error { return Ok(http::Response::builder().status(502).body(error.into_bytes())?); }
    let response = flow.response.ok_or_else(|| anyhow::anyhow!("Missing response"))?;
    let mut output = http::Response::builder().status(response.status).body(STANDARD.decode(response.body_base64)?)?;
    for field in end_to_end(&response.headers) {
        output.headers_mut().append(http::header::HeaderName::from_bytes(field.name.as_bytes())?, http::HeaderValue::from_bytes(&header_bytes(&field.value)?)?);
    }
    Ok(output)
}

pub fn headers(map: &http::HeaderMap) -> Vec<Header> {
    map.iter().map(|(name, value)| Header { name: name.to_string(), value: value.as_bytes().iter().map(|b| *b as char).collect() }).collect()
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
        fields.push(Header { name: name.clone(), value });
    }
    Ok(fields)
}

fn request_h2(draft: &RequestDraft) -> Result<(http::Request<()>, Vec<u8>, Vec<String>, Vec<Header>)> {

    // Reuse framing/URL validation, but do not inject H1-only Host/length fields.
    let (url, _, body, _) = prepare(draft)?;
    ensure!(url.scheme() == "https", "h2 requires HTTPS");
    let mut request = http::Request::builder().method(draft.method.as_str()).uri(url.as_str()).body(())?;
    let pseudo_names: Vec<&str> = if draft.pseudo_headers.is_empty() { vec![":method", ":scheme", ":authority", ":path"] }
        else { draft.pseudo_headers.iter().map(|h| h.name.as_str()).collect() };
    let mut fields = Vec::new();
    for name in pseudo_names {
        let value = match name {
            ":method" => draft.method.clone(), ":scheme" => url.scheme().into(),
            ":authority" => authority(&url), ":path" => url[url::Position::BeforePath..url::Position::AfterQuery].into(),
            _ => anyhow::bail!("Unsupported HTTP/2 pseudo header: {name}"),
        };
        ensure!(!fields.iter().any(|h: &Header| h.name == name), "Repeated HTTP/2 pseudo header");
        fields.push(Header { name: name.into(), value });
    }
    // The h2 URI encoder supplies these fields. Never silently invent one for a captured request.
    ensure!([":method", ":scheme", ":path"].iter().all(|n| fields.iter().any(|h| h.name == *n)), "Missing required HTTP/2 pseudo header");
    ensure!(fields.iter().any(|h| h.name == ":authority") || draft.headers.iter().any(|h| h.name.eq_ignore_ascii_case("host")), "Missing HTTP/2 authority/Host");
    let retained = end_to_end(&draft.headers);
    let mut notes = vec!["HTTP/2 按客户端/编辑列表的字段顺序编码，包含伪头部和交错重复字段；HPACK 压缩字节、帧边界及 TLS 握手不作原样复制。".into()];
    for original in &draft.headers {
        let name = original.name.to_ascii_lowercase();
        if name == "expect" || (!retained.contains(original) && !(name == "te" && original.value == "trailers")) {
            notes.push(format!("移除字段 {}。", original.name)); continue;
        }
        let value = if name == "host" { authority(&url) } else if name == "content-length" { body.len().to_string() } else { original.value.clone() };
        if value != original.value { notes.push(format!("{} 按目标/正文修正，位置不变。", original.name)); }
        request.headers_mut().append(http::header::HeaderName::from_bytes(name.as_bytes())?, http::HeaderValue::from_bytes(&header_bytes(&value)?)?);
        fields.push(Header { name, value });
    }
    request.extensions_mut().insert(h2::ext::HeaderOrder(fields.iter().map(|h| h.name.clone()).collect()));
    Ok((request, body, notes, fields))
}
fn request(draft: &RequestDraft) -> Result<(http::Request<()>, Vec<u8>, Vec<String>)> {

    let (url, fields, body, mut notes) = prepare(draft)?;
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
    request_h2(draft)?;
    let url = validate_url(&draft.url)?;
    let config = crate::tls_config::config(&draft.tls, roots, "h2")?;
    let tcp = crate::upstream::connect(&url, proxy).await?.stream;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(rustls::pki_types::ServerName::try_from(hostname(&url))?, tcp).await?;
    ensure!(tls.get_ref().1.alpn_protocol() == Some(b"h2"), "Server did not negotiate h2");
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
    let (request, body, notes, sent_fields) = request_h2(draft)?;
    let (sender, connection) = h2::client::Builder::new().max_header_list_size(MAX_HEAD as u32).enable_push(false).handshake::<_, Bytes>(tls).await?;
    let _driver = Abort(tokio::spawn(async move { let _ = connection.await; }));
    let mut sender = sender.ready().await?;
    let (response, mut stream) = sender.send_request(request, body.is_empty())?;
    if !body.is_empty() { send_h2_body(&mut stream, Bytes::from(body)).await?; }
    let response = response.await?;
    let status = response.status().as_u16();
    let fields = ordered_headers(response.headers(), response.extensions(), None, None)?.into_iter().filter(|h| !h.name.starts_with(':')).collect();
    let mut recv = response.into_body();
    let body = recv_h2_body(&mut recv).await?;
    Ok((CapturedResponse { upstream_tls: None, sent_request_headers: Some(sent_fields), tls_version: None, status, version: "HTTP/2".into(), headers: fields, body_base64: STANDARD.encode(body), raw_head_base64: None }, notes))
}

pub async fn recv_h2_body(stream: &mut h2::RecvStream) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = stream.data().await {
        let chunk = chunk?;
        ensure!(body.len() + chunk.len() <= MAX_BODY, "Body exceeds 8 MiB limit");
        stream.flow_control().release_capacity(chunk.len())?;
        body.extend_from_slice(&chunk);
    }
    ensure!(stream.trailers().await?.is_none(), "Trailers are not supported yet");
    Ok(body)
}
pub async fn send_h2_body(stream: &mut h2::SendStream<Bytes>, mut body: Bytes) -> Result<()> {
    while !body.is_empty() {
        stream.reserve_capacity(body.len().min(16384));
        let capacity = std::future::poll_fn(|cx| stream.poll_capacity(cx)).await.transpose()?.ok_or_else(|| anyhow::anyhow!("Stream closed"))?;
        if capacity == 0 { continue; }
        let chunk = body.split_to(capacity.min(body.len()));
        stream.send_data(chunk, body.is_empty())?;
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
    let (request, body, notes) = request(draft)?;
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
    if !body.is_empty() { stream.send_data(Bytes::from(body)).await?; }
    stream.finish().await?;
    let response = stream.recv_response().await?;
    let mut body = Vec::new();
    while let Some(mut chunk) = stream.recv_data().await? {
        ensure!(body.len() + chunk.remaining() <= MAX_BODY, "Body exceeds 8 MiB limit");
        body.extend_from_slice(&chunk.copy_to_bytes(chunk.remaining()));
    }
    ensure!(stream.recv_trailers().await?.is_none(), "Trailers are not supported yet");
    let result = CapturedResponse { upstream_tls: Some(upstream_tls), sent_request_headers: None, tls_version: Some("TLS 1.3".into()), status: response.status().as_u16(), version: "HTTP/3".into(), headers: headers(response.headers()), body_base64: STANDARD.encode(body), raw_head_base64: None };
    connection.close(0u32.into(), b"complete");
    Ok((result, notes))
    };
    if let Some(tunnel) = &mut tunnel {
        tokio::select! { result = exchange => result, error = tunnel.stopped() => Err(error) }
    } else { exchange.await }
}
