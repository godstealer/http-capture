//! Best-effort process attribution for locally originating TCP connections.
//!
//! The accepted connection is server-facing: the OS client row must have its
//! local endpoint equal to `peer`, and its remote endpoint equal to `local`.
//! Never cache the result by PID or port: either can be reused immediately.

use std::net::{IpAddr, SocketAddr};
#[cfg(any(windows, target_os = "linux"))]
use std::time::Instant;
use std::time::Duration;
use tokio::sync::Semaphore;

use crate::model::ProcessInfo;

#[cfg(target_os = "linux")]
#[path = "traffic_process_linux.rs"]
mod platform;
#[cfg(windows)]
#[path = "traffic_process_windows.rs"]
mod platform;
#[cfg(all(test, windows))]
#[path = "traffic_process_linux.rs"]
#[allow(dead_code)]
mod linux_parser_tests;

pub(crate) const SUPPORTED: bool = cfg!(any(windows, target_os = "linux"));
#[cfg(any(windows, target_os = "linux"))]
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(any(windows, target_os = "linux"))]
const BLOCKING_BUDGET: Duration = Duration::from_millis(750);
#[cfg(any(windows, target_os = "linux"))]
static LOOKUPS: Semaphore = Semaphore::const_new(4);

fn normalized_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(ip)),
        ip => ip,
    }
}

#[cfg(any(windows, target_os = "linux", test))]
fn normalized_address(address: SocketAddr) -> SocketAddr {
    match address {
        SocketAddr::V6(mut address) => {
            // Flow labels are not part of a TCP endpoint identity.
            address.set_flowinfo(0);
            address
                .ip()
                .to_ipv4_mapped()
                .map(|ip| SocketAddr::from((ip, address.port())))
                .unwrap_or(SocketAddr::V6(address))
        }
        address => address,
    }
}

/// This is intentionally a fresh adapter snapshot, not a cached address list.
pub(crate) fn is_local(ip: IpAddr) -> bool {
    let ip = normalized_ip(ip);
    if ip.is_loopback() {
        return true;
    }
    if ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    if_addrs::get_if_addrs()
        .map(|interfaces| interfaces.iter().any(|interface| normalized_ip(interface.ip()) == ip))
        .unwrap_or(false)
}

/// A timeout does not cancel a blocking syscall. Keep the permit in the worker
/// so timed-out or cancelled callers cannot create an unbounded worker queue.
pub(crate) async fn lookup(peer: SocketAddr, local: SocketAddr) -> Option<ProcessInfo> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        let peer = normalized_address(peer);
        let local = normalized_address(local);
        if peer.port() == 0 || local.port() == 0 || peer.is_ipv4() != local.is_ipv4() {
            return None;
        }
        run_bounded(&LOOKUPS, LOOKUP_TIMEOUT, move || {
            // A busy queue must not consume the native lookup's own budget.
            // run_bounded still limits the caller's total wait, including the
            // queue, and the worker retains its permit after caller timeout.
            let deadline = Instant::now() + BLOCKING_BUDGET;
            if !is_local(peer.ip()) || Instant::now() >= deadline {
                return None;
            }
            platform::lookup(peer, local, deadline)
        }).await
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (peer, local);
        None
    }
}

pub(crate) async fn run_bounded<T: Send + 'static>(
    semaphore: &'static Semaphore,
    timeout: Duration,
    work: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    // Browser page loads commonly open more than four connections together.
    // Wait for an existing lookup rather than permanently losing attribution
    // for every connection beyond the concurrency budget. Waiting is bounded
    // and cancellable; no blocking worker is spawned before a permit is held.
    tokio::time::timeout(timeout, async {
        let permit = semaphore.acquire().await.ok()?;
        let worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        });
        worker.await.ok()?
    }).await.ok()?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_address_check_handles_loopback_mapped_and_remote_addresses() {
        assert!(is_local("127.0.0.1".parse().unwrap()));
        assert!(is_local("::1".parse().unwrap()));
        assert!(is_local("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!is_local("0.0.0.0".parse().unwrap()));
        assert!(!is_local("::".parse().unwrap()));
        assert!(!is_local("224.0.0.1".parse().unwrap()));
        assert!(!is_local("ff02::1".parse().unwrap()));
        assert!(!is_local("192.0.2.41".parse().unwrap()));
    }

    #[test]
    fn normalization_preserves_ipv6_scopes_and_normalizes_only_mapped_ipv4() {
        assert_eq!(
            normalized_address("[::ffff:127.0.0.1]:43123".parse().unwrap()),
            "127.0.0.1:43123".parse::<SocketAddr>().unwrap()
        );
        let scoped = SocketAddr::V6(std::net::SocketAddrV6::new(
            "fe80::1".parse().unwrap(), 43123, 0, 7,
        ));
        assert_eq!(normalized_address(scoped), scoped);
        let mut with_flow = scoped;
        if let SocketAddr::V6(address) = &mut with_flow { address.set_flowinfo(42); }
        assert_eq!(normalized_address(with_flow), scoped);
        assert!(normalized_address("[::1]:43123".parse().unwrap()).is_ipv6());
    }

    #[tokio::test]
    async fn invalid_and_nonlocal_peers_are_not_attributed() {
        assert!(lookup("192.0.2.41:12345".parse().unwrap(), "127.0.0.1:8080".parse().unwrap()).await.is_none());
        assert!(lookup("127.0.0.1:0".parse().unwrap(), "127.0.0.1:8080".parse().unwrap()).await.is_none());
        assert!(lookup("127.0.0.1:12345".parse().unwrap(), "[::1]:8080".parse().unwrap()).await.is_none());
    }

    #[cfg(any(windows, target_os = "linux"))]
    async fn assert_worker_keeps_permit(cancel_caller: bool) {
        let semaphore: &'static Semaphore = Box::leak(Box::new(Semaphore::new(1)));
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, finish) = std::sync::mpsc::channel();
        let caller = tokio::spawn(run_bounded(semaphore, Duration::from_millis(30), move || {
            let _ = started.send(());
            finish.recv_timeout(Duration::from_secs(2)).unwrap();
            None::<ProcessInfo>
        }));
        ready.await.unwrap();
        if cancel_caller {
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
        } else {
            assert!(caller.await.unwrap().is_none());
        }
        assert_eq!(semaphore.available_permits(), 0);
        assert!(run_bounded(semaphore, Duration::from_millis(30), || -> Option<ProcessInfo> { panic!("must not start another worker before the slot is released") }).await.is_none());
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while semaphore.available_permits() == 0 { tokio::task::yield_now().await; }
        }).await.unwrap();
        assert_eq!(semaphore.available_permits(), 1);
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[tokio::test]
    async fn timeout_keeps_concurrency_slot_until_blocking_work_finishes() {
        assert_worker_keeps_permit(false).await;
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[tokio::test]
    async fn cancellation_keeps_concurrency_slot_until_blocking_work_finishes() {
        assert_worker_keeps_permit(true).await;
    }

    #[tokio::test]
    async fn burst_waits_for_lookup_slots_instead_of_dropping_connections() {
        let semaphore: &'static Semaphore = Box::leak(Box::new(Semaphore::new(4)));
        let held = semaphore.acquire_many(4).await.unwrap();
        let mut calls = tokio::task::JoinSet::new();
        for index in 0..16 {
            calls.spawn(run_bounded(semaphore, Duration::from_secs(2), move || Some(index)));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(calls.try_join_next().is_none(), "busy lookup slots must queue callers");
        drop(held);
        let mut values = Vec::new();
        while let Some(value) = calls.join_next().await { values.push(value.unwrap().unwrap()); }
        values.sort();
        assert_eq!(values, (0..16).collect::<Vec<_>>());
        assert_eq!(semaphore.available_permits(), 4);
    }

    #[tokio::test]
    async fn timed_out_or_cancelled_waiters_never_start_a_late_lookup() {
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        let semaphore: &'static Semaphore = Box::leak(Box::new(Semaphore::new(1)));
        let held = semaphore.acquire().await.unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let seen = started.clone();
        assert!(run_bounded(semaphore, Duration::from_millis(10), move || {
            seen.store(true, Ordering::SeqCst); Some(())
        }).await.is_none());
        let seen = started.clone();
        let call = tokio::spawn(run_bounded(semaphore, Duration::from_secs(2), move || {
            seen.store(true, Ordering::SeqCst); Some(())
        }));
        tokio::task::yield_now().await;
        call.abort(); assert!(call.await.unwrap_err().is_cancelled());
        drop(held); tokio::task::yield_now().await;
        assert!(!started.load(Ordering::SeqCst));
        assert_eq!(semaphore.available_permits(), 1);
        assert_eq!(run_bounded(semaphore, Duration::from_secs(1), || Some(7)).await, Some(7));
    }
}
