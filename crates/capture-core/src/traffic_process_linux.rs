use crate::model::ProcessInfo;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::time::Instant;

// These limits also bound work if procfs contains more entries than the lookup
// deadline allows us to inspect. A truncated TCP table is never accepted.
const MAX_PROC_ENTRIES: usize = 16_384;
const MAX_FDS_PER_PROCESS: usize = 8_192;
const MAX_FDS_TOTAL: usize = 65_536;
const MAX_TCP_ROWS: usize = 65_536;
const MAX_TCP_BYTES: usize = 16 * 1024 * 1024;
const MAX_TCP_LINE_BYTES: u64 = 1_024;

/// The caller verifies that `peer.ip()` belongs to this host and runs this
/// blocking, best-effort lookup with a bounded deadline.
pub(super) fn lookup(
    peer: SocketAddr,
    local: SocketAddr,
    deadline: Instant,
) -> Option<ProcessInfo> {
    // proc/net/tcp6 does not include IPv6 zone IDs. Matching a scoped socket
    // by its address alone could attribute another interface's connection.
    if [peer, local].iter().any(|address| matches!(address, SocketAddr::V6(address) if address.scope_id() != 0)) {
        return None;
    }
    lookup_in(Path::new("/proc"), peer, local, deadline)
}

fn lookup_in(
    proc_root: &Path,
    peer: SocketAddr,
    local: SocketAddr,
    deadline: Instant,
) -> Option<ProcessInfo> {
    let inode = socket_inode(proc_root, peer, local, deadline)?;
    let socket_link = format!("socket:[{inode}]");
    let socket_link = Path::new(&socket_link);
    let mut inspected_fds = 0;

    'processes: for entry in fs::read_dir(proc_root).ok()?.take(MAX_PROC_ENTRIES) {
        if Instant::now() >= deadline {
            return None;
        }
        let Ok(entry) = entry else { continue };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .filter(|pid| *pid != 0)
        else {
            continue;
        };
        let process_dir = entry.path();
        let Ok(fds) = fs::read_dir(process_dir.join("fd")) else {
            // hidepid, permission restrictions and exited processes are normal.
            continue;
        };
        for fd in fds.take(MAX_FDS_PER_PROCESS) {
            if Instant::now() >= deadline || inspected_fds >= MAX_FDS_TOTAL {
                return None;
            }
            inspected_fds += 1;
            let Ok(fd) = fd else { continue };
            let fd_path = fd.path();
            if fs::read_link(&fd_path).ok().as_deref() != Some(socket_link) {
                continue;
            }

            let Some(start_time) = process_start_time(&process_dir, pid) else {
                continue 'processes;
            };
            let Some(comm) = read_small_file(&process_dir.join("comm"), 4_096) else {
                continue 'processes;
            };
            let name = String::from_utf8_lossy(&comm);
            let name = name.strip_suffix('\n').unwrap_or(&name);
            if name.is_empty() {
                continue 'processes;
            }
            let process = ProcessInfo {
                pid,
                name: name.to_owned(),
                path: fs::read_link(process_dir.join("exe"))
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned()),
            };

            // The socket may close while /proc is being scanned. Re-read its
            // complete tuple and inode, the actual FD link, and the process's
            // start time rather than trusting a stale inode or recycled PID.
            if socket_inode(proc_root, peer, local, deadline)? != inode {
                return None;
            }
            if fs::read_link(&fd_path).ok().as_deref() != Some(socket_link)
                || process_start_time(&process_dir, pid) != Some(start_time)
                || Instant::now() >= deadline
            {
                continue 'processes;
            }
            return Some(process);
        }
    }
    None
}

fn socket_inode(
    proc_root: &Path,
    peer: SocketAddr,
    local: SocketAddr,
    deadline: Instant,
) -> Option<u64> {
    let mut matched = None;
    // An IPv4 client may use either an AF_INET socket or a mapped AF_INET6
    // socket. Search both tables, while rejecting conflicting exact matches.
    for (name, ipv6) in [("tcp", false), ("tcp6", true)] {
        if Instant::now() >= deadline {
            return None;
        }
        let file = match File::open(proc_root.join("net").join(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        scan_tcp_table(BufReader::new(file), ipv6, peer, local, deadline, &mut matched)?;
    }
    matched
}

fn scan_tcp_table<R: BufRead>(
    mut reader: R,
    ipv6: bool,
    peer: SocketAddr,
    local: SocketAddr,
    deadline: Instant,
    matched: &mut Option<u64>,
) -> Option<()> {
    let mut line = String::new();
    let mut total_bytes = 0;
    for _ in 0..MAX_TCP_ROWS {
        if Instant::now() >= deadline {
            return None;
        }
        line.clear();
        // Per-line and total byte bounds avoid unbounded read_line allocation.
        let count = reader
            .by_ref()
            .take(MAX_TCP_LINE_BYTES + 1)
            .read_line(&mut line)
            .ok()?;
        if count == 0 {
            return Some(());
        }
        total_bytes += count;
        if count > MAX_TCP_LINE_BYTES as usize || total_bytes > MAX_TCP_BYTES {
            return None;
        }
        let Some(row) = parse_tcp_row(&line, ipv6) else {
            continue;
        };
        // Accepted-stream peer is the client socket's local address, and the
        // accepted-stream local address is that socket's remote address.
        if !same_endpoint(row.local, peer) || !same_endpoint(row.remote, local) {
            continue;
        }
        if matched.is_some_and(|inode| inode != row.inode) {
            return None;
        }
        *matched = Some(row.inode);
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
struct TcpRow {
    local: SocketAddr,
    remote: SocketAddr,
    inode: u64,
}

fn parse_tcp_row(line: &str, ipv6: bool) -> Option<TcpRow> {
    // Field order is documented by the Linux kernel:
    // https://www.kernel.org/doc/html/latest/networking/proc_net_tcp.html
    let mut fields = line.split_whitespace();
    fields.next()?;
    let local = parse_endpoint(fields.next()?, ipv6)?;
    let remote = parse_endpoint(fields.next()?, ipv6)?;
    if fields.next()? != "01" {
        return None;
    }
    let inode = fields.nth(5)?.parse::<u64>().ok()?;
    if inode == 0 {
        return None;
    }
    Some(TcpRow { local, remote, inode })
}

fn parse_endpoint(value: &str, ipv6: bool) -> Option<SocketAddr> {
    let (address, port) = value.split_once(':')?;
    if port.len() != 4 {
        return None;
    }
    let port = u16::from_str_radix(port, 16).ok()?;
    let ip = if ipv6 {
        if address.len() != 32 {
            return None;
        }
        let mut octets = [0; 16];
        // tcp6_seq_show prints each native-endian s6_addr32 word as hex,
        // rather than printing the IPv6 address as one big-endian integer.
        for (word, destination) in address.as_bytes().chunks_exact(8).zip(octets.chunks_exact_mut(4)) {
            let word = u32::from_str_radix(std::str::from_utf8(word).ok()?, 16).ok()?;
            destination.copy_from_slice(&word.to_ne_bytes());
        }
        IpAddr::V6(Ipv6Addr::from(octets))
    } else {
        if address.len() != 8 {
            return None;
        }
        let address = u32::from_str_radix(address, 16).ok()?;
        IpAddr::V4(Ipv4Addr::from(address.to_ne_bytes()))
    };
    Some(SocketAddr::new(ip, port))
}

fn same_endpoint(a: SocketAddr, b: SocketAddr) -> bool {
    a.port() == b.port() && canonical_ip(a.ip()) == canonical_ip(b.ip())
}

fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(ip)),
        ip => ip,
    }
}

fn read_small_file(path: &Path, limit: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path).ok()?.take(limit as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= limit).then_some(bytes)
}

fn process_start_time(process_dir: &Path, pid: u32) -> Option<u64> {
    let stat = read_small_file(&process_dir.join("stat"), 8_192)?;
    parse_start_time(std::str::from_utf8(&stat).ok()?, pid)
}

fn parse_start_time(stat: &str, pid: u32) -> Option<u64> {
    let (prefix, _) = stat.split_once('(')?;
    if prefix.trim().parse::<u32>().ok()? != pid {
        return None;
    }
    // comm (field 2) can contain whitespace or parentheses. Fields following
    // the final ')' start at state (field 3); starttime is field 22.
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::Duration;

    #[cfg(target_endian = "little")]
    const LOOPBACK_V4: &str = "0100007F";
    #[cfg(target_endian = "big")]
    const LOOPBACK_V4: &str = "7F000001";
    #[cfg(target_endian = "little")]
    const LOOPBACK_V6: &str = "00000000000000000000000001000000";
    #[cfg(target_endian = "big")]
    const LOOPBACK_V6: &str = "00000000000000000000000000000001";
    #[cfg(target_endian = "little")]
    const MAPPED_LOOPBACK: &str = "0000000000000000FFFF00000100007F";
    #[cfg(target_endian = "big")]
    const MAPPED_LOOPBACK: &str = "00000000000000000000FFFF7F000001";

    fn row(local: &str, remote: &str, state: &str, inode: u64) -> String {
        format!(" 0: {local} {remote} {state} 00000000:00000000 00:00000000 00000000 1000 0 {inode} 1 0\n")
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(2)
    }

    #[test]
    fn parses_ipv4_and_ipv6_native_word_encoding() {
        assert_eq!(
            parse_endpoint(&format!("{LOOPBACK_V4}:C350"), false),
            Some("127.0.0.1:50000".parse().unwrap())
        );
        assert_eq!(
            parse_endpoint(&format!("{LOOPBACK_V6}:1F90"), true),
            Some("[::1]:8080".parse().unwrap())
        );
        assert_eq!(
            parse_endpoint(&format!("{MAPPED_LOOPBACK}:C350"), true),
            Some("[::ffff:127.0.0.1]:50000".parse().unwrap())
        );
    }

    #[test]
    fn matches_only_complete_client_direction_tuple() {
        let peer = "127.0.0.1:50000".parse().unwrap();
        let local = "127.0.0.1:8080".parse().unwrap();
        let client = format!("{LOOPBACK_V4}:C350");
        let proxy = format!("{LOOPBACK_V4}:1F90");
        let other_proxy = format!("{LOOPBACK_V4}:1F91");
        let data = [
            row(&proxy, &client, "01", 11),
            row(&client, &other_proxy, "01", 22),
            row(&client, &proxy, "0A", 33),
            row(&client, &proxy, "06", 44),
            row(&client, &proxy, "01", 55),
        ].concat();
        let mut matched = None;
        assert_eq!(scan_tcp_table(Cursor::new(data), false, peer, local, deadline(), &mut matched), Some(()));
        assert_eq!(matched, Some(55));
    }

    #[test]
    fn matches_mapped_ipv4_and_rejects_ambiguous_inodes() {
        let peer = "127.0.0.1:50000".parse().unwrap();
        let local = "127.0.0.1:8080".parse().unwrap();
        let data = row(&format!("{MAPPED_LOOPBACK}:C350"), &format!("{MAPPED_LOOPBACK}:1F90"), "01", 55);
        let mut matched = None;
        assert_eq!(scan_tcp_table(Cursor::new(&data), true, peer, local, deadline(), &mut matched), Some(()));
        assert_eq!(matched, Some(55));
        matched = Some(66);
        assert_eq!(scan_tcp_table(Cursor::new(data), true, peer, local, deadline(), &mut matched), None);
    }

    #[test]
    fn malformed_rows_and_zero_inodes_are_ignored() {
        for value in ["", "header local_address remote_address", "0: no:port no:port 01", "0: éééééééééééééééé:0001 0:0000 01"] {
            assert_eq!(parse_tcp_row(value, false), None);
            assert_eq!(parse_tcp_row(value, true), None);
        }
        let endpoint = format!("{LOOPBACK_V4}:C350");
        assert_eq!(parse_tcp_row(&row(&endpoint, &endpoint, "01", 0), false), None);
    }

    #[test]
    fn rejects_expired_and_oversized_table_scans() {
        let address = "127.0.0.1:8080".parse().unwrap();
        let mut matched = None;
        assert_eq!(scan_tcp_table(Cursor::new(""), false, address, address, Instant::now(), &mut matched), None);
        assert_eq!(scan_tcp_table(Cursor::new("x".repeat(MAX_TCP_LINE_BYTES as usize + 1)), false, address, address, deadline(), &mut matched), None);
    }

    #[test]
    fn scoped_ipv6_cannot_be_attributed_without_procfs_zone_information() {
        let peer = SocketAddr::V6(std::net::SocketAddrV6::new("fe80::1".parse().unwrap(), 50000, 0, 7));
        let local = SocketAddr::V6(std::net::SocketAddrV6::new("fe80::2".parse().unwrap(), 8080, 0, 7));
        assert!(lookup(peer, local, deadline()).is_none());
    }

    #[test]
    fn parses_process_start_time_after_parenthesized_comm() {
        let fields = (3..=21).map(|field| if field == 3 { "S".to_owned() } else { field.to_string() }).collect::<Vec<_>>().join(" ");
        let stat = format!("42 (a process (with) spaces) {fields} 123456 23\n");
        assert_eq!(parse_start_time(&stat, 42), Some(123456));
        assert_eq!(parse_start_time(&stat, 43), None);
        assert_eq!(parse_start_time("42 (missing fields) S", 42), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn resolves_current_process_for_a_live_loopback_client_socket() {
        use std::net::{TcpListener, TcpStream};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (accepted, peer) = listener.accept().unwrap();
        let process = lookup(peer, accepted.local_addr().unwrap(), deadline()).expect("current process should own its live client socket");
        assert_eq!(process.pid, std::process::id());
        assert!(!process.name.is_empty());
        drop((accepted, client));
    }
}
