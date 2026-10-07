//! Per-request SOCKS5 UDP association. The loopback bridge carries only QUIC ciphertext.
use anyhow::{ensure, Context, Result};
use std::net::{IpAddr, SocketAddr};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpStream, UdpSocket}};

pub(crate) struct Tunnel { pub endpoint: quinn::Endpoint, pub peer: SocketAddr, task: tokio::task::JoinHandle<Result<()>> }
impl Drop for Tunnel { fn drop(&mut self) { self.task.abort(); } }
impl Tunnel {
    pub async fn stopped(&mut self) -> anyhow::Error {
        match (&mut self.task).await {
            Ok(Err(error)) => error,
            Ok(Ok(())) => anyhow::anyhow!("SOCKS5 UDP association closed"),
            Err(_) => anyhow::anyhow!("SOCKS5 UDP relay stopped"),
        }
    }
}

fn destination(url: &url::Url) -> Result<Vec<u8>> {
    let mut out = vec![0, 0, 0];
    match url.host().context("Missing UDP target")? {
        url::Host::Ipv4(ip) => { out.push(1); out.extend(ip.octets()); }
        url::Host::Ipv6(ip) => { out.push(4); out.extend(ip.octets()); }
        url::Host::Domain(host) => { ensure!(host.len() <= 255, "SOCKS5 domain too long"); out.extend([3, host.len() as u8]); out.extend(host.as_bytes()); }
    }
    out.extend(url.port_or_known_default().context("Missing UDP port")?.to_be_bytes());
    Ok(out)
}

fn payload(packet: &[u8], port: u16) -> Option<&[u8]> {
    if packet.get(..3)? != [0,0,0] { return None; } // No fragmented SOCKS datagrams.
    let end = match *packet.get(3)? { 1 => 8, 4 => 20, 3 => 5 + *packet.get(4)? as usize, _ => return None };
    if u16::from_be_bytes(packet.get(end..end+2)?.try_into().ok()?) != port { return None; }
    packet.get(end+2..)
}

pub(crate) async fn open(target: &url::Url, proxy: &crate::upstream::UpstreamProxy) -> Result<Tunnel> {
    ensure!(proxy.url.scheme() == "socks5", "HTTP/3 上游仅支持 SOCKS5 UDP ASSOCIATE；HTTP CONNECT 不支持 UDP，未回退直连");
    let config = proxy.for_helper().await?;
    let url = url::Url::parse(&config.url)?;
    let mut tcp = TcpStream::connect((crate::http1::hostname(&url).as_str(), url.port().unwrap_or(1080))).await.context("SOCKS5 UDP control connection failed")?;
    let method = if config.username.is_empty() { 0 } else { 2 };
    tcp.write_all(&[5,1,method]).await?;
    let mut reply = [0;2]; tcp.read_exact(&mut reply).await?;
    ensure!(reply == [5,method], "SOCKS5 UDP authentication method rejected; no direct fallback");
    if method == 2 {
        let mut auth = vec![1,config.username.len() as u8]; auth.extend(config.username.as_bytes());
        auth.push(config.password.len() as u8); auth.extend(config.password.as_bytes());
        tcp.write_all(&auth).await?; tcp.read_exact(&mut reply).await?;
        ensure!(reply == [1,0], "SOCKS5 UDP authentication failed");
    }
    // The relay learns the actual client UDP endpoint from its first datagram.
    tcp.write_all(&[5,3,0,1,0,0,0,0,0,0]).await?;
    let mut head = [0;4]; tcp.read_exact(&mut head).await?;
    ensure!(head[0]==5 && head[2]==0, "Invalid SOCKS5 UDP response");
    ensure!(head[1]==0, "SOCKS5 UDP ASSOCIATE rejected (code {}); no direct fallback", head[1]);
    let host = match head[3] {
        1 => { let mut v=[0;4]; tcp.read_exact(&mut v).await?; IpAddr::from(v).to_string() }
        4 => { let mut v=[0;16]; tcp.read_exact(&mut v).await?; IpAddr::from(v).to_string() }
        3 => { let n=tcp.read_u8().await?; let mut v=vec![0;n as usize]; tcp.read_exact(&mut v).await?; String::from_utf8(v)? }
        _ => anyhow::bail!("Invalid SOCKS5 UDP address type"),
    };
    let port=tcp.read_u16().await?; ensure!(port!=0, "SOCKS5 UDP relay returned port zero");
    let relay = if host.parse::<IpAddr>().is_ok_and(|ip| ip.is_unspecified()) {
        SocketAddr::new(tcp.peer_addr()?.ip(),port)
    } else { tokio::net::lookup_host((host.as_str(),port)).await?.next().context("No SOCKS5 UDP relay address")? };
    let remote=UdpSocket::bind(if relay.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }).await?;
    remote.connect(relay).await?;
    let local=UdpSocket::bind("127.0.0.1:0").await?;
    let peer=local.local_addr()?;
    let endpoint=quinn::Endpoint::client("127.0.0.1:0".parse()?)?;
    local.connect(endpoint.local_addr()?).await?;
    let prefix=destination(target)?;
    let target_port=target.port_or_known_default().unwrap();
    let task=tokio::spawn(async move {
        let mut outbound=vec![0;65535]; let mut inbound=vec![0;65535]; let mut control=[0;1];
        loop {
            tokio::select! {
                result=tcp.read(&mut control) => { result?; anyhow::bail!("SOCKS5 UDP control connection closed; no direct fallback"); }
                result=local.recv(&mut outbound) => {
                    let n=result?; let mut packet=Vec::with_capacity(prefix.len()+n);
                    packet.extend(&prefix); packet.extend(&outbound[..n]); remote.send(&packet).await?;
                }
                result=remote.recv(&mut inbound) => {
                    let n=result?;
                    if let Some(bytes)=payload(&inbound[..n],target_port) { local.send(bytes).await?; }
                }
            }
        }
    });
    Ok(Tunnel{endpoint,peer,task})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn domain_is_sent_to_proxy_and_drop_releases_association() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let state=crate::upstream::UpstreamState::default();
            state.update(crate::upstream::UpstreamInput{enabled:true,url:format!("socks5://{}",listener.local_addr().unwrap()),username:String::new(),password:None,auth_enabled:false}).unwrap();
            let server=tokio::spawn(async move {
                let (mut tcp,_)=listener.accept().await.unwrap();
                let mut hello=[0;3]; tcp.read_exact(&mut hello).await.unwrap();
                tcp.write_all(&[5,0]).await.unwrap();
                let mut command=[0;10]; tcp.read_exact(&mut command).await.unwrap();
                let socket=UdpSocket::bind("127.0.0.1:0").await.unwrap();
                let mut reply=vec![5,0,0,1,127,0,0,1]; reply.extend(socket.local_addr().unwrap().port().to_be_bytes());
                tcp.write_all(&reply).await.unwrap();
                let mut packet=[0;2048]; let (n,_)=socket.recv_from(&mut packet).await.unwrap();
                assert_eq!(&packet[..4], &[0,0,0,3]);
                let length=packet[4] as usize;
                assert_eq!(&packet[5..5+length], b"no-local-dns.invalid");
                assert!(n>length+7);
                let mut end=[0;1]; assert_eq!(tcp.read(&mut end).await.unwrap(),0);
            });
            let mut tunnel=open(&"https://no-local-dns.invalid/".parse().unwrap(),&state.snapshot().unwrap()).await.unwrap();
            let roots=rustls::RootCertStore::empty();
            let tls=crate::tls_config::config(&crate::model::TlsProfile::default(),&roots,"h3").unwrap();
            tunnel.endpoint.set_default_client_config(quinn::ClientConfig::new(std::sync::Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap())));
            let connection=tunnel.endpoint.connect(tunnel.peer,"no-local-dns.invalid").unwrap();
            // Cancel while the handshake is pending. Drop must close the TCP association.
            assert!(tokio::time::timeout(std::time::Duration::from_millis(100),connection).await.is_err());
            drop(tunnel);
            server.await.unwrap();
        }).await.unwrap();
    }
    #[tokio::test]
    async fn associate_rejection_and_control_close_are_errors() {
        for rejected in [true,false] {
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let state=crate::upstream::UpstreamState::default();
            state.update(crate::upstream::UpstreamInput{enabled:true,url:format!("socks5://{}",listener.local_addr().unwrap()),username:String::new(),password:None,auth_enabled:false}).unwrap();
            let server=tokio::spawn(async move {
                let (mut tcp,_)=listener.accept().await.unwrap();
                let mut hello=[0;3]; tcp.read_exact(&mut hello).await.unwrap(); assert_eq!(hello,[5,1,0]);
                tcp.write_all(&[5,0]).await.unwrap();
                let mut command=[0;10]; tcp.read_exact(&mut command).await.unwrap(); assert_eq!(command[1],3);
                let socket=UdpSocket::bind("127.0.0.1:0").await.unwrap();
                let mut reply=vec![5,if rejected{7}else{0},0,1,0,0,0,0]; reply.extend(socket.local_addr().unwrap().port().to_be_bytes());
                tcp.write_all(&reply).await.unwrap();
            });
            let result=open(&"https://no-local-dns.invalid/".parse().unwrap(),&state.snapshot().unwrap()).await;
            if rejected { assert!(result.err().unwrap().to_string().contains("code 7")); }
            else {
                let mut tunnel=result.unwrap();
                let error=tokio::time::timeout(std::time::Duration::from_secs(2),tunnel.stopped()).await.unwrap();
                assert!(error.to_string().contains("control connection closed"));
            }
            server.await.unwrap();
        }
    }
    #[test]
    fn udp_envelopes_validate_fragmentation_lengths_and_port() {
        for url in ["https://example.org/", "https://127.0.0.1/", "https://[::1]/"] {
            let mut bytes=destination(&url.parse().unwrap()).unwrap(); bytes.extend([1,2,3]);
            assert_eq!(payload(&bytes,443),Some(&[1,2,3][..]));
            assert!(payload(&bytes,80).is_none());
            for n in 0..bytes.len()-3 { assert!(payload(&bytes[..n],443).is_none()); }
            bytes[2]=1; assert!(payload(&bytes,443).is_none());
        }
    }
}
