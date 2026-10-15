use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::net::{IpAddr, SocketAddr};

pub async fn check_listener_port(host: &str, port: u16) -> Result<()> {
    ensure!(port >= 1024, "端口必须在 1024–65535 之间");
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    let address: SocketAddr = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
        .parse().context("监听 IP 无效")?;
    let listener = tokio::net::TcpListener::bind(address).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::AddrInUse {
            anyhow::anyhow!("端口 {port} 已被占用，请使用其他端口")
        } else { anyhow::anyhow!("无法监听 {address}：{error}") }
    })?;
    drop(listener);
    Ok(())
}

#[cfg(test)]
mod port_tests {
    use super::*;

    #[tokio::test]
    async fn detects_occupied_ports_and_releases_probe_socket() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(check_listener_port("127.0.0.1", port).await.unwrap_err().to_string().contains("已被占用"));
        drop(listener);
        check_listener_port("127.0.0.1", port).await.unwrap();
        let _listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_invalid_range_and_host() {
        assert!(check_listener_port("127.0.0.1", 0).await.is_err());
        assert!(check_listener_port("127.0.0.1", 1023).await.is_err());
        assert!(check_listener_port("invalid", 8080).await.is_err());
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all="camelCase")]
pub struct InterfaceAddress {
    pub name: String,
    pub address: String,
    pub family: &'static str,
    pub available: bool,
    pub loopback: bool,
}

pub fn interfaces() -> Result<Vec<InterfaceAddress>> {
    let mut result = Vec::new();
    for iface in if_addrs::get_if_addrs().context("无法读取本机网卡列表")? {
        let ip = iface.ip();
        if ip.is_unspecified() || ip.is_multicast() { continue; }
        let link_local = matches!(ip, IpAddr::V6(v) if v.is_unicast_link_local());
        let address = if link_local {
            match iface.index { Some(index) if index > 0 => format!("{ip}%{index}"), _ => continue }
        } else { ip.to_string() };
        result.push(InterfaceAddress { available: iface.is_oper_up(), loopback: iface.is_loopback(), name: iface.name, address,
            family: if ip.is_ipv4() {"IPv4"} else {"IPv6"} });
    }
    result.sort_by(|a,b| (!a.available,a.loopback,&a.name,a.family,&a.address).cmp(&(!b.available,b.loopback,&b.name,b.family,&b.address)));
    result.dedup_by(|a,b| a.name==b.name && a.address==b.address);
    Ok(result)
}

/// Accept IP literals only. IPv6 link-local addresses require a numeric interface scope.
pub fn listener_address(host: &str, port: u16) -> Result<SocketAddr> {
    let host=host.trim();
    let address: SocketAddr = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
        .parse().context("监听地址必须是 IPv4 或 IPv6；IPv6 链路本地地址需带 %网卡索引")?;
    ensure!(!address.ip().is_multicast(), "监听地址不能是组播地址");
    if let SocketAddr::V6(v)=address {
        ensure!(!v.ip().is_unicast_link_local() || v.scope_id()!=0, "IPv6 链路本地地址需带 %网卡索引，请从网卡列表选择");
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn listener_literals_and_ipv6_scope() {
        assert_eq!(listener_address("192.168.1.2",8080).unwrap().to_string(),"192.168.1.2:8080");
        assert_eq!(listener_address("fe80::1%12",8080).unwrap().to_string(),"[fe80::1%12]:8080");
        assert!(listener_address("fe80::1",8080).is_err());
        assert!(listener_address("localhost",8080).is_err());
        assert!(listener_address("224.0.0.1",8080).is_err());
        assert!(listener_address("::1",8080).is_ok());
    }
    #[test]
    fn enumerate_local_interfaces() {
        let interfaces=interfaces().unwrap();
        assert!(!interfaces.is_empty());
        for item in interfaces { assert!(!item.name.is_empty()); assert!(listener_address(&item.address,0).is_ok()); }
    }
}
