#[cfg(feature = "udpgw")]
use crate::udpgw::UdpGwClient;
use crate::{
    directions::{IncomingDataEvent, IncomingDirection, OutgoingDirection},
    http::HttpManager,
    no_proxy::NoProxyManager,
    session_info::{IpProtocol, SessionInfo},
    virtual_dns::VirtualDns,
};
pub use clap::ValueEnum;
use ipstack::{IpStackStream, IpStackTcpStream, IpStackUdpStream};
use proxy_handler::{ProxyHandler, ProxyHandlerManager};
use socks::SocksProxyManager;
pub use socks5_impl::protocol::UserKey;
#[cfg(feature = "udpgw")]
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};
use std::{
    collections::VecDeque,
    io::ErrorKind,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpSocket, TcpStream, UdpSocket},
    sync::{Mutex, mpsc::Receiver},
};
pub use tokio_util::sync::CancellationToken;
use tproxy_config::is_private_ip;
pub use tun::DEFAULT_MTU;
use udp_stream::UdpStream;
#[cfg(feature = "udpgw")]
use udpgw::{UDPGW_KEEPALIVE_TIME, UDPGW_MAX_CONNECTIONS, UdpGwClientStream, UdpGwResponse};

pub use {
    args::{ArgDns, ArgProxy, ArgVerbosity, Args, ProxyType, TcpProxyObserver},
    error::{BoxError, Error, Result},
    traffic_status::{TrafficStatus, tun2proxy_set_traffic_status_callback},
};

pub use general_api::general_run_async;

pub const FORCE_EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

mod android;
mod args;
mod directions;
mod dns;
mod dump_logger;
mod error;
mod general_api;
mod http;
mod no_proxy;
mod proxy_handler;
mod session_info;
pub mod socket_transfer;
mod socks;
mod traffic_status;
#[cfg(feature = "udpgw")]
pub mod udpgw;
mod virtual_dns;
#[doc(hidden)]
pub mod win_svc;

const DNS_PORT: u16 = 53;

#[allow(unused)]
#[derive(Hash, Copy, Clone, Eq, PartialEq, Debug)]
#[cfg_attr(
    target_os = "linux",
    derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)
)]
pub enum SocketProtocol {
    Tcp,
    Udp,
}

#[allow(unused)]
#[derive(Hash, Copy, Clone, Eq, PartialEq, Debug)]
#[cfg_attr(
    target_os = "linux",
    derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)
)]
pub enum SocketDomain {
    IpV4,
    IpV6,
}

impl From<IpAddr> for SocketDomain {
    fn from(value: IpAddr) -> Self {
        match value {
            IpAddr::V4(_) => Self::IpV4,
            IpAddr::V6(_) => Self::IpV6,
        }
    }
}

struct SocketQueue {
    tcp_v4: Mutex<Receiver<TcpSocket>>,
    tcp_v6: Mutex<Receiver<TcpSocket>>,
    udp_v4: Mutex<Receiver<UdpSocket>>,
    udp_v6: Mutex<Receiver<UdpSocket>>,
}

impl SocketQueue {
    async fn recv_tcp(&self, domain: SocketDomain) -> Result<TcpSocket, std::io::Error> {
        match domain {
            SocketDomain::IpV4 => &self.tcp_v4,
            SocketDomain::IpV6 => &self.tcp_v6,
        }
        .lock()
        .await
        .recv()
        .await
        .ok_or(ErrorKind::Other.into())
    }
    async fn recv_udp(&self, domain: SocketDomain) -> Result<UdpSocket, std::io::Error> {
        match domain {
            SocketDomain::IpV4 => &self.udp_v4,
            SocketDomain::IpV6 => &self.udp_v6,
        }
        .lock()
        .await
        .recv()
        .await
        .ok_or(ErrorKind::Other.into())
    }
}

async fn create_tcp_stream(
    socket_queue: &Option<Arc<SocketQueue>>,
    peer: SocketAddr,
) -> std::io::Result<TcpStream> {
    match &socket_queue {
        None => TcpStream::connect(peer).await,
        Some(queue) => queue.recv_tcp(peer.ip().into()).await?.connect(peer).await,
    }
}

async fn create_udp_stream(
    socket_queue: &Option<Arc<SocketQueue>>,
    peer: SocketAddr,
) -> std::io::Result<UdpStream> {
    match &socket_queue {
        None => UdpStream::connect(peer).await,
        Some(queue) => {
            let socket = queue.recv_udp(peer.ip().into()).await?;
            socket.connect(peer).await?;
            UdpStream::from_tokio(socket, peer).await
        }
    }
}

/// Run the proxy server
/// # Arguments
/// * `device` - The network device to use
/// * `mtu` - The MTU of the network device
/// * `args` - The arguments to use
/// * `shutdown_token` - The token to exit the server
/// # Returns
/// * The number of sessions while exiting
pub async fn run<D>(
    device: D,
    mtu: u16,
    args: Args,
    shutdown_token: CancellationToken,
) -> crate::Result<usize>
where
    D: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    if let Some(redirect) = args.udp_443_redirect {
        if !args.direct_udp_for_http
            || args.proxy.proxy_type != ProxyType::Http
            || !redirect.ip().is_loopback()
            || redirect.port() == 0
        {
            return Err("UDP/443 redirect requires direct HTTP UDP and a loopback listener".into());
        }
    }
    log::info!("{} {} starting...", env!("CARGO_PKG_NAME"), version_info!());
    log::info!(
        "Proxy {} server: {}",
        args.proxy.proxy_type,
        args.proxy.addr
    );

    let server_addr = args.proxy.addr;
    let key = args.proxy.credentials.clone();
    let dns_addr = args.dns_addr;
    let ipv6_enabled = args.ipv6_enabled;
    let virtual_dns = if args.dns == ArgDns::Virtual {
        Some(Arc::new(Mutex::new(VirtualDns::new(
            args.virtual_dns_pool,
            args.virtual_dns_pool_v6,
        ))))
    } else {
        None
    };

    #[cfg(target_os = "linux")]
    let socket_queue = match args.socket_transfer_fd {
        None => None,
        Some(fd) => {
            use crate::socket_transfer::{
                reconstruct_socket, reconstruct_transfer_socket, request_sockets,
            };
            use tokio::sync::mpsc::channel;

            let fd = reconstruct_socket(fd)?;
            let socket = reconstruct_transfer_socket(fd)?;
            let socket = Arc::new(Mutex::new(socket));

            macro_rules! create_socket_queue {
                ($domain:ident) => {{
                    const SOCKETS_PER_REQUEST: usize = 64;

                    let socket = socket.clone();
                    let (tx, rx) = channel(SOCKETS_PER_REQUEST);
                    tokio::spawn(async move {
                        loop {
                            let sockets = match request_sockets(
                                socket.lock().await,
                                SocketDomain::$domain,
                                SOCKETS_PER_REQUEST as u32,
                            )
                            .await
                            {
                                Ok(sockets) => sockets,
                                Err(err) => {
                                    log::warn!("Socket allocation request failed: {err}");
                                    continue;
                                }
                            };
                            for s in sockets {
                                if let Err(_) = tx.send(s).await {
                                    return;
                                }
                            }
                        }
                    });
                    Mutex::new(rx)
                }};
            }

            Some(Arc::new(SocketQueue {
                tcp_v4: create_socket_queue!(IpV4),
                tcp_v6: create_socket_queue!(IpV6),
                udp_v4: create_socket_queue!(IpV4),
                udp_v6: create_socket_queue!(IpV6),
            }))
        }
    };

    #[cfg(not(target_os = "linux"))]
    let socket_queue = None;

    use socks5_impl::protocol::Version::{V4, V5};
    let mgr: Arc<dyn ProxyHandlerManager> = match args.proxy.proxy_type {
        ProxyType::Socks5 => Arc::new(SocksProxyManager::new(server_addr, V5, key)),
        ProxyType::Socks4 => Arc::new(SocksProxyManager::new(server_addr, V4, key)),
        ProxyType::Http => Arc::new(HttpManager::new(server_addr, key)),
        ProxyType::None => Arc::new(NoProxyManager::new()),
    };

    let mut ipstack_config = ipstack::IpStackConfig::default();
    ipstack_config.mtu(mtu)?;
    let mut tcp_cfg = ipstack::TcpConfig::default();
    tcp_cfg.timeout = std::time::Duration::from_secs(args.tcp_timeout);
    ipstack_config.with_tcp_config(tcp_cfg);
    ipstack_config.udp_timeout(std::time::Duration::from_secs(args.udp_timeout));

    let mut ip_stack = ipstack::IpStack::new(ipstack_config, device);

    #[cfg(feature = "udpgw")]
    let udpgw_client = args.udpgw_server.map(|addr| {
        log::info!("UDP Gateway enabled, server: {addr}");
        use std::time::Duration;
        let client = Arc::new(UdpGwClient::new(
            mtu,
            args.udpgw_connections.unwrap_or(UDPGW_MAX_CONNECTIONS),
            args.udpgw_keepalive
                .map(Duration::from_secs)
                .unwrap_or(UDPGW_KEEPALIVE_TIME),
            args.udp_timeout,
            addr,
        ));
        let client_keepalive = client.clone();
        let shutdown_clone = shutdown_token.clone();
        tokio::spawn(async move {
            if let Err(err) = client_keepalive.heartbeat_task(shutdown_clone).await {
                log::error!("UDP Gateway heartbeat task error: {err}");
            }
        });
        client
    });

    let task_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    use std::sync::atomic::Ordering::Relaxed;
    // Keep connection tasks owned by this TUN run. Otherwise stopping the VPN
    // leaves established proxy/UDP sockets alive on the shared Tokio runtime.
    let mut sessions = tokio::task::JoinSet::new();
    let mut shutdown_requested = false;

    loop {
        let task_count = task_count.clone();
        let virtual_dns = virtual_dns.clone();
        let ip_stack_stream = tokio::select! {
            _ = shutdown_token.cancelled() => {
                log::info!("Shutdown received");
                shutdown_requested = true;
                break;
            }
            finished = sessions.join_next(), if !sessions.is_empty() => {
                if let Some(Err(error)) = finished {
                    log::warn!("Connection task failed: {error}");
                    task_count.fetch_sub(1, Relaxed);
                }
                continue;
            }
            ip_stack_stream = ip_stack.accept() => {
                ip_stack_stream?
            }
        };
        let max_sessions = args.max_sessions;
        match ip_stack_stream {
            IpStackStream::Tcp(tcp) => {
                if task_count.load(Relaxed) >= max_sessions {
                    if args.exit_on_fatal_error {
                        log::info!("Too many sessions that over {max_sessions}, exiting...");
                        break;
                    }
                    log::warn!("Too many sessions that over {max_sessions}, dropping new session");
                    continue;
                }
                let active = task_count.fetch_add(1, Relaxed).saturating_add(1);
                log::trace!("Session count {active}");
                let info = SessionInfo::new(tcp.local_addr(), tcp.peer_addr(), IpProtocol::Tcp);
                let domain_name = if let Some(virtual_dns) = &virtual_dns {
                    let mut virtual_dns = virtual_dns.lock().await;
                    virtual_dns.touch_ip(&tcp.peer_addr().ip());
                    virtual_dns.resolve_ip(&tcp.peer_addr().ip()).cloned()
                } else {
                    None
                };
                let proxy_handler = match mgr.new_proxy_handler(info, domain_name, false).await {
                    Ok(handler) => handler,
                    Err(err) => {
                        log::warn!("Failed to create TCP connection {info}: {err}");
                        task_count.fetch_sub(1, Relaxed);
                        continue;
                    }
                };
                let socket_queue = socket_queue.clone();
                let observer = args.tcp_proxy_observer.clone();
                sessions.spawn(async move {
                    if let Err(err) = handle_tcp_session(tcp, proxy_handler, socket_queue, observer).await {
                        log::error!("{info} error \"{err}\"");
                    }
                    let active = task_count.fetch_sub(1, Relaxed).saturating_sub(1);
                    log::trace!("Session count {active}");
                });
            }
            IpStackStream::Udp(udp) => {
                if task_count.load(Relaxed) >= max_sessions {
                    if args.exit_on_fatal_error {
                        log::info!("Too many sessions that over {max_sessions}, exiting...");
                        break;
                    }
                    log::warn!("Too many sessions that over {max_sessions}, dropping new session");
                    continue;
                }
                let active = task_count.fetch_add(1, Relaxed).saturating_add(1);
                log::trace!("Session count {active}");
                let mut info = SessionInfo::new(udp.local_addr(), udp.peer_addr(), IpProtocol::Udp);
                if info.dst.port() == DNS_PORT {
                    if is_private_ip(info.dst.ip()) {
                        info.dst.set_ip(dns_addr); // !!! Here we change the destination address to remote DNS server!!!
                    }
                    if args.dns == ArgDns::OverTcp {
                        info.protocol = IpProtocol::Tcp;
                        let proxy_handler = match mgr.new_proxy_handler(info, None, false).await {
                            Ok(handler) => handler,
                            Err(err) => {
                                log::warn!("Failed to create DNS-over-TCP connection {info}: {err}");
                                task_count.fetch_sub(1, Relaxed);
                                continue;
                            }
                        };
                        let socket_queue = socket_queue.clone();
                        sessions.spawn(async move {
                            if let Err(err) = handle_dns_over_tcp_session(
                                udp,
                                proxy_handler,
                                socket_queue,
                                ipv6_enabled,
                            )
                            .await
                            {
                                log::error!("{info} error \"{err}\"");
                            }
                            let active = task_count.fetch_sub(1, Relaxed).saturating_sub(1);
                            log::trace!("Session count {active}");
                        });
                        continue;
                    }
                    if args.dns == ArgDns::Virtual {
                        sessions.spawn(async move {
                            if let Some(virtual_dns) = virtual_dns {
                                if let Err(err) = handle_virtual_dns_session(udp, virtual_dns).await
                                {
                                    log::error!("{info} error \"{err}\"");
                                }
                            }
                            let active = task_count.fetch_sub(1, Relaxed).saturating_sub(1);
                            log::trace!("Session count {active}");
                        });
                        continue;
                    }
                    assert_eq!(args.dns, ArgDns::Direct);
                }
                let domain_name = if let Some(virtual_dns) = &virtual_dns {
                    let mut virtual_dns = virtual_dns.lock().await;
                    virtual_dns.touch_ip(&udp.peer_addr().ip());
                    virtual_dns.resolve_ip(&udp.peer_addr().ip()).cloned()
                } else {
                    None
                };
                #[cfg(feature = "udpgw")]
                if let Some(udpgw) = udpgw_client.clone() {
                    let tcp_src = match udp.peer_addr() {
                        SocketAddr::V4(_) => {
                            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
                        }
                        SocketAddr::V6(_) => {
                            SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0))
                        }
                    };
                    let tcpinfo =
                        SessionInfo::new(tcp_src, udpgw.get_udpgw_server_addr(), IpProtocol::Tcp);
                    let proxy_handler = match mgr.new_proxy_handler(tcpinfo, None, false).await {
                        Ok(handler) => handler,
                        Err(err) => {
                            log::warn!("Failed to create UDP gateway connection {tcpinfo}: {err}");
                            task_count.fetch_sub(1, Relaxed);
                            continue;
                        }
                    };
                    let queue = socket_queue.clone();
                    sessions.spawn(async move {
                        let dst = info.dst; // real UDP destination address
                        let dst_addr = match domain_name {
                            Some(ref d) => {
                                socks5_impl::protocol::Address::from((d.clone(), dst.port()))
                            }
                            None => dst.into(),
                        };
                        if let Err(e) = handle_udp_gateway_session(
                            udp,
                            udpgw,
                            &dst_addr,
                            proxy_handler,
                            queue,
                            ipv6_enabled,
                        )
                        .await
                        {
                            log::info!("Ending {info} with \"{e}\"");
                        }
                        let active = task_count.fetch_sub(1, Relaxed).saturating_sub(1);
                        log::trace!("Session count {active}");
                    });
                    continue;
                }
                let direct_udp =
                    args.direct_udp_for_http && args.proxy.proxy_type == ProxyType::Http;
                if direct_udp {
                    // The HTTP/3 listener derives the real origin from the TLS
                    // ClientHello SNI. Send UDP/443 there without resolving a
                    // virtual DNS address or leaking it to the network.
                    let redirect = args.udp_443_redirect.filter(|_| info.dst.port() == 443);
                    if let Some(redirect) = redirect {
                        info.dst = redirect;
                    } else if let Some(name) = domain_name.as_ref() {
                        // Virtual DNS assigns a synthetic IP. Resolve it outside
                        // the VPN before opening the direct UDP socket.
                        let original_family_is_ipv4 = info.dst.is_ipv4();
                        let resolved = tokio::net::lookup_host((name.as_str(), info.dst.port()))
                            .await
                            .ok()
                            .and_then(|mut addresses| {
                                addresses.find(|addr| addr.is_ipv4() == original_family_is_ipv4)
                            });
                        let Some(address) = resolved else {
                            log::warn!("Could not resolve direct UDP destination {name}");
                            task_count.fetch_sub(1, Relaxed);
                            continue;
                        };
                        info.dst = address;
                    }
                }
                let handler = if direct_udp {
                    NoProxyManager::new()
                        .new_proxy_handler(info, None, true)
                        .await
                } else {
                    mgr.new_proxy_handler(info, domain_name, true).await
                };
                match handler {
                    Ok(proxy_handler) => {
                        let socket_queue = socket_queue.clone();
                        sessions.spawn(async move {
                            let ty = if direct_udp {
                                ProxyType::None
                            } else {
                                args.proxy.proxy_type
                            };
                            if let Err(err) = handle_udp_associate_session(
                                udp,
                                ty,
                                proxy_handler,
                                socket_queue,
                                ipv6_enabled,
                            )
                            .await
                            {
                                log::info!("Ending {info} with \"{err}\"");
                            }
                            let active = task_count.fetch_sub(1, Relaxed).saturating_sub(1);
                            log::trace!("Session count {active}");
                        });
                    }
                    Err(e) => {
                        log::error!("Failed to create UDP connection: {e}");
                        task_count.fetch_sub(1, Relaxed);
                    }
                }
            }
            IpStackStream::UnknownTransport(u) => {
                let len = u.payload().len();
                log::info!(
                    "#0 unhandled transport - Ip Protocol {:?}, length {}",
                    u.ip_protocol(),
                    len
                );
                continue;
            }
            IpStackStream::UnknownNetwork(pkt) => {
                log::info!("#0 unknown transport - {} bytes", pkt.len());
                continue;
            }
        }
    }
    let active = task_count.load(Relaxed);
    sessions.abort_all();
    while sessions.join_next().await.is_some() {}
    Ok(if shutdown_requested { 0 } else { active })
}

async fn handle_virtual_dns_session(
    mut udp: IpStackUdpStream,
    dns: Arc<Mutex<VirtualDns>>,
) -> crate::Result<()> {
    let mut buf = [0_u8; 4096];
    loop {
        let len = match udp.read(&mut buf).await {
            Err(e) => {
                // indicate UDP read fails not an error.
                log::debug!("Virtual DNS session error: {e}");
                break;
            }
            Ok(len) => len,
        };
        if len == 0 {
            break;
        }
        let (msg, qname, ip) = dns.lock().await.generate_query(&buf[..len])?;
        udp.write_all(&msg).await?;
        log::debug!("Virtual DNS query: {qname} -> {ip}");
    }
    Ok(())
}

async fn copy_and_record_traffic<R, W>(
    reader: &mut R,
    writer: &mut W,
    is_tx: bool,
) -> tokio::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin + ?Sized,
    W: tokio::io::AsyncWrite + Unpin + ?Sized,
{
    let mut buf = vec![0; 8192];
    let mut total = 0;
    loop {
        match reader.read(&mut buf).await? {
            0 => break, // EOF
            n => {
                total += n as u64;
                let (tx, rx) = if is_tx { (n, 0) } else { (0, n) };
                if let Err(e) = crate::traffic_status::traffic_status_update(tx, rx) {
                    log::debug!("Record traffic status error: {e}");
                }
                writer.write_all(&buf[..n]).await?;
            }
        }
    }
    Ok(total)
}

async fn handle_tcp_session(
    mut tcp_stack: IpStackTcpStream,
    proxy_handler: Arc<Mutex<dyn ProxyHandler>>,
    socket_queue: Option<Arc<SocketQueue>>,
    observer: Option<TcpProxyObserver>,
) -> crate::Result<()> {
    let (session_info, server_addr) = {
        let handler = proxy_handler.lock().await;

        (handler.get_session_info(), handler.get_server_addr())
    };

    let mut server = create_tcp_stream(&socket_queue, server_addr).await?;
    if let (Some(observer), Ok(proxy_peer)) = (observer, server.local_addr()) {
        observer.notify(session_info.src, session_info.dst, proxy_peer);
    }

    log::info!("Beginning {session_info}");

    if let Err(e) = handle_proxy_session(&mut server, proxy_handler).await {
        tcp_stack.shutdown().await?;
        return Err(e);
    }

    let (mut t_rx, mut t_tx) = tokio::io::split(tcp_stack);
    let (mut s_rx, mut s_tx) = tokio::io::split(server);

    let res = tokio::join!(
        async move {
            let r = copy_and_record_traffic(&mut t_rx, &mut s_tx, true).await;
            if let Err(err) = s_tx.shutdown().await {
                log::trace!("{session_info} s_tx shutdown error {err}");
            }
            r
        },
        async move {
            let r = copy_and_record_traffic(&mut s_rx, &mut t_tx, false).await;
            if let Err(err) = t_tx.shutdown().await {
                log::trace!("{session_info} t_tx shutdown error {err}");
            }
            r
        },
    );
    log::info!("Ending {session_info} with {res:?}");

    Ok(())
}

#[cfg(feature = "udpgw")]
async fn handle_udp_gateway_session(
    mut udp_stack: IpStackUdpStream,
    udpgw_client: Arc<UdpGwClient>,
    udp_dst: &socks5_impl::protocol::Address,
    proxy_handler: Arc<Mutex<dyn ProxyHandler>>,
    socket_queue: Option<Arc<SocketQueue>>,
    ipv6_enabled: bool,
) -> crate::Result<()> {
    let proxy_server_addr = { proxy_handler.lock().await.get_server_addr() };
    let udp_mtu = udpgw_client.get_udp_mtu();
    let udp_timeout = udpgw_client.get_udp_timeout();

    let mut stream = loop {
        match udpgw_client.pop_server_connection_from_queue().await {
            Some(stream) => {
                if stream.is_closed() {
                    continue;
                } else {
                    break stream;
                }
            }
            None => {
                let mut tcp_server_stream =
                    create_tcp_stream(&socket_queue, proxy_server_addr).await?;
                if let Err(e) = handle_proxy_session(&mut tcp_server_stream, proxy_handler).await {
                    return Err(format!("udpgw connection error: {e}").into());
                }
                break UdpGwClientStream::new(tcp_server_stream);
            }
        }
    };

    let tcp_local_addr = stream.local_addr();
    let sn = stream.serial_number();

    log::info!(
        "[UdpGw] Beginning stream {} {} -> {}",
        sn,
        &tcp_local_addr,
        udp_dst
    );

    let Some(mut reader) = stream.get_reader() else {
        return Err("get reader failed".into());
    };

    let Some(mut writer) = stream.get_writer() else {
        return Err("get writer failed".into());
    };

    let mut tmp_buf = vec![0; udp_mtu.into()];

    loop {
        tokio::select! {
            len = udp_stack.read(&mut tmp_buf) => {
                let read_len = match len {
                    Ok(0) => {
                        log::info!("[UdpGw] Ending stream {} {} <> {}", sn, &tcp_local_addr, udp_dst);
                        break;
                    }
                    Ok(n) => n,
                    Err(e) => {
                        log::info!("[UdpGw] Ending stream {} {} <> {} with udp stack \"{}\"", sn, &tcp_local_addr, udp_dst, e);
                        break;
                    }
                };
                crate::traffic_status::traffic_status_update(read_len, 0)?;
                let sn = stream.serial_number();
                if let Err(e) = UdpGwClient::send_udpgw_packet(ipv6_enabled, &tmp_buf[0..read_len], udp_dst, sn, &mut writer).await {
                    log::info!("[UdpGw] Ending stream {} {} <> {} with send_udpgw_packet {}", sn, &tcp_local_addr, udp_dst, e);
                    break;
                }
                log::debug!("[UdpGw] stream {} {} -> {} send len {}", sn, &tcp_local_addr, udp_dst, read_len);
                stream.update_activity();
            }
            ret = UdpGwClient::recv_udpgw_packet(udp_mtu, udp_timeout, &mut reader) => {
                if let Ok((len, _)) = ret {
                    crate::traffic_status::traffic_status_update(0, len)?;
                }
                match ret {
                    Err(e) => {
                        log::warn!("[UdpGw] Ending stream {} {} <> {} with recv_udpgw_packet {}", sn, &tcp_local_addr, udp_dst, e);
                        stream.close();
                        break;
                    }
                    Ok((_, packet)) => match packet {
                        //should not received keepalive
                        UdpGwResponse::KeepAlive => {
                            log::error!("[UdpGw] Ending stream {} {} <> {} with recv keepalive", sn, &tcp_local_addr, udp_dst);
                            stream.close();
                            break;
                        }
                        //server udp may be timeout,can continue to receive udp data?
                        UdpGwResponse::Error => {
                            log::info!("[UdpGw] Ending stream {} {} <> {} with recv udp error", sn, &tcp_local_addr, udp_dst);
                            stream.update_activity();
                            continue;
                        }
                        UdpGwResponse::TcpClose => {
                            log::error!("[UdpGw] Ending stream {} {} <> {} with tcp closed", sn, &tcp_local_addr, udp_dst);
                            stream.close();
                            break;
                        }
                        UdpGwResponse::Data(data) => {
                            use socks5_impl::protocol::StreamOperation;
                            let len = data.len();
                            let f = data.header.flags;
                            log::debug!("[UdpGw] stream {sn} {} <- {} receive {f} len {len}", &tcp_local_addr, udp_dst);
                            if let Err(e) = udp_stack.write_all(&data.data).await {
                                log::error!("[UdpGw] Ending stream {} {} <> {} with send_udp_packet {}", sn, &tcp_local_addr, udp_dst, e);
                                break;
                            }
                        }
                    }
                }
                stream.update_activity();
            }
        }
    }

    if !stream.is_closed() {
        udpgw_client
            .store_server_connection_full(stream, reader, writer)
            .await;
    }

    Ok(())
}

async fn handle_udp_associate_session(
    mut udp_stack: IpStackUdpStream,
    proxy_type: ProxyType,
    proxy_handler: Arc<Mutex<dyn ProxyHandler>>,
    socket_queue: Option<Arc<SocketQueue>>,
    ipv6_enabled: bool,
) -> crate::Result<()> {
    use socks5_impl::protocol::{Address, StreamOperation, UdpHeader};

    let (session_info, server_addr, domain_name, udp_addr) = {
        let handler = proxy_handler.lock().await;
        (
            handler.get_session_info(),
            handler.get_server_addr(),
            handler.get_domain_name(),
            handler.get_udp_associate(),
        )
    };

    log::info!("Beginning {session_info}");

    // `_server` is meaningful here, it must be alive all the time
    // to ensure that UDP transmission will not be interrupted accidentally.
    let (_server, udp_addr) = match udp_addr {
        Some(udp_addr) => (None, udp_addr),
        None => {
            let mut server = create_tcp_stream(&socket_queue, server_addr).await?;
            let udp_addr = handle_proxy_session(&mut server, proxy_handler).await?;
            (Some(server), udp_addr.ok_or("udp associate failed")?)
        }
    };

    let mut udp_server = create_udp_stream(&socket_queue, udp_addr).await?;

    let mut buf1 = [0_u8; 4096];
    let mut buf2 = [0_u8; 4096];
    loop {
        tokio::select! {
            len = udp_stack.read(&mut buf1) => {
                let len = len?;
                if len == 0 {
                    break;
                }
                let buf1 = &buf1[..len];

                crate::traffic_status::traffic_status_update(len, 0)?;

                if let ProxyType::Socks4 | ProxyType::Socks5 = proxy_type {
                    let s5addr = if let Some(domain_name) = &domain_name {
                        Address::DomainAddress(domain_name.clone().into(), session_info.dst.port())
                    } else {
                        session_info.dst.into()
                    };

                    // Add SOCKS5 UDP header to the incoming data
                    let mut s5_udp_data = Vec::<u8>::new();
                    UdpHeader::new(0, s5addr).write_to_stream(&mut s5_udp_data)?;
                    s5_udp_data.extend_from_slice(buf1);

                    udp_server.write_all(&s5_udp_data).await?;
                } else {
                    udp_server.write_all(buf1).await?;
                }
            }
            len = udp_server.read(&mut buf2) => {
                let len = len?;
                if len == 0 {
                    break;
                }
                let buf2 = &buf2[..len];

                crate::traffic_status::traffic_status_update(0, len)?;

                if let ProxyType::Socks4 | ProxyType::Socks5 = proxy_type {
                    // Remove SOCKS5 UDP header from the server data
                    let header = UdpHeader::retrieve_from_stream(&mut &buf2[..])?;
                    let data = &buf2[header.len()..];

                    let buf = if session_info.dst.port() == DNS_PORT {
                        let mut message = dns::parse_data_to_dns_message(data, false)?;
                        if !ipv6_enabled {
                            dns::remove_ipv6_entries(&mut message);
                        }
                        message.to_vec()?
                    } else {
                        data.to_vec()
                    };

                    udp_stack.write_all(&buf).await?;
                } else {
                    udp_stack.write_all(buf2).await?;
                }
            }
        }
    }

    log::info!("Ending {session_info}");

    Ok(())
}

async fn handle_dns_over_tcp_session(
    mut udp_stack: IpStackUdpStream,
    proxy_handler: Arc<Mutex<dyn ProxyHandler>>,
    socket_queue: Option<Arc<SocketQueue>>,
    ipv6_enabled: bool,
) -> crate::Result<()> {
    let (session_info, server_addr) = {
        let handler = proxy_handler.lock().await;

        (handler.get_session_info(), handler.get_server_addr())
    };

    let mut server = create_tcp_stream(&socket_queue, server_addr).await?;

    log::info!("Beginning {session_info}");

    let _ = handle_proxy_session(&mut server, proxy_handler).await?;

    let mut buf1 = [0_u8; 4096];
    let mut buf2 = [0_u8; 4096];
    loop {
        tokio::select! {
            len = udp_stack.read(&mut buf1) => {
                let len = len?;
                if len == 0 {
                    break;
                }
                let buf1 = &buf1[..len];

                _ = dns::parse_data_to_dns_message(buf1, false)?;

                // Insert the DNS message length in front of the payload
                let len = u16::try_from(buf1.len())?;
                let mut buf = Vec::with_capacity(std::mem::size_of::<u16>() + usize::from(len));
                buf.extend_from_slice(&len.to_be_bytes());
                buf.extend_from_slice(buf1);

                server.write_all(&buf).await?;

                crate::traffic_status::traffic_status_update(buf.len(), 0)?;
            }
            len = server.read(&mut buf2) => {
                let len = len?;
                if len == 0 {
                    break;
                }
                let mut buf = buf2[..len].to_vec();

                crate::traffic_status::traffic_status_update(0, len)?;

                let mut to_send: VecDeque<Vec<u8>> = VecDeque::new();
                loop {
                    if buf.len() < 2 {
                        break;
                    }
                    let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
                    if buf.len() < len + 2 {
                        break;
                    }

                    // remove the length field
                    let data = buf[2..len + 2].to_vec();

                    let mut message = dns::parse_data_to_dns_message(&data, false)?;

                    let name = dns::extract_domain_from_dns_message(&message)?;
                    let ip = dns::extract_ipaddr_from_dns_message(&message);
                    log::trace!("DNS over TCP query result: {name} -> {ip:?}");

                    if !ipv6_enabled {
                        dns::remove_ipv6_entries(&mut message);
                    }

                    to_send.push_back(message.to_vec()?);
                    if len + 2 == buf.len() {
                        break;
                    }
                    buf = buf[len + 2..].to_vec();
                }

                while let Some(packet) = to_send.pop_front() {
                    udp_stack.write_all(&packet).await?;
                }
            }
        }
    }

    log::info!("Ending {session_info}");

    Ok(())
}

/// This function is used to handle the business logic of tun2proxy and SOCKS5 server.
/// When handling UDP proxy, the return value UDP associate IP address is the result of this business logic.
/// However, when handling TCP business logic, the return value Ok(None) is meaningless, just indicating that the operation was successful.
async fn handle_proxy_session(
    server: &mut TcpStream,
    proxy_handler: Arc<Mutex<dyn ProxyHandler>>,
) -> crate::Result<Option<SocketAddr>> {
    let mut launched = false;
    let mut proxy_handler = proxy_handler.lock().await;
    let dir = OutgoingDirection::ToServer;
    let (mut tx, mut rx) = (0, 0);

    loop {
        if proxy_handler.connection_established() {
            break;
        }

        if !launched {
            let data = proxy_handler.peek_data(dir).buffer;
            let len = data.len();
            if len == 0 {
                return Err("proxy_handler launched went wrong".into());
            }
            server.write_all(data).await?;
            proxy_handler.consume_data(dir, len);
            tx += len;

            launched = true;
        }

        let mut buf = [0_u8; 4096];
        let len = server.read(&mut buf).await?;
        if len == 0 {
            return Err("server closed accidentially".into());
        }
        rx += len;
        let event = IncomingDataEvent {
            direction: IncomingDirection::FromServer,
            buffer: &buf[..len],
        };
        proxy_handler.push_data(event).await?;

        let data = proxy_handler.peek_data(dir).buffer;
        let len = data.len();
        if len > 0 {
            server.write_all(data).await?;
            proxy_handler.consume_data(dir, len);
            tx += len;
        }
    }
    crate::traffic_status::traffic_status_update(tx, rx)?;
    Ok(proxy_handler.get_udp_associate())
}

#[cfg(test)]
mod mobile_fault_tests {
    use super::*;
    use etherparse::{PacketBuilder, SlicedPacket, TransportSlice};
    use std::{net::Ipv6Addr, time::Duration};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn unsupported_udp_does_not_consume_session_capacity() {
        let (device, mut peer) = tokio::io::duplex(4096);
        let mut args = Args::default();
        args.proxy = ArgProxy::try_from("http://127.0.0.1:18080").unwrap();
        args.max_sessions = 1;
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(run(device, 1500, args, shutdown.clone()));

        let payload = b"test";
        let packet = PacketBuilder::ipv4([10, 0, 0, 2], [203, 0, 113, 1], 64).udp(41000, 9999);
        let mut bytes = Vec::with_capacity(packet.size(payload.len()));
        packet.write(&mut bytes, payload).unwrap();
        peer.write_all(&bytes).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        shutdown.cancel();
        let sessions = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("TUN loop should stop")
            .expect("TUN task should not panic")
            .expect("unsupported UDP must not fail the VPN loop");
        assert_eq!(sessions, 0, "rejected UDP must release its session slot");
    }

    #[tokio::test]
    async fn http_proxy_mode_forwards_non_dns_udp_both_ways() {
        let upstream = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = upstream.local_addr().unwrap().port();
        let (device, mut peer) = tokio::io::duplex(4096);
        let mut args = Args::default();
        args.proxy = ArgProxy::try_from("http://127.0.0.1:18080").unwrap();
        args.dns = ArgDns::Virtual;
        args.direct_udp_for_http = true;
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(run(device, 1500, args, shutdown.clone()));

        let payload = b"ping";
        let packet = PacketBuilder::ipv4([10, 0, 0, 2], [127, 0, 0, 1], 64).udp(41001, port);
        let mut bytes = Vec::with_capacity(packet.size(payload.len()));
        packet.write(&mut bytes, payload).unwrap();
        peer.write_all(&bytes).await.unwrap();

        let mut received = [0_u8; 64];
        let (size, source) = tokio::time::timeout(
            Duration::from_secs(2),
            upstream.recv_from(&mut received),
        )
        .await
        .expect("UDP should reach the upstream socket")
        .unwrap();
        assert_eq!(&received[..size], payload);
        upstream.send_to(b"pong", source).await.unwrap();

        let mut response = [0_u8; 1500];
        let size = tokio::time::timeout(Duration::from_secs(2), peer.read(&mut response))
            .await
            .expect("UDP reply should return through the TUN")
            .unwrap();
        let packet = SlicedPacket::from_ip(&response[..size]).unwrap();
        match packet.transport {
            Some(TransportSlice::Udp(udp)) => assert_eq!(udp.payload(), b"pong"),
            other => panic!("expected a UDP reply, got {other:?}"),
        }

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("TUN loop should stop")
            .expect("TUN task should not panic")
            .expect("UDP relay must not fail the VPN loop");
    }

    #[tokio::test]
    async fn http_proxy_mode_redirects_udp_443_to_local_h3_ingress() {
        let ingress = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (device, mut peer) = tokio::io::duplex(4096);
        let mut args = Args::default();
        args.proxy = ArgProxy::try_from("http://127.0.0.1:18080").unwrap();
        args.direct_udp_for_http = true;
        args.udp_443_redirect = Some(ingress.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(run(device, 1500, args, shutdown.clone()));

        let payload = b"quic-packet";
        let original = [203, 0, 113, 7];
        let packet = PacketBuilder::ipv4([10, 0, 0, 2], original, 64).udp(41003, 443);
        let mut bytes = Vec::with_capacity(packet.size(payload.len()));
        packet.write(&mut bytes, payload).unwrap();
        peer.write_all(&bytes).await.unwrap();

        let mut received = [0_u8; 64];
        let (size, source) = tokio::time::timeout(
            Duration::from_secs(2),
            ingress.recv_from(&mut received),
        )
        .await
        .expect("UDP/443 must reach the local H3 ingress")
        .unwrap();
        assert_eq!(&received[..size], payload);
        ingress.send_to(b"h3-response", source).await.unwrap();

        let mut response = [0_u8; 1500];
        let size = tokio::time::timeout(Duration::from_secs(2), peer.read(&mut response))
            .await
            .expect("H3 reply must return through the TUN")
            .unwrap();
        let packet = SlicedPacket::from_ip(&response[..size]).unwrap();
        match packet.transport {
            Some(TransportSlice::Udp(udp)) => {
                assert_eq!(udp.payload(), b"h3-response");
                assert_eq!(udp.source_port(), 443);
            }
            other => panic!("expected a UDP reply, got {other:?}"),
        }

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("TUN loop should stop")
            .expect("TUN task should not panic")
            .expect("UDP redirect must not fail the VPN loop");
    }

    #[tokio::test]
    async fn http_proxy_mode_redirects_ipv6_udp_443_to_ipv4_loopback() {
        let ingress = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (device, mut peer) = tokio::io::duplex(4096);
        let mut args = Args::default();
        args.proxy = ArgProxy::try_from("http://127.0.0.1:18080").unwrap();
        args.direct_udp_for_http = true;
        args.ipv6_enabled = true;
        args.udp_443_redirect = Some(ingress.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(run(device, 1500, args, shutdown.clone()));

        let source = "fd00:198:18::2".parse::<Ipv6Addr>().unwrap().octets();
        let target = "2001:db8::7".parse::<Ipv6Addr>().unwrap().octets();
        let payload = b"quic6";
        let packet = PacketBuilder::ipv6(source, target, 64).udp(41004, 443);
        let mut bytes = Vec::with_capacity(packet.size(payload.len()));
        packet.write(&mut bytes, payload).unwrap();
        peer.write_all(&bytes).await.unwrap();

        let mut received = [0_u8; 64];
        let (size, sender) = tokio::time::timeout(Duration::from_secs(2), ingress.recv_from(&mut received))
            .await
            .expect("IPv6 UDP/443 must reach IPv4 loopback")
            .unwrap();
        assert_eq!(&received[..size], payload);
        ingress.send_to(b"h3-6", sender).await.unwrap();

        let mut response = [0_u8; 1500];
        let size = tokio::time::timeout(Duration::from_secs(2), peer.read(&mut response))
            .await
            .expect("IPv6 H3 reply must return through the TUN")
            .unwrap();
        let packet = SlicedPacket::from_ip(&response[..size]).unwrap();
        match packet.transport {
            Some(TransportSlice::Udp(udp)) => {
                assert_eq!(udp.payload(), b"h3-6");
                assert_eq!(udp.source_port(), 443);
            }
            other => panic!("expected an IPv6 UDP reply, got {other:?}"),
        }

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("TUN loop should stop")
            .expect("TUN task should not panic")
            .expect("IPv6 UDP redirect must not fail the VPN loop");
    }

    #[tokio::test]
    async fn http_proxy_mode_forwards_ipv6_udp_both_ways() {
        let upstream = UdpSocket::bind("[::1]:0").await.unwrap();
        let port = upstream.local_addr().unwrap().port();
        let (device, mut peer) = tokio::io::duplex(4096);
        let mut args = Args::default();
        args.proxy = ArgProxy::try_from("http://127.0.0.1:18080").unwrap();
        args.dns = ArgDns::Virtual;
        args.direct_udp_for_http = true;
        args.ipv6_enabled = true;
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(run(device, 1500, args, shutdown.clone()));

        let payload = b"ping6";
        let source = "fd00:198:18::2".parse::<Ipv6Addr>().unwrap().octets();
        let packet = PacketBuilder::ipv6(source, Ipv6Addr::LOCALHOST.octets(), 64)
            .udp(41002, port);
        let mut bytes = Vec::with_capacity(packet.size(payload.len()));
        packet.write(&mut bytes, payload).unwrap();
        peer.write_all(&bytes).await.unwrap();

        let mut received = [0_u8; 64];
        let (size, source) = tokio::time::timeout(
            Duration::from_secs(2),
            upstream.recv_from(&mut received),
        )
        .await
        .expect("IPv6 UDP should reach the upstream socket")
        .unwrap();
        assert_eq!(&received[..size], payload);
        upstream.send_to(b"pong6", source).await.unwrap();

        let mut response = [0_u8; 1500];
        let size = tokio::time::timeout(Duration::from_secs(2), peer.read(&mut response))
            .await
            .expect("IPv6 UDP reply should return through the TUN")
            .unwrap();
        let packet = SlicedPacket::from_ip(&response[..size]).unwrap();
        match packet.transport {
            Some(TransportSlice::Udp(udp)) => assert_eq!(udp.payload(), b"pong6"),
            other => panic!("expected an IPv6 UDP reply, got {other:?}"),
        }

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("TUN loop should stop")
            .expect("TUN task should not panic")
            .expect("IPv6 UDP relay must not fail the VPN loop");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tun_tcp_connect_reaches_local_http_proxy() {
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (device, mut peer) = tokio::io::duplex(4096);
        let mut args = Args::default();
        args.proxy = ArgProxy::try_from(
            format!("http://{}", proxy.local_addr().unwrap()).as_str(),
        )
        .unwrap();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(run(device, 1500, args, shutdown.clone()));

        let source = [10, 0, 0, 2];
        let destination = [203, 0, 113, 1];
        let syn = PacketBuilder::ipv4(source, destination, 64)
            .tcp(41003, 443, 42, 65535)
            .syn();
        let mut packet = Vec::with_capacity(syn.size(0));
        syn.write(&mut packet, &[]).unwrap();
        peer.write_all(&packet).await.unwrap();

        let mut response = [0_u8; 1500];
        let size = tokio::time::timeout(Duration::from_secs(2), peer.read(&mut response))
            .await
            .expect("TUN TCP handshake should receive SYN-ACK")
            .unwrap();
        let server_sequence = {
            let syn_ack = SlicedPacket::from_ip(&response[..size]).unwrap();
            match syn_ack.transport {
                Some(TransportSlice::Tcp(tcp)) if tcp.syn() => tcp.sequence_number(),
                other => panic!("expected SYN-ACK, got {other:?}"),
            }
        };
        let ack = PacketBuilder::ipv4(source, destination, 64)
            .tcp(41003, 443, 43, 65535)
            .ack(server_sequence.wrapping_add(1));
        packet.clear();
        ack.write(&mut packet, &[]).unwrap();
        peer.write_all(&packet).await.unwrap();

        let (mut upstream, _) = tokio::time::timeout(Duration::from_secs(2), proxy.accept())
            .await
            .expect("TUN TCP session should connect to local HTTP proxy")
            .unwrap();
        let mut request = [0_u8; 1024];
        let size = tokio::time::timeout(Duration::from_secs(2), upstream.read(&mut request))
            .await
            .expect("proxy should receive CONNECT")
            .unwrap();
        assert!(
            request[..size].starts_with(b"CONNECT 203.0.113.1:443 HTTP/1.1\r\n"),
            "unexpected proxy request: {:?}",
            String::from_utf8_lossy(&request[..size])
        );
        upstream
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await
            .unwrap();
        let data = PacketBuilder::ipv4(source, destination, 64)
            .tcp(41003, 443, 43, 65535)
            .psh()
            .ack(server_sequence.wrapping_add(1));
        packet.clear();
        data.write(&mut packet, b"hello").unwrap();
        peer.write_all(&packet).await.unwrap();
        let size = tokio::time::timeout(Duration::from_secs(2), upstream.read(&mut request))
            .await
            .expect("proxy should receive TCP payload")
            .unwrap();
        assert_eq!(&request[..size], b"hello");

        upstream.write_all(b"world").await.unwrap();
        let echoed = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let size = peer.read(&mut response).await.unwrap();
                let packet = SlicedPacket::from_ip(&response[..size]).unwrap();
                if let Some(TransportSlice::Tcp(tcp)) = packet.transport {
                    if !tcp.payload().is_empty() {
                        break tcp.payload().to_vec();
                    }
                }
            }
        })
        .await
        .expect("TCP response should return through the TUN");
        assert_eq!(echoed, b"world");

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("TUN loop should stop")
            .expect("TUN task should not panic")
            .expect("TCP setup must not fail the VPN loop");
        let closed = tokio::time::timeout(Duration::from_secs(2), upstream.read(&mut request))
            .await
            .expect("stopping the VPN should close its active proxy connection")
            .expect("proxy connection should close cleanly");
        assert_eq!(closed, 0, "old proxy session survived TUN shutdown");
    }
}
