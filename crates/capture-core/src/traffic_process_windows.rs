use std::ffi::OsString;
use std::mem::{offset_of, size_of};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::os::windows::ffi::OsStringExt;
use std::path::Path;
use std::ptr;
use std::time::Instant;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, MIB_TCP_STATE_ESTAB, TCP_TABLE_OWNER_PID_CONNECTIONS,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::model::ProcessInfo;

const MAX_TABLE_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn lookup(peer: SocketAddr, local: SocketAddr, deadline: Instant) -> Option<ProcessInfo> {
    let pid = connection_pid(peer, local, deadline)?;
    if Instant::now() >= deadline {
        return None;
    }
    let raw_handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw_handle.is_null() {
        return None;
    }
    // Retain the native process object until the final connection recheck.
    // Dropping it after querying the image would leave a PID-reuse window.
    let handle = ProcessHandle(raw_handle);
    let process = process_info(pid, &handle)?;
    // The handle and image lookup take time. Recheck the complete connection
    // rather than returning a stale PID after the client has gone away.
    if Instant::now() >= deadline || connection_pid(peer, local, deadline)? != pid {
        return None;
    }
    Some(process)
}

fn connection_pid(peer: SocketAddr, local: SocketAddr, deadline: Instant) -> Option<u32> {
    if Instant::now() >= deadline {
        return None;
    }
    let family = if peer.is_ipv4() { AF_INET } else { AF_INET6 };
    let table = tcp_table(u32::from(family), deadline)?;
    let bytes = unsafe {
        // Vec<u32> provides the alignment required by the native TCP tables.
        std::slice::from_raw_parts(table.as_ptr().cast::<u8>(), table.len() * size_of::<u32>())
    };
    if peer.is_ipv4() {
        table_pid::<MIB_TCPROW_OWNER_PID>(
            bytes,
            offset_of!(MIB_TCPTABLE_OWNER_PID, table),
            |row| ipv4_row_pid(row, peer, local),
        )
    } else {
        table_pid::<MIB_TCP6ROW_OWNER_PID>(
            bytes,
            offset_of!(MIB_TCP6TABLE_OWNER_PID, table),
            |row| ipv6_row_pid(row, peer, local),
        )
    }
}

fn tcp_table(family: u32, deadline: Instant) -> Option<Vec<u32>> {
    let mut bytes = 0;
    let result = unsafe {
        GetExtendedTcpTable(
            ptr::null_mut(), &mut bytes, 0, family, TCP_TABLE_OWNER_PID_CONNECTIONS, 0,
        )
    };
    if result != ERROR_INSUFFICIENT_BUFFER {
        return None;
    }
    // The table can grow between the size probe and the snapshot. Bound both
    // allocation and retries instead of following a rapidly growing table.
    for _ in 0..3 {
        if Instant::now() >= deadline || bytes < 4 || bytes as usize > MAX_TABLE_BYTES {
            return None;
        }
        let words = (bytes as usize).div_ceil(size_of::<u32>());
        let mut table = vec![0_u32; words];
        let mut actual_bytes = (words * size_of::<u32>()) as u32;
        let result = unsafe {
            GetExtendedTcpTable(
                table.as_mut_ptr().cast(), &mut actual_bytes, 0,
                family, TCP_TABLE_OWNER_PID_CONNECTIONS, 0,
            )
        };
        if result == 0 {
            if actual_bytes as usize > table.len() * size_of::<u32>() || actual_bytes < 4 {
                return None;
            }
            // All native row layouts are multiples of four bytes.
            table.truncate((actual_bytes as usize).div_ceil(size_of::<u32>()));
            return Some(table);
        }
        if result != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
        bytes = actual_bytes;
    }
    None
}

fn table_pid<T: Copy>(
    bytes: &[u8], offset: usize, match_row: impl Fn(T) -> Option<u32>,
) -> Option<u32> {
    let count = u32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    let needed = offset.checked_add(count.checked_mul(size_of::<T>())?)?;
    if needed > bytes.len() {
        return None;
    }
    let mut matched = None;
    for row in bytes.get(offset..needed)?.chunks_exact(size_of::<T>()) {
        // Tests may provide unaligned byte slices; reading unaligned is also
        // safe for native row padding and does not borrow foreign memory.
        let row = unsafe { ptr::read_unaligned(row.as_ptr().cast::<T>()) };
        if let Some(pid) = match_row(row) {
            if matched.is_some() && matched != Some(pid) {
                return None;
            }
            matched = Some(pid);
        }
    }
    matched
}

fn port(raw: u32) -> u16 {
    u16::from_be(raw as u16)
}

fn ipv4_row_pid(row: MIB_TCPROW_OWNER_PID, peer: SocketAddr, local: SocketAddr) -> Option<u32> {
    let row_local = SocketAddr::from((
        Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes()), port(row.dwLocalPort),
    ));
    let row_remote = SocketAddr::from((
        Ipv4Addr::from(row.dwRemoteAddr.to_ne_bytes()), port(row.dwRemotePort),
    ));
    (row.dwState == MIB_TCP_STATE_ESTAB as u32
        && row.dwOwningPid != 0
        && row_local == peer
        && row_remote == local)
        .then_some(row.dwOwningPid)
}

fn ipv6_row_pid(row: MIB_TCP6ROW_OWNER_PID, peer: SocketAddr, local: SocketAddr) -> Option<u32> {
    // The IP Helper API documents ports AND IPv6 scope IDs in network order.
    let row_local = SocketAddr::V6(SocketAddrV6::new(
        Ipv6Addr::from(row.ucLocalAddr), port(row.dwLocalPort),
        0, u32::from_be(row.dwLocalScopeId),
    ));
    let row_remote = SocketAddr::V6(SocketAddrV6::new(
        Ipv6Addr::from(row.ucRemoteAddr), port(row.dwRemotePort),
        0, u32::from_be(row.dwRemoteScopeId),
    ));
    (row.dwState == MIB_TCP_STATE_ESTAB as u32
        && row.dwOwningPid != 0
        && row_local == peer
        && row_remote == local)
        .then_some(row.dwOwningPid)
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0); }
    }
}

fn process_info(pid: u32, handle: &ProcessHandle) -> Option<ProcessInfo> {
    let mut path = vec![0_u16; 32768];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut length) } == 0
        || length == 0
        || length as usize > path.len()
    {
        return None;
    }
    let path = OsString::from_wide(&path[..length as usize])
        .to_string_lossy().into_owned();
    let name = Path::new(&path).file_name()?.to_string_lossy().into_owned();
    Some(ProcessInfo { pid, name, path: Some(path) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{IpAddr, TcpListener, TcpStream};
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    fn ipv4_row(peer: SocketAddr, local: SocketAddr, pid: u32) -> MIB_TCPROW_OWNER_PID {
        let (SocketAddr::V4(peer), SocketAddr::V4(local)) = (peer, local) else {
            panic!("IPv4 fixture")
        };
        MIB_TCPROW_OWNER_PID {
            dwState: MIB_TCP_STATE_ESTAB as u32,
            dwLocalAddr: u32::from_ne_bytes(peer.ip().octets()),
            dwLocalPort: u32::from(peer.port().to_be()),
            dwRemoteAddr: u32::from_ne_bytes(local.ip().octets()),
            dwRemotePort: u32::from(local.port().to_be()),
            dwOwningPid: pid,
        }
    }

    #[test]
    fn ipv4_matches_the_full_client_tuple_and_rejects_listener_direction() {
        let peer = "127.0.0.2:53123".parse().unwrap();
        let local = "127.0.0.1:8080".parse().unwrap();
        let row = ipv4_row(peer, local, 1234);
        assert_eq!(ipv4_row_pid(row, peer, local), Some(1234));
        assert_eq!(ipv4_row_pid(row, local, peer), None);
        assert_eq!(ipv4_row_pid(row, "127.0.0.3:53123".parse().unwrap(), local), None);
        assert_eq!(ipv4_row_pid(row, peer, "127.0.0.1:8081".parse().unwrap()), None);
        assert_eq!(ipv4_row_pid(row, peer, "127.0.0.3:8080".parse().unwrap()), None);
        assert_eq!(ipv4_row_pid(MIB_TCPROW_OWNER_PID { dwState: 2, ..row }, peer, local), None);
        assert_eq!(ipv4_row_pid(MIB_TCPROW_OWNER_PID { dwOwningPid: 0, ..row }, peer, local), None);
    }

    #[test]
    fn ipv6_matches_scoped_tuple_and_rejects_same_ports_on_other_addresses() {
        let peer = SocketAddrV6::new("fe80::1234".parse().unwrap(), 53123, 0, 7);
        let local = SocketAddrV6::new("fe80::5678".parse().unwrap(), 8080, 0, 7);
        let row = MIB_TCP6ROW_OWNER_PID {
            ucLocalAddr: peer.ip().octets(),
            dwLocalScopeId: peer.scope_id().to_be(),
            dwLocalPort: u32::from(peer.port().to_be()),
            ucRemoteAddr: local.ip().octets(),
            dwRemoteScopeId: local.scope_id().to_be(),
            dwRemotePort: u32::from(local.port().to_be()),
            dwState: MIB_TCP_STATE_ESTAB as u32,
            dwOwningPid: 1234,
        };
        assert_eq!(ipv6_row_pid(row, peer.into(), local.into()), Some(1234));
        assert_eq!(ipv6_row_pid(row, local.into(), peer.into()), None);
        let mut other_scope = peer;
        other_scope.set_scope_id(8);
        assert_eq!(ipv6_row_pid(row, other_scope.into(), local.into()), None);
        let mut other_ip = peer;
        other_ip.set_ip("fe80::abcd".parse().unwrap());
        assert_eq!(ipv6_row_pid(row, other_ip.into(), local.into()), None);
    }

    #[test]
    fn malformed_or_ambiguous_table_is_not_attributed() {
        let peer = "127.0.0.2:53123".parse().unwrap();
        let local = "127.0.0.1:8080".parse().unwrap();
        let mut bytes = vec![1_u8]; // Deliberately unaligned payload starts below.
        bytes.extend_from_slice(&1_u32.to_ne_bytes());
        let row = ipv4_row(peer, local, 1234);
        let row_bytes = unsafe {
            std::slice::from_raw_parts(
                (&row as *const MIB_TCPROW_OWNER_PID).cast::<u8>(),
                size_of::<MIB_TCPROW_OWNER_PID>(),
            )
        };
        bytes.extend_from_slice(row_bytes);
        let get = |data: &[u8]| {
            table_pid::<MIB_TCPROW_OWNER_PID>(data, 4, |row| ipv4_row_pid(row, peer, local))
        };
        assert_eq!(get(&bytes[1..]), Some(1234));
        assert_eq!(get(&bytes[1..bytes.len() - 1]), None);
        bytes[1..5].copy_from_slice(&2_u32.to_ne_bytes());
        let row = ipv4_row(peer, local, 9876);
        bytes.extend_from_slice(unsafe {
            std::slice::from_raw_parts(
                (&row as *const MIB_TCPROW_OWNER_PID).cast::<u8>(),
                size_of::<MIB_TCPROW_OWNER_PID>(),
            )
        });
        assert_eq!(get(&bytes[1..]), None);
    }

    #[test]
    fn child_client_fixture() {
        let Ok(address) = std::env::var("HTTP_CAPTURE_PROCESS_TEST_PEER") else {
            return;
        };
        let mut client = TcpStream::connect(address).expect("connect process fixture");
        client.write_all(b"ready").unwrap();
        client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut finish = [0_u8];
        client.read_exact(&mut finish).unwrap();
    }

    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    async fn assert_live_child_process(ip: IpAddr) {
        let listener = TcpListener::bind((ip, 0)).expect("bind process fixture");
        listener.set_nonblocking(true).unwrap();
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact", "traffic_process::platform::tests::child_client_fixture", "--nocapture",
            ])
            .env("HTTP_CAPTURE_PROCESS_TEST_PEER", listener.local_addr().unwrap().to_string())
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().expect("spawn process fixture");
        let child = ChildGuard(child);
        let child_pid = child.0.id();
        assert_ne!(child_pid, std::process::id());
        let (mut server, peer) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await.expect("child connects").unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut ready = [0_u8; 5];
        tokio::time::timeout(Duration::from_secs(10), server.read_exact(&mut ready))
            .await.unwrap().unwrap();
        assert_eq!(&ready, b"ready");
        let process = super::super::lookup(peer, server.local_addr().unwrap())
            .await.expect("identify real local client");
        assert_eq!(process.pid, child_pid, "must identify child client, not this proxy process");
        let executable = std::env::current_exe().unwrap();
        assert_eq!(process.name, executable.file_name().unwrap().to_string_lossy());
        assert_eq!(process.path.as_deref().map(Path::new), Some(executable.as_path()));
        server.write_all(&[1]).await.unwrap();
    }

    #[tokio::test]
    async fn live_ipv4_connection_identifies_the_distinct_client_process() {
        assert_live_child_process(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    }

    #[tokio::test]
    async fn live_ipv6_connection_identifies_the_distinct_client_process() {
        assert_live_child_process(IpAddr::V6(Ipv6Addr::LOCALHOST)).await;
    }
}
