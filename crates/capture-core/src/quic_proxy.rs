//! Explicit loopback QUIC reverse proxy.
use anyhow::{ensure, Result};
use bytes::{Buf, Bytes};
use crate::{http1::*, model::*, multiplex, Engine};
use std::{sync::Arc, time::Duration};

pub struct QuicHandle { pub address: std::net::SocketAddr, stop: Option<tokio::sync::oneshot::Sender<()>>, task: tokio::task::JoinHandle<Result<()>> }
impl QuicHandle {
    pub async fn stop(mut self) { if let Some(stop)=self.stop.take() { let _=stop.send(()); } let _=(&mut self.task).await; }
}
impl Drop for QuicHandle { fn drop(&mut self) { if let Some(stop)=self.stop.take() { let _=stop.send(()); } } }
struct SniResolver(Arc<Engine>);
impl std::fmt::Debug for SniResolver { fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result { f.write_str("CaptureSniResolver") } }
impl rustls::server::ResolvesServerCert for SniResolver {
    fn resolve(&self, hello: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let name=hello.server_name()?;
        let config=self.0.ca.server_config(name).ok()?;
        config.cert_resolver.resolve(hello)
    }
}
/// UDP 443 redirect ingress. Real target is the authenticated ClientHello SNI,
/// never the loopback redirect address; cross-origin requests are rejected.
pub async fn start_tun(engine: Arc<Engine>) -> Result<QuicHandle> {
    let mut tls=rustls::ServerConfig::builder().with_no_client_auth().with_cert_resolver(Arc::new(SniResolver(engine.clone())));
    tls.alpn_protocols=vec![b"h3".to_vec()];
    let config=quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls)?));
    let endpoint=quinn::Endpoint::server(config,"127.0.0.1:0".parse()?)?;
    let address=endpoint.local_addr()?;
    let (stop, receiver)=tokio::sync::oneshot::channel();
    let task=tokio::spawn(serve(engine,None,endpoint,receiver));
    Ok(QuicHandle{address,stop:Some(stop),task})
}

pub async fn run(engine: Arc<Engine>, target: url::Url, port: u16, ready: Option<tokio::sync::oneshot::Sender<std::net::SocketAddr>>) -> Result<()> {
    ensure!(target.scheme() == "https" && target.path() == "/" && target.query().is_none(), "Target must be an HTTPS origin");
    let mut tls = (*engine.ca.server_config(&hostname(&target))?).clone();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    tls.max_early_data_size = 0;
    let config = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls)?));
    let endpoint = quinn::Endpoint::server(config, ([127, 0, 0, 1], port).into())?;
    println!("QUIC reverse proxy: {} -> {}\nCA: {}", endpoint.local_addr()?, target, engine.ca.cert_path.display());
    if let Some(ready) = ready { let _ = ready.send(endpoint.local_addr()?); }
    let (stop, receiver)=tokio::sync::oneshot::channel();
    let worker=tokio::spawn(serve(engine,Some(target),endpoint,receiver));
    tokio::pin!(worker);
    tokio::select! {
        result=&mut worker => return result?,
        _=tokio::signal::ctrl_c() => { let _=stop.send(()); }
    }
    worker.await?
}
async fn serve(engine: Arc<Engine>, target: Option<url::Url>, endpoint: quinn::Endpoint, mut stop: tokio::sync::oneshot::Receiver<()>) -> Result<()> {
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            incoming = endpoint.accept(), if tasks.len() < 32 => {
                let Some(incoming) = incoming else { break };
                let engine = engine.clone(); let target = target.clone();
                tasks.spawn(async move {
                    let work = async {
                        let connection = tokio::time::timeout(Duration::from_secs(10), incoming).await
                            .map_err(|_| anyhow::anyhow!("H3 client handshake timed out after 10 seconds"))??;
                        let client_tls = crate::tls_details::quic(&connection);
                        let redirected = target.is_none();
                        let traffic_origin = crate::traffic_origin::quic(connection.remote_address(), redirected);

                        let target = match target {
                            Some(target) => target,
                            None => {
                                let name=client_tls.server_name.as_deref().ok_or_else(||anyhow::anyhow!("TUN H3 requires visible SNI; ECH is unsupported"))?;
                                let mut origin=url::Url::parse("https://placeholder.invalid/")?;
                                origin.set_host(Some(name))?;
                                origin
                            }
                        };
                        let mut server = h3::server::builder().max_field_section_size(MAX_HEAD as u64).build(h3_quinn::Connection::new(connection)).await?;
                        let mut streams = tokio::task::JoinSet::new();
                        loop {
                            tokio::select! {
                                _ = streams.join_next(), if !streams.is_empty() => {},
                                next = server.accept(), if streams.len() < 32 => {
                                    let Some(resolver) = next? else { break };
                                    let engine = engine.clone(); let target = target.clone(); let client_tls = client_tls.clone();
                                    let traffic_origin = traffic_origin.clone();
                                    streams.spawn(async move {
                                        let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
                                        let (request, mut stream) = match tokio::time::timeout_at(deadline, resolver.resolve_request()).await {
                                            Ok(Ok(value)) => value,
                                            Ok(Err(error)) => { eprintln!("h3 request: {error:#}"); return; }
                                            Err(_) => { eprintln!("h3 request timed out after 300 seconds"); return; }
                                        };
                                        let result = tokio::time::timeout_at(deadline, async {
                                            let valid=request.method()!=http::Method::CONNECT
                                                && request.uri().host()==Some(hostname(&target).as_str())
                                                && request.uri().scheme_str()==Some("https")
                                                && (!redirected || request.uri().port_u16().unwrap_or(443)==443);
                                            if !valid {
                                                stream.stop_sending(h3::error::Code::H3_REQUEST_REJECTED);
                                                stream.stop_stream(h3::error::Code::H3_REQUEST_REJECTED);
                                                anyhow::bail!("H3 request origin or method is unsupported");
                                            }
                                            let expected = multiplex::declared_body_length(request.headers())?;
                                            let mut body = crate::body_spool::BodySpool::empty();
                                            while let Some(mut chunk) = stream.recv_data().await? {
                                                while chunk.has_remaining() {
                                                    let count = chunk.chunk().len();
                                                    ensure!(count > 0, "Empty HTTP/3 receive buffer");
                                                    let next_len = body.len.checked_add(count as u64)
                                                        .ok_or_else(|| anyhow::anyhow!("HTTP/3 body length overflow"))?;
                                                    multiplex::check_body_length(expected, next_len, false)?;
                                                    body.append(chunk.chunk()).await?;
                                                    chunk.advance(count);
                                                }
                                            }
                                            multiplex::check_body_length(expected, body.len, true)?;
                                            ensure!(stream.recv_trailers().await?.is_none(), "Trailers unsupported");
                                            let path = request.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
                                            let draft = RequestDraft { upstream_profile_id: None, scripts: Default::default(), pseudo_headers: vec![], engine: "h3".into(), method: request.method().to_string(),
                                                url: format!("https://{}{path}", authority(&target)), headers: multiplex::headers(request.headers()),
                                                body_base64: body.into_base64().await?.into(), tls: TlsProfile::default() };
                                            let flow = crate::traffic_origin::scope(Some(traffic_origin), engine.execute_with_tls(draft, None, "capture", None, Some("HTTP/3".into()), Some(client_tls))).await?;
                                            let (response, mut body) = multiplex::response_head_for_flow(flow).await?;
                                            let (parts, ()) = response.into_parts();
                                            stream.send_response(http::Response::from_parts(parts, ())).await?;
                                            let mut remaining = body.len;
                                            while remaining > 0 {
                                                let chunk = body.next_chunk().await?;
                                                ensure!(!chunk.is_empty() && chunk.len() as u64 <= remaining, "Incomplete temporary response body");
                                                remaining -= chunk.len() as u64;
                                                stream.send_data(Bytes::from(chunk)).await?;
                                            }
                                            stream.finish().await?;
                                            Ok::<(), anyhow::Error>(())
                                        }).await.unwrap_or_else(|_| Err(anyhow::anyhow!("H3 stream timed out after 300 seconds")));
                                        if let Err(error) = result {
                                            // A dropped Quinn send stream may finish normally. Explicitly
                                            // reset it so a failed disk read/timeout cannot make a response
                                            // prefix look like a successfully completed body.
                                            stream.stop_sending(h3::error::Code::H3_INTERNAL_ERROR);
                                            stream.stop_stream(h3::error::Code::H3_INTERNAL_ERROR);
                                            eprintln!("h3 stream: {error:#}");
                                        }
                                    });
                                }
                            }
                        }
                        Ok::<_, anyhow::Error>(())
                    };
                    match tokio::time::timeout(Duration::from_secs(360), work).await {
                        Ok(Err(error)) => eprintln!("h3 connection: {error:#}"),
                        Err(_) => eprintln!("h3 connection timed out"), _ => {},
                    }
                });
            }
        }
    }
    endpoint.close(0u32.into(), b"stopping");
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    endpoint.wait_idle().await;
    Ok(())
}
