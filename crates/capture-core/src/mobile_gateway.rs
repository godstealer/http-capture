//! An isolated, authenticated LAN entry point for Android VPN forwarding.
//! The desktop's ordinary HTTP proxy must never be exposed as the pairing
//! receiver: it has no device authentication and may decrypt HTTPS.

use crate::mobile_pairing::{PairingCode, PendingPairing, PAIRING_TOKEN_BYTES};
use crate::{model::{Flow, RequestDraft, TrafficOrigin}, Engine};
use anyhow::{ensure, Context, Result};
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, net::{Ipv4Addr, SocketAddr}, sync::{Arc, Mutex}, time::{Instant, SystemTime, UNIX_EPOCH}};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream, UdpSocket}, sync::{broadcast, mpsc, watch, Semaphore}, task::JoinHandle, time::{timeout, Duration}};
use tokio_rustls::TlsAcceptor;

const SESSION_LIFETIME_SECONDS: u64 = 3600;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_HOST_BYTES: usize = 253;
const MAX_UDP_FRAME: usize = 12 * 1024;

#[derive(Clone, Debug)]
pub enum GatewayEvent {
    Paired { peer: SocketAddr },
    Connected { peer: SocketAddr, host: String, port: u16 },
    Disconnected,
}

#[derive(Default)]
struct GatewayState {
    pending: PendingPairing,
    session: Option<([u8; PAIRING_TOKEN_BYTES], u64)>,
}

/// Protocol v1 is carried entirely within TLS pinned to the SPKI in the QR.
/// Type 1 redeems the one-use QR token and returns a short-lived session token.
/// Type 2 authenticates with that token, then tunnels one TCP stream. A zero
/// response byte means success; any other response byte closes the stream.
pub struct MobileGateway {
    address: SocketAddr,
    fingerprint: String,
    state: Arc<Mutex<GatewayState>>,
    generation: watch::Sender<u64>,
    events: broadcast::Sender<GatewayEvent>,
    task: JoinHandle<()>,
    #[cfg(test)]
    certificate_der: Vec<u8>,
}

impl MobileGateway {
    pub async fn start(host: Ipv4Addr, port: u16, engine: Arc<Engine>) -> Result<Self> {
        Self::start_inner(host, port, false, Some(engine)).await
    }

    async fn start_inner(host: Ipv4Addr, port: u16, allow_loopback: bool, engine: Option<Arc<Engine>>) -> Result<Self> {
        ensure!((host.is_private() && !host.is_loopback()) || (allow_loopback && host.is_loopback()),
            "Gateway requires a private LAN IPv4 address");
        let key = KeyPair::generate()?;
        let params = CertificateParams::new(vec![host.to_string()])?;
        let cert = params.self_signed(&key)?;
        let certificate_der = cert.der().to_vec();
        let digest = Sha256::digest(key.public_key_der());
        let fingerprint = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(certificate_der.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())))?;
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind((host, port)).await?;
        let address = listener.local_addr()?;
        ensure!(address.port() >= 1024, "Gateway port must be at least 1024");
        let state = Arc::new(Mutex::new(GatewayState::default()));
        let (generation, _) = watch::channel(0u64);
        let (events, _) = broadcast::channel(64);
        let task_state = state.clone();
        let task_generation = generation.clone();
        let task_events = events.clone();
        let task_engine = engine.clone();
        let permits = Arc::new(Semaphore::new(64));
        let task = tokio::spawn(async move {
            while let Ok((socket, peer)) = listener.accept().await {
                let Ok(permit) = permits.clone().try_acquire_owned() else { continue; };
                let acceptor = acceptor.clone();
                let state = task_state.clone();
                let generation = task_generation.clone();
                let events = task_events.clone();
                let engine = task_engine.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(error) = serve(socket, peer, acceptor, state, generation, events, allow_loopback, engine).await {
                        tracing::debug!(%peer, %error, "Mobile gateway connection ended");
                    }
                });
            }
        });
        Ok(Self { address, fingerprint, state, generation, events, task,
            #[cfg(test)] certificate_der })
    }

    pub fn address(&self) -> SocketAddr { self.address }
    pub fn fingerprint(&self) -> &str { &self.fingerprint }

    pub fn is_paired(&self) -> bool {
        self.state.lock().unwrap_or_else(|error| error.into_inner()).session
            .as_ref().is_some_and(|(_, expiry)| *expiry > unix_seconds().unwrap_or(u64::MAX))
    }

    pub fn events(&self) -> broadcast::Receiver<GatewayEvent> { self.events.subscribe() }

    pub fn open_pairing(&self) -> Result<PairingCode> {
        ensure!(!self.is_paired(), "Disconnect the current Android device before pairing another one");
        let host = match self.address.ip() { std::net::IpAddr::V4(host) => host, _ => unreachable!() };
        let code = PairingCode::generate(host, self.address.port(), &self.fingerprint, unix_seconds()?)?;
        self.state.lock().unwrap_or_else(|error| error.into_inner()).pending.replace(code.clone());
        Ok(code)
    }

    pub fn close_pairing_if(&self, token: &[u8]) {
        self.state.lock().unwrap_or_else(|error| error.into_inner()).pending.revoke_if(token);
    }

    pub fn disconnect(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.pending.revoke();
        state.session = None;
        self.generation.send_modify(|value| *value = value.wrapping_add(1));
        let _ = self.events.send(GatewayEvent::Disconnected);
    }

    pub fn stop(self) {
        self.disconnect();
        self.task.abort();
    }
}

async fn serve(socket: TcpStream, peer: SocketAddr, acceptor: TlsAcceptor,
    state: Arc<Mutex<GatewayState>>, generation_sender: watch::Sender<u64>,
    events: broadcast::Sender<GatewayEvent>, allow_loopback_destination: bool,
    engine: Option<Arc<Engine>>) -> Result<()> {
    let mut generation = generation_sender.subscribe();
    let mut tls = timeout(HANDSHAKE_TIMEOUT, acceptor.accept(socket)).await??;
    let mut operation = [0u8; 1];
    timeout(HANDSHAKE_TIMEOUT, tls.read_exact(&mut operation)).await??;
    let mut token = [0u8; PAIRING_TOKEN_BYTES];
    timeout(HANDSHAKE_TIMEOUT, tls.read_exact(&mut token)).await??;
    match operation[0] {
        1 => {
            let mut session = [0u8; PAIRING_TOKEN_BYTES];
            let result = (|| -> Result<()> {
                let mut locked = state.lock().unwrap_or_else(|error| error.into_inner());
                locked.pending.redeem(&token, unix_seconds()?)?;
                getrandom::getrandom(&mut session)
                    .map_err(|error| anyhow::anyhow!("Session token generation failed: {error}"))?;
                locked.session = Some((session, unix_seconds()? + SESSION_LIFETIME_SECONDS));
                generation_sender.send_modify(|value| *value = value.wrapping_add(1));
                Ok(())
            })();
            if result.is_err() { tls.write_all(&[1]).await?; return result; }
            tls.write_all(&[0]).await?;
            tls.write_all(&session).await?;
            let _ = events.send(GatewayEvent::Paired { peer });
        }
        2 => {
            let session_expiry = {
                let locked = state.lock().unwrap_or_else(|error| error.into_inner());
                locked.session.as_ref().and_then(|(saved, expiry)|
                    (*expiry > unix_seconds().unwrap_or(u64::MAX) && constant_time_eq(saved, &token))
                        .then_some(*expiry))
            };
            let Some(session_expiry) = session_expiry else {
                tls.write_all(&[1]).await?;
                anyhow::bail!("Mobile session is not authorized");
            };
            let generation_at_start = *generation.borrow();
            let host_len = timeout(HANDSHAKE_TIMEOUT, tls.read_u16()).await?? as usize;
            ensure!(host_len > 0 && host_len <= MAX_HOST_BYTES, "Invalid destination host length");
            let mut host_bytes = vec![0; host_len];
            timeout(HANDSHAKE_TIMEOUT, tls.read_exact(&mut host_bytes)).await??;
            let host = String::from_utf8(host_bytes)?;
            ensure!(host.bytes().all(|byte| byte.is_ascii_alphanumeric() || b".-:".contains(&byte)),
                "Invalid destination host");
            let port = timeout(HANDSHAKE_TIMEOUT, tls.read_u16()).await??;
            ensure!(port != 0, "Invalid destination port");
            let destinations = timeout(HANDSHAKE_TIMEOUT, tokio::net::lookup_host((host.as_str(), port))).await??;
            let mut upstream = None;
            for destination in destinations {
                if !allowed_destination(destination, tls.get_ref().0.local_addr()?, allow_loopback_destination) {
                    continue;
                }
                if let Ok(Ok(stream)) = timeout(Duration::from_secs(10), TcpStream::connect(destination)).await {
                    upstream = Some(stream);
                    break;
                }
            }
            let Some(mut upstream) = upstream else {
                tls.write_all(&[2]).await?;
                anyhow::bail!("Destination is unavailable or disallowed");
            };
            tls.write_all(&[0]).await?;
            let _ = events.send(GatewayEvent::Connected { peer, host: host.clone(), port });
            let started = Instant::now();
            let mut first_bytes = [0u8; 8192];
            let initial = match timeout(Duration::from_millis(200), tls.read(&mut first_bytes)).await {
                Ok(result) => result?, Err(_) => 0,
            };
            if initial > 0 { upstream.write_all(&first_bytes[..initial]).await?; }
            let mut flow = engine.as_ref().map(|engine| {
                let (method, url) = observed_request(&first_bytes[..initial], &host, port);
                let flow = mobile_flow(peer, method, url)?;
                engine.store.insert(&flow)?;
                let _ = engine.events.send(flow.clone());
                Ok::<_, anyhow::Error>(flow)
            }).transpose()?;
            let transferred = tokio::select! {
                result = tokio::io::copy_bidirectional(&mut tls, &mut upstream) => result.map_err(anyhow::Error::from),
                _ = tokio::time::sleep(Duration::from_secs(session_expiry.saturating_sub(unix_seconds()?))) => {
                    Err(anyhow::anyhow!("Mobile session expired"))
                }
                _ = generation.changed() => {
                    Err(anyhow::anyhow!(if *generation.borrow() != generation_at_start
                        { "Mobile session was revoked" } else { "Mobile gateway stopped" }))
                }
            };
            if let (Some(engine), Some(flow)) = (engine.as_ref(), flow.as_mut()) {
                flow.duration_ms = started.elapsed().as_millis() as u64;
                match &transferred {
                    Ok((sent, received)) => {
                        flow.state = "completed".into();
                        flow.notes.push(format!("Tunnel bytes: client={}, server={received}", *sent + initial as u64));
                    }
                    Err(error) => { flow.state = "failed".into(); flow.error = Some(error.to_string()); }
                }
                engine.store.insert(flow)?;
                let _ = engine.events.send(flow.clone());
            }
            transferred?;
        }
        3 => {
            let authorized = {
                let locked = state.lock().unwrap_or_else(|error| error.into_inner());
                locked.session.as_ref().is_some_and(|(saved, expiry)|
                    *expiry > unix_seconds().unwrap_or(u64::MAX) && constant_time_eq(saved, &token))
            };
            tls.write_all(&[if authorized { 0 } else { 1 }]).await?;
            ensure!(authorized, "Mobile session is not authorized");
        }
        4 => {
            let session_expiry = {
                let locked = state.lock().unwrap_or_else(|error| error.into_inner());
                locked.session.as_ref().and_then(|(saved, expiry)|
                    (*expiry > unix_seconds().unwrap_or(u64::MAX) && constant_time_eq(saved, &token))
                        .then_some(*expiry))
            };
            let Some(expiry) = session_expiry else {
                tls.write_all(&[1]).await?;
                anyhow::bail!("Mobile UDP session is not authorized");
            };
            tls.write_all(&[0]).await?;
            serve_udp_frames(&mut tls, peer, generation, expiry, engine, allow_loopback_destination).await?;
        }
        _ => { tls.write_all(&[1]).await?; anyhow::bail!("Unsupported mobile gateway operation"); }
    }
    Ok(())
}

fn constant_time_eq(left: &[u8; PAIRING_TOKEN_BYTES], right: &[u8; PAIRING_TOKEN_BYTES]) -> bool {
    left.iter().zip(right).fold(0u8, |difference, (a, b)| difference | (a ^ b)) == 0
}

fn allowed_destination(target: SocketAddr, gateway: SocketAddr, allow_loopback: bool) -> bool {
    if target == gateway { return false; }
    match target.ip() {
        std::net::IpAddr::V4(ip) => !ip.is_unspecified() && !ip.is_multicast()
            && (allow_loopback || !ip.is_loopback()),
        std::net::IpAddr::V6(ip) => !ip.is_unspecified() && !ip.is_multicast()
            && (allow_loopback || !ip.is_loopback()),
    }
}

fn observed_request(bytes: &[u8], host: &str, port: u16) -> (String, String) {
    let authority = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    let fallback = ("CONNECT".into(), format!("tcp://{authority}:{port}"));
    let Some(end) = bytes.windows(2).position(|pair| pair == b"\r\n") else { return fallback; };
    let Ok(line) = std::str::from_utf8(&bytes[..end]) else { return fallback; };
    let mut fields = line.split_ascii_whitespace();
    let (Some(method), Some(path), Some(version)) = (fields.next(), fields.next(), fields.next()) else { return fallback; };
    if fields.next().is_some() || !version.starts_with("HTTP/1.")
        || !method.bytes().all(|byte| byte.is_ascii_uppercase())
        || method.len() > 16 || path.len() > 4096 { return fallback; }
    let url = if path.starts_with("http://") || path.starts_with("https://") { path.to_string() }
        else if path.starts_with('/') { format!("http://{authority}:{port}{path}") }
        else { return fallback };
    (method.into(), url)
}

fn mobile_flow(peer: SocketAddr, method: String, url: String) -> Result<Flow> {
    Ok(Flow {
        state: "receiving".into(), websocket: None, original_request: None,
        original_response: None, client_tls: None, client_protocol: Some("TCP".into()),
        traffic_origin: Some(TrafficOrigin { entry: "android-lan".into(),
            client_address: Some(peer.to_string()), process: None, application: None,
            process_status: "remote".into() }),
        id: uuid::Uuid::new_v4().to_string(), parent_id: None,
        started_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
        duration_ms: 0, source: "capture".into(),
        request: RequestDraft { upstream_profile_id: None, scripts: Default::default(),
            pseudo_headers: vec![], engine: "native".into(), method, url, headers: vec![],
            body_base64: Default::default(), tls: Default::default() },
        response: None, error: None, raw_request_head_base64: None,
        notes: vec!["Android VPN 加密转发：只记录可见连接和首个 HTTP 请求元数据；HTTPS 正文未解密。".into()],
    })
}

/// tun2proxy's UDPGW frames stay inside the authenticated TLS stream.
async fn serve_udp_frames(tls: &mut tokio_rustls::server::TlsStream<TcpStream>, peer: SocketAddr,
    mut generation: watch::Receiver<u64>, expiry: u64, engine: Option<Arc<Engine>>,
    allow_loopback_destination: bool) -> Result<()> {
    let gateway = tls.get_ref().0.local_addr()?;
    let (mut reader, mut writer) = tokio::io::split(tls);
    let (outgoing, mut responses) = mpsc::channel::<Vec<u8>>(256);
    let (stop, _) = watch::channel(false);
    let read_loop = async {
        let mut sessions: HashMap<(u16, String, u16), mpsc::Sender<Vec<u8>>> = HashMap::new();
        let mut recorded = HashSet::new();
        loop {
            let length = timeout(Duration::from_secs(60), reader.read_u16()).await?? as usize;
            ensure!((3..=MAX_UDP_FRAME).contains(&length), "Invalid UDP gateway frame length");
            let mut frame = vec![0; length];
            timeout(Duration::from_secs(8), reader.read_exact(&mut frame)).await??;
            let id = [frame[1], frame[2]];
            if frame[0] & 1 != 0 {
                outgoing.send(vec![0, 3, 1, id[0], id[1]]).await?;
                continue;
            }
            ensure!(frame[0] & 2 != 0, "Unsupported UDP gateway frame");
            let (host, port, address_end) = parse_udp_destination(&frame)?;
            let key = (u16::from_be_bytes(id), host.clone(), port);
            if let Some(engine) = engine.as_ref() {
                if recorded.len() >= 1024 { recorded.clear(); }
                if recorded.insert(key.clone()) {
                    let authority = if host.contains(':') { format!("[{host}]") } else { host.clone() };
                    let mut flow = mobile_flow(peer, "UDP".into(), format!("udp://{authority}:{port}"))?;
                    flow.state = "completed".into();
                    flow.client_protocol = Some("UDP".into());
                    flow.notes = vec!["Android VPN 加密 UDP 转发：仅记录目标元数据，不保存载荷。".into()];
                    engine.store.insert(&flow)?;
                    let _ = engine.events.send(flow);
                }
            }
            let sender = if let Some(existing) = sessions.get(&key).filter(|sender| !sender.is_closed()) {
                existing.clone()
            } else {
                sessions.remove(&key);
                sessions.retain(|_, sender| !sender.is_closed());
                if sessions.len() >= 256 {
                    outgoing.send(udp_error(id)).await?;
                    continue;
                }
                let socket = match open_udp(&host, port, gateway, allow_loopback_destination).await {
                    Ok(socket) => socket,
                    Err(error) => {
                        tracing::debug!(%host, port, %error, "Mobile UDP destination failed");
                        outgoing.send(udp_error(id)).await?;
                        continue;
                    }
                };
                let (sender, inbound) = mpsc::channel(64);
                let mut response_head = frame[..address_end].to_vec();
                response_head[0] = 2;
                tokio::spawn(udp_flow(socket, response_head, inbound, outgoing.clone(), stop.subscribe()));
                sessions.insert(key, sender.clone());
                sender
            };
            if sender.send(frame[address_end..].to_vec()).await.is_err() {
                outgoing.send(udp_error(id)).await?;
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let write_loop = async {
        while let Some(frame) = responses.recv().await { writer.write_all(&frame).await?; }
        Ok::<(), anyhow::Error>(())
    };
    let result = tokio::select! {
        result = read_loop => result,
        result = write_loop => result,
        _ = generation.changed() => Err(anyhow::anyhow!("Mobile UDP session revoked")),
        _ = tokio::time::sleep(Duration::from_secs(expiry.saturating_sub(unix_seconds()?))) =>
            Err(anyhow::anyhow!("Mobile UDP session expired")),
    };
    stop.send_replace(true);
    result
}

async fn udp_flow(socket: UdpSocket, response_head: Vec<u8>, mut inbound: mpsc::Receiver<Vec<u8>>,
    outgoing: mpsc::Sender<Vec<u8>>, mut stop: watch::Receiver<bool>) {
    let mut bytes = vec![0; MAX_UDP_FRAME];
    let idle = tokio::time::sleep(Duration::from_secs(120));
    tokio::pin!(idle);
    loop {
        tokio::select! {
            request = inbound.recv() => {
                let Some(request) = request else { break; };
                if socket.send(&request).await.is_err() { break; }
                idle.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(120));
            }
            result = socket.recv(&mut bytes) => {
                let Ok(length) = result else { break; };
                let frame_length = response_head.len() + length;
                if frame_length > MAX_UDP_FRAME { continue; }
                let mut frame = Vec::with_capacity(frame_length + 2);
                frame.extend_from_slice(&(frame_length as u16).to_be_bytes());
                frame.extend_from_slice(&response_head);
                frame.extend_from_slice(&bytes[..length]);
                if outgoing.send(frame).await.is_err() { break; }
                idle.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(120));
            }
            _ = &mut idle => break,
            _ = stop.changed() => break,
        }
    }
}

fn udp_error(id: [u8; 2]) -> Vec<u8> { vec![0, 3, 0x20, id[0], id[1]] }

fn parse_udp_destination(frame: &[u8]) -> Result<(String, u16, usize)> {
    ensure!(frame.len() >= 7, "UDP gateway frame is too short");
    let mut offset = 4;
    let host = match frame[3] {
        1 => {
            ensure!(frame.len() >= offset + 4 + 2, "Invalid UDP IPv4 address");
            let host = Ipv4Addr::new(frame[offset], frame[offset+1], frame[offset+2], frame[offset+3]).to_string();
            offset += 4;
            host
        }
        4 => {
            ensure!(frame.len() >= offset + 16 + 2, "Invalid UDP IPv6 address");
            let mut bytes = [0; 16];
            bytes.copy_from_slice(&frame[offset..offset+16]);
            offset += 16;
            std::net::Ipv6Addr::from(bytes).to_string()
        }
        3 => {
            let length = frame[offset] as usize;
            offset += 1;
            ensure!(length > 0 && frame.len() >= offset + length + 2, "Invalid UDP hostname");
            let host = std::str::from_utf8(&frame[offset..offset+length])?.to_string();
            ensure!(host.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-'),
                "Invalid UDP hostname");
            offset += length;
            host
        }
        _ => anyhow::bail!("Unsupported UDP address type"),
    };
    let port = u16::from_be_bytes([frame[offset], frame[offset+1]]);
    ensure!(port != 0, "Invalid UDP destination port");
    Ok((host, port, offset + 2))
}

async fn open_udp(host: &str, port: u16, gateway: SocketAddr,
    allow_loopback_destination: bool) -> Result<UdpSocket> {
    let destinations = timeout(Duration::from_secs(5), tokio::net::lookup_host((host, port))).await??;
    for destination in destinations {
        if !allowed_destination(destination, gateway, allow_loopback_destination) { continue; }
        let bind = if destination.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
        let socket = UdpSocket::bind(bind).await?;
        if socket.connect(destination).await.is_err() { continue; }
        return Ok(socket);
    }
    anyhow::bail!("UDP destination is unavailable or disallowed")
}

fn unix_seconds() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH).context("Clock is before Unix epoch")?.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::ServerName;
    use tokio_rustls::TlsConnector;

    async fn connect(gateway: &MobileGateway) -> tokio_rustls::client::TlsStream<TcpStream> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(gateway.certificate_der.clone())).unwrap();
        let config = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        let socket = TcpStream::connect(gateway.address()).await.unwrap();
        let host = gateway.address().ip();
        TlsConnector::from(Arc::new(config))
            .connect(ServerName::IpAddress(host.into()), socket).await.unwrap()
    }

    #[tokio::test]
    async fn redeem_once_reject_replay_and_tunnel_until_revoked() {
        let host = Ipv4Addr::LOCALHOST;
        let gateway = MobileGateway::start_inner(host, 0, true, None).await.unwrap();
        let mut token = [0u8; PAIRING_TOKEN_BYTES];
        token.fill(7);
        gateway.state.lock().unwrap().pending.replace(PairingCode {
            host, port: gateway.address().port(), token,
            fingerprint: gateway.fingerprint.clone(), expires_at: unix_seconds().unwrap() + 300,
        });
        let mut invalid = connect(&gateway).await;
        invalid.write_all(&[1]).await.unwrap();
        invalid.write_all(&[8; PAIRING_TOKEN_BYTES]).await.unwrap();
        assert_eq!(invalid.read_u8().await.unwrap(), 1);

        let mut pairing = connect(&gateway).await;
        pairing.write_all(&[1]).await.unwrap();
        pairing.write_all(&token).await.unwrap();
        assert_eq!(pairing.read_u8().await.unwrap(), 0);
        let mut session = [0u8; PAIRING_TOKEN_BYTES];
        pairing.read_exact(&mut session).await.unwrap();
        let mut replay = connect(&gateway).await;
        replay.write_all(&[1]).await.unwrap();
        replay.write_all(&token).await.unwrap();
        assert_eq!(replay.read_u8().await.unwrap(), 1);
        let mut heartbeat = connect(&gateway).await;
        heartbeat.write_all(&[3]).await.unwrap();
        heartbeat.write_all(&session).await.unwrap();
        assert_eq!(heartbeat.read_u8().await.unwrap(), 0);

        let echo = TcpListener::bind((host, 0)).await.unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        let echo_task = tokio::spawn(async move {
            let (mut peer, _) = echo.accept().await.unwrap();
            let mut bytes = [0; 4];
            peer.read_exact(&mut bytes).await.unwrap();
            peer.write_all(&bytes).await.unwrap();
            peer.read_u8().await.err()
        });
        let mut tunnel = connect(&gateway).await;
        tunnel.write_all(&[2]).await.unwrap();
        tunnel.write_all(&session).await.unwrap();
        tunnel.write_u16(9).await.unwrap();
        tunnel.write_all(b"127.0.0.1").await.unwrap();
        tunnel.write_u16(echo_port).await.unwrap();
        assert_eq!(tunnel.read_u8().await.unwrap(), 0);
        tunnel.write_all(b"ping").await.unwrap();
        let mut answer = [0; 4];
        tunnel.read_exact(&mut answer).await.unwrap();
        assert_eq!(&answer, b"ping");

        let udp_echo = UdpSocket::bind((host, 0)).await.unwrap();
        let udp_port = udp_echo.local_addr().unwrap().port();
        let udp_task = tokio::spawn(async move {
            let mut payload = [0; 16];
            let mut source_port = None;
            for _ in 0..2 {
                let (length, sender) = udp_echo.recv_from(&mut payload).await.unwrap();
                if let Some(previous) = source_port { assert_eq!(sender.port(), previous); }
                source_port = Some(sender.port());
                udp_echo.send_to(&payload[..length], sender).await.unwrap();
            }
        });
        let mut udp = connect(&gateway).await;
        udp.write_all(&[4]).await.unwrap();
        udp.write_all(&session).await.unwrap();
        assert_eq!(udp.read_u8().await.unwrap(), 0);
        udp.write_all(&[0, 3, 1, 0x12, 0x34]).await.unwrap();
        let mut keepalive = [0; 5];
        udp.read_exact(&mut keepalive).await.unwrap();
        assert_eq!(keepalive, [0, 3, 1, 0x12, 0x34]);
        udp.write_u16(14).await.unwrap();
        udp.write_all(&[2, 0x12, 0x34, 1, 127, 0, 0, 1]).await.unwrap();
        udp.write_u16(udp_port).await.unwrap();
        udp.write_all(b"ping").await.unwrap();
        udp.write_u16(14).await.unwrap();
        udp.write_all(&[2, 0x12, 0x34, 1, 127, 0, 0, 1]).await.unwrap();
        udp.write_u16(udp_port).await.unwrap();
        udp.write_all(b"pong").await.unwrap();
        let mut responses = Vec::new();
        for _ in 0..2 {
            assert_eq!(udp.read_u16().await.unwrap(), 14);
            let mut udp_response = [0; 14];
            udp.read_exact(&mut udp_response).await.unwrap();
            responses.push(udp_response[10..].to_vec());
        }
        assert!(responses.contains(&b"ping".to_vec()));
        assert!(responses.contains(&b"pong".to_vec()));
        udp_task.await.unwrap();
        gateway.disconnect();
        assert!(timeout(Duration::from_secs(2), tunnel.read_u8()).await.unwrap().is_err());
        assert!(timeout(Duration::from_secs(2), udp.read_u8()).await.unwrap().is_err());
        assert!(timeout(Duration::from_secs(2), echo_task).await.unwrap().unwrap().is_some());
        let mut revoked = connect(&gateway).await;
        revoked.write_all(&[2]).await.unwrap();
        revoked.write_all(&session).await.unwrap();
        assert_eq!(revoked.read_u8().await.unwrap(), 1);
        let mut revoked_ping = connect(&gateway).await;
        revoked_ping.write_all(&[3]).await.unwrap();
        revoked_ping.write_all(&session).await.unwrap();
        assert_eq!(revoked_ping.read_u8().await.unwrap(), 1);
        gateway.stop();
    }

    #[tokio::test]
    async fn a_different_desktop_identity_fails_tls_before_pairing() {
        let expected = MobileGateway::start_inner(Ipv4Addr::LOCALHOST, 0, true, None).await.unwrap();
        let actual = MobileGateway::start_inner(Ipv4Addr::LOCALHOST, 0, true, None).await.unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(expected.certificate_der.clone())).unwrap();
        let config = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        let socket = TcpStream::connect(actual.address()).await.unwrap();
        let result = TlsConnector::from(Arc::new(config))
            .connect(ServerName::IpAddress(Ipv4Addr::LOCALHOST.into()), socket).await;
        assert!(result.is_err());
        expected.stop();
        actual.stop();
    }

    #[test]
    fn production_does_not_proxy_back_into_the_pc_loopback_or_gateway() {
        let gateway: SocketAddr = "192.168.1.5:8899".parse().unwrap();
        assert!(!allowed_destination("127.0.0.1:80".parse().unwrap(), gateway, false));
        assert!(!allowed_destination("[::1]:80".parse().unwrap(), gateway, false));
        assert!(!allowed_destination(gateway, gateway, false));
        assert!(allowed_destination("8.8.8.8:443".parse().unwrap(), gateway, false));
    }

    #[test]
    fn visible_http_request_uses_method_and_path_without_claiming_tls_body() {
        assert_eq!(observed_request(b"GET /search?q=1 HTTP/1.1\r\nHost: example.test\r\n\r\n",
            "example.test", 80), ("GET".into(), "http://example.test:80/search?q=1".into()));
        assert_eq!(observed_request(&[0x16, 0x03, 0x01, 0, 42], "example.test", 443),
            ("CONNECT".into(), "tcp://example.test:443".into()));
    }
}
