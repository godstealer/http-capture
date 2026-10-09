use crate::{http1::*, model::*, Engine};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{net::{IpAddr, Ipv4Addr, SocketAddr}, sync::Arc, time::Duration};
use tokio::{io::{AsyncRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}, sync::oneshot, task::{JoinHandle, JoinSet}};
use tokio_rustls::LazyConfigAcceptor;

pub struct ProxyHandle {
    pub address: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}
impl ProxyHandle {
    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() { let _ = stop.send(()); }
        let _ = (&mut self.task).await;
    }
}
impl Drop for ProxyHandle {
    fn drop(&mut self) { if let Some(stop) = self.stop.take() { let _ = stop.send(()); } }
}

pub async fn start(engine: Arc<Engine>, port: u16) -> Result<ProxyHandle> {
    start_on(engine, IpAddr::V4(Ipv4Addr::LOCALHOST), port).await
}

/// The control API remains loopback-only; this address applies only to captured traffic.
pub async fn start_on(engine: Arc<Engine>, host: IpAddr, port: u16) -> Result<ProxyHandle> {
    start_at(engine, SocketAddr::new(host, port)).await
}

pub async fn start_at(engine: Arc<Engine>, requested: SocketAddr) -> Result<ProxyHandle> {
    start_mode(engine, requested, false).await
}
pub async fn start_tun_bridge(engine: Arc<Engine>) -> Result<ProxyHandle> {
    start_mode(engine, "127.0.0.1:0".parse().unwrap(), true).await
}
async fn start_mode(engine: Arc<Engine>, requested: SocketAddr, transparent: bool) -> Result<ProxyHandle> {
    ensure!(!requested.ip().is_multicast(), "监听地址不能是组播地址");
    let listener = TcpListener::bind(requested).await.with_context(||format!("无法监听 {requested}：端口可能被占用，或 IP 不属于本机"))?;
    let address = listener.local_addr()?;
    if !transparent { *engine.upstream.listener.write().unwrap() = Some(address); }
    let (stop, mut receiver) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                _ = &mut receiver => break,
                Some(_) = connections.join_next(), if !connections.is_empty() => {},
                result = listener.accept(), if connections.len() < 64 => {
                    match result {
                        Ok((stream, _)) => {
                            let engine = engine.clone();
                            connections.spawn(async move {
                                match tokio::time::timeout(Duration::from_secs(360), handle(stream, engine, address, transparent)).await {
                                    Ok(Err(error)) => tracing::warn!(%error, "Proxy connection failed"),
                                    Err(_) => tracing::warn!("Proxy connection timed out"),
                                    _ => {},
                                }
                            });
                        }
                        Err(error) => { tracing::error!(%error, "Proxy accept failed"); break; }
                    }
                }
            }
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        let mut listener = engine.upstream.listener.write().unwrap();
        if *listener == Some(address) { *listener = None; }
    });
    Ok(ProxyHandle { address, stop: Some(stop), task })
}

async fn error_response<S: AsyncWrite + Unpin>(stream: &mut S, code: u16, text: &str) -> Result<()> {
    let head = format!("HTTP/1.1 {code} Proxy Error\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", text.len());
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(text.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

async fn handle(stream: TcpStream, engine: Arc<Engine>, address: SocketAddr, transparent: bool) -> Result<()> {
    let mut stream = BufReader::new(stream);
    let head = read_head(&mut stream).await?;
    if head.is_empty() { return Ok(()); }
    let (method, target, _) = match parse_request(&head) {
        Ok(v) => v,
        Err(e) => { error_response(&mut stream, 400, &e.to_string()).await?; return Ok(()); }
    };
    if method == "CONNECT" {
        let mut url = validate_url(&format!("https://{target}/"))?;
        ensure!(url.path() == "/" && url.query().is_none(), "Invalid CONNECT authority");
        prevent_loop(&url, address).await?;
        stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
        stream.flush().await?;
        if transparent && stream.fill_buf().await?.first().is_some_and(|b| *b != 22) {
            url.set_scheme("http").map_err(|_|anyhow::anyhow!("Invalid HTTP scheme"))?;
            let head=read_head(&mut stream).await?;
            if head.is_empty() { return Ok(()); }
            let (_,_,headers)=parse_request(&head)?;
            if let Some(host)=headers.iter().find(|h|h.name.eq_ignore_ascii_case("host")) {
                let named=validate_url(&format!("http://{}/",host.value))?;
                ensure!(named.path()=="/" && named.query().is_none(),"Invalid Host");
                url=named;
            }
            if let Err(err)=exchange(&mut stream,&head,Some(&url),&engine,address,None).await {
                error_response(&mut stream,502,&format!("{err:#}")).await?;
            }
            stream.shutdown().await?;return Ok(());
        }
        let start = LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream).await?;
        if transparent {
            if let Some(name)=start.client_hello().server_name() { url.set_host(Some(name))?; }
        }
        let mut config = (*engine.ca.server_config(&hostname(&url))?).clone();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let config = Arc::new(config);
        let mut client_tls = crate::tls_details::client_hello(&start.client_hello());
        let local_chain = config.cert_resolver.resolve(start.client_hello()).map(|key| crate::tls_details::certificates(&key.cert)).unwrap_or_default();
        let tls = start.into_stream(config).await.context("TLS handshake failed; trust the local CA in your client")?;
        let negotiated = crate::tls_details::negotiated(tls.get_ref().1);
        client_tls.version = negotiated.version; client_tls.cipher_suite = negotiated.cipher_suite;
        client_tls.alpn = negotiated.alpn; client_tls.handshake_kind = negotiated.handshake_kind;
        client_tls.certificates = local_chain;
        if tls.get_ref().1.alpn_protocol() == Some(b"h2") {
            return crate::multiplex::h2_capture_with_tls(tls, engine, url, Some(client_tls)).await;
        }
        let mut tls = BufReader::new(tls);
        let head = read_head(&mut tls).await?;
        if head.is_empty() { return Ok(()); }
        if let Err(err) = exchange(&mut tls, &head, Some(&url), &engine, address, Some(client_tls)).await {
            error_response(&mut tls, 502, &format!("{err:#}")).await?;
        }
        tls.shutdown().await?;
    } else {
        if let Err(err) = exchange(&mut stream, &head, None, &engine, address, None).await {
            error_response(&mut stream, 502, &format!("{err:#}")).await?;
        }
        stream.shutdown().await?;
    }
    Ok(())
}

pub(crate) fn listener_ip_matches(ip: IpAddr, listener: SocketAddr) -> bool {
    let ip = ip.to_canonical();
    ip.is_loopback() || ip.is_unspecified() || ip == listener.ip().to_canonical()
        || (listener.ip().is_unspecified() && std::net::UdpSocket::bind(SocketAddr::new(ip, 0)).is_ok())
}
async fn prevent_loop(url: &url::Url, address: SocketAddr) -> Result<()> {
    if url.port_or_known_default() != Some(address.port()) { return Ok(()); }
    let host = hostname(url);
    let addresses = tokio::net::lookup_host((host.as_str(), address.port())).await?;
    ensure!(!addresses.into_iter().any(|a|listener_ip_matches(a.ip(),address)), "Refusing a proxy loop");
    Ok(())
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut BufReader<S>, head: &[u8],
    tunnel: Option<&url::Url>, engine: &Engine, address: SocketAddr, client_tls: Option<TlsDetails>) -> Result<()> {
    let (method, target, headers) = parse_request(head)?;
    let url = if let Some(origin) = tunnel {
        ensure!(target.starts_with('/') && !target.starts_with("//"), "Only origin-form targets inside CONNECT are supported");
        validate_url(&format!("{}://{}{target}", origin.scheme(), authority(origin)))?
    } else {
        let normalized = if let Some(rest) = target.strip_prefix("ws://") { format!("http://{rest}") } else { target.clone() };
        validate_url(&normalized)?
    };
    prevent_loop(&url, address).await?;
    let mut request = RequestDraft { upstream_profile_id: None, scripts: Default::default(), pseudo_headers: vec![], engine: "native".into(), method: method.clone(), url: url.to_string(), headers, body_base64: String::new(),
        tls: TlsProfile { preset: "native".into(), ..Default::default() } };
    if crate::websocket::requested(&request.headers) { return crate::websocket::capture(stream, request, head, engine, client_tls).await; }
    let body_result: Result<Vec<u8>> = async {
        ensure!(values(&request.headers, "upgrade").next().is_none(), "WebSocket/Upgrade is not supported yet");
        let expectations: Vec<_> = values(&request.headers, "expect").collect();
        if !expectations.is_empty() {
            ensure!(expectations.len() == 1 && expectations[0].eq_ignore_ascii_case("100-continue"), "Unsupported Expect header");
            stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await?;
            stream.flush().await?;
        }
        let (body, trailers) = read_body(stream, &request.headers, false).await?;
        ensure!(!trailers, "Request trailers are not supported yet");
        Ok(body)
    }.await;
    match body_result {
        Ok(body) => request.body_base64 = STANDARD.encode(body),
        Err(error) => {
            engine.record_failure(request, STANDARD.encode(head), format!("{error:#}"))?;
            error_response(stream, 400, &format!("{error:#}")).await?;
            return Ok(());
        }
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let work = crate::sse::DOWNSTREAM.scope(tx, engine.execute_with_tls(request, None, "capture", Some(STANDARD.encode(head)), Some("HTTP/1.1".into()), client_tls));
    tokio::pin!(work);
    let mut streamed = false;
    let mut completed: Option<Result<Flow>> = None;
    let flow = loop {
        if completed.is_some() && rx.is_empty() { break completed.take().unwrap()?; }
        tokio::select! {
            biased;
            Some(event) = rx.recv() => {
                match event {
                    crate::sse::Event::Head(response) => {
                        streamed = true;
                        let mut out = format!("HTTP/1.1 {} \r\n", response.status).into_bytes();
                        for h in end_to_end(&response.headers).into_iter().filter(|h| !h.name.eq_ignore_ascii_case("content-length")) {
                            out.extend_from_slice(h.name.as_bytes()); out.extend_from_slice(b": "); out.extend(header_bytes(&h.value)?); out.extend_from_slice(b"\r\n");
                        }
                        out.extend_from_slice(b"Connection: close\r\n\r\n");
                        stream.write_all(&out).await?;
                    }
                    crate::sse::Event::Data(data) => stream.write_all(&data).await?,
                }
                stream.flush().await?;
            }
            result = &mut work, if completed.is_none() => completed = Some(result),
        }
    };
    if streamed { return Ok(()); }
    if let Some(error) = flow.error { error_response(stream, 502, &error).await?; return Ok(()); }
    let response = flow.response.context("No response")?;
    let body = STANDARD.decode(&response.body_base64)?;
    let mut headers = end_to_end(&response.headers);
    // Transfer coding is decoded. HEAD and 304 retain their representation length.
    if method != "HEAD" && response.status != 304 {
        headers.retain(|h| !h.name.eq_ignore_ascii_case("content-length"));
        if response.status != 204 {
            headers.push(Header { name: "Content-Length".into(), value: body.len().to_string() });
        }
    }
    let mut out = format!("HTTP/1.1 {} \r\n", response.status).into_bytes();
    for h in headers {
        out.extend_from_slice(h.name.as_bytes()); out.extend_from_slice(b": ");
        out.extend(header_bytes(&h.value)?); out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    out.extend(body);
    stream.write_all(&out).await?;
    stream.flush().await?;
    Ok(())
}
