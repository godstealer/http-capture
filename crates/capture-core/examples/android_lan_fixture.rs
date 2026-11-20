//! Isolated Android VPN/LAN E2E host. Never starts the desktop UI, proxy,
//! system routing, or certificate installer. Its only control channel is stdin.
#[path = "android_lan_fixture/protocol.rs"]
mod protocol;

use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use capture_core::{
    body_capture::{BodyCaptureState, VARIANTS},
    mobile_gateway::MobileGateway,
    model::{Flow, Header},
    Engine,
};
use protocol::Traffic;
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UdpSocket},
    sync::mpsc,
    task::JoinSet,
    time::{timeout, Duration, Instant},
};

const LIFETIME: Duration = Duration::from_secs(420);
const VERIFY_WAIT: Duration = Duration::from_secs(30);
#[derive(Default)]
struct Facts {
    http_requests: usize,
    http_replies: usize,
    udp_received: [bool; 2],
    udp_echoed: usize,
    udp_peer: Option<SocketAddr>,
    dns_received: usize,
    dns_replied: usize,
    error: Option<&'static str>,
}
type SharedFacts = Arc<Mutex<Facts>>;
fn fact_error(facts: &SharedFacts, reason: &'static str) {
    facts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .error
        .get_or_insert(reason);
}

struct Fixture {
    directory: PathBuf,
    gateway: Option<MobileGateway>,
    engine: Arc<Engine>,
    tasks: JoinSet<()>,
    facts: SharedFacts,
    traffic: Traffic,
    udp_port: u16,
    dns_wire_port: u16,
    verified: bool,
    disconnected: bool,
    stopped: bool,
    ready_created: bool,
    verified_device_name: Option<String>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(gateway) = self.gateway.take() {
            gateway.stop();
        }
        self.tasks.abort_all();
        if self.ready_created {
            let _ = fs::remove_file(self.directory.join("ready.json"));
            self.ready_created = false;
        }
    }
}
impl Fixture {
    async fn start(host: Ipv4Addr, directory: PathBuf) -> Result<Self> {
        validate_directory(&directory)?;
        ensure!(
            host.is_private() && !host.is_loopback(),
            "Fixture needs a private LAN IPv4 address"
        );
        ensure!(
            if_addrs::get_if_addrs()?
                .iter()
                .any(|interface| interface.ip() == std::net::IpAddr::V4(host)),
            "Fixture host is not assigned to this runner"
        );
        // This is the only Engine path. The caller must give a fresh empty
        // temporary directory; no real capture store or credentials are opened.
        let engine = Engine::open(&directory.join("engine"))?;
        let traffic = Traffic::new(uuid::Uuid::new_v4().simple().to_string(), host, 1024)?;
        let gateway = MobileGateway::start(host, 0, engine.clone()).await?;
        let mut fixture = Self {
            directory,
            gateway: Some(gateway),
            engine,
            tasks: JoinSet::new(),
            facts: Default::default(),
            traffic,
            udp_port: 0,
            dns_wire_port: 0,
            verified: false,
            disconnected: false,
            stopped: false,
            ready_created: false,
            verified_device_name: None,
        };
        let http = TcpListener::bind((host, 0)).await?;
        let http_port = http.local_addr()?.port();
        let gateway_port = fixture
            .gateway
            .as_ref()
            .expect("live gateway")
            .address()
            .port();
        let udp = bind_udp_origin(host, &[gateway_port, http_port]).await?;
        fixture.udp_port = udp.local_addr()?.port();
        let dns = bind_udp_origin(host, &[gateway_port, http_port, fixture.udp_port]).await?;
        fixture.dns_wire_port = dns.local_addr()?.port();
        ensure!(
            [http_port, fixture.udp_port, fixture.dns_wire_port]
                .iter()
                .all(|port| *port >= 1024),
            "Origin source requires high ports"
        );
        fixture.traffic = Traffic::new(fixture.traffic.run_id.clone(), host, http_port)?;
        let traffic = fixture.traffic.clone();
        let facts = fixture.facts.clone();
        fixture.tasks.spawn(async move {
            if http_origin(http, traffic, facts.clone()).await.is_err() {
                fact_error(&facts, "HTTP origin failed");
            }
        });
        let traffic = fixture.traffic.clone();
        let facts = fixture.facts.clone();
        fixture.tasks.spawn(async move {
            if udp_origin(udp, traffic, facts.clone()).await.is_err() {
                fact_error(&facts, "UDP origin failed");
            }
        });
        let traffic = fixture.traffic.clone();
        let facts = fixture.facts.clone();
        fixture.tasks.spawn(async move {
            if dns_origin(dns, traffic, facts.clone()).await.is_err() {
                fact_error(&facts, "DNS-wire origin failed");
            }
        });
        let gateway = fixture.gateway.as_ref().expect("live gateway");
        let pairing = gateway.open_pairing()?;
        let ready = fixture.traffic.ready(
            pairing.to_uri(),
            gateway.address().port(),
            fixture.udp_port,
            fixture.dns_wire_port,
        );
        write_private_json(
            &fixture.directory.join("ready.json"),
            &serde_json::to_value(ready)?,
        )?;
        fixture.ready_created = true;
        event("ready");
        Ok(fixture)
    }
    async fn verify(&mut self) -> Result<()> {
        ensure!(
            !self.verified && !self.disconnected,
            "Unexpected verify phase"
        );
        let deadline = Instant::now() + VERIFY_WAIT;
        loop {
            ensure!(
                self.facts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .error
                    .is_none(),
                "Origin validation failed"
            );
            let gateway = self.gateway.as_ref().expect("live gateway");
            let device = gateway.device_name();
            let flows = self.engine.store.list()?;
            let origins_ok = {
                let facts = self.facts.lock().unwrap_or_else(|e| e.into_inner());
                facts.http_requests == 2
                    && facts.http_replies == 2
                    && facts.udp_received == [true, true]
                    && facts.udp_echoed == 2
                    && facts.udp_peer.is_some()
                    && facts.dns_received == 1
                    && facts.dns_replied == 1
            };
            if gateway.is_paired()
                && origins_ok
                && device
                    .as_deref()
                    .is_some_and(|name| !name.trim().is_empty())
                && validate_flows(
                    &flows,
                    &self.traffic,
                    self.udp_port,
                    self.dns_wire_port,
                    device.as_deref().unwrap(),
                )
                .is_ok()
            {
                write_private_json(
                    &self.directory.join("verification.json"),
                    &json!({"schemaVersion":1,"runId":self.traffic.run_id,"status":"ok","httpExchanges":2,"udpDatagrams":2,"dnsWireDatagrams":1,"udpSourcePorts":1,
                    "bodyCapture":"notRecorded","deviceNameMatched":true,"originVerified":true,"httpFlows":2,"httpRequestsExact":true,"orderedRawHeaders":true,"androidLanOrigin":true,"deviceNameObserved":true,
                    "allFourBodyVariantsNotRecorded":true,"httpBodyBytesRetained":false,"udpReceived":2,"udpEchoRepliesSent":2,
                    "udpSourcePortReused":true,"dnsWireReceived":1,"dnsWireReplySent":1,
                    "dnsWireEvidenceOnly":true,"androidReceiveAssertionsRequired":true}),
                )?;
                self.verified_device_name = device;
                self.verified = true;
                event("verified");
                return Ok(());
            }
            ensure!(Instant::now() < deadline, "Verification deadline exceeded");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    fn disconnect(&mut self) -> Result<()> {
        ensure!(
            self.verified && !self.disconnected,
            "Unexpected disconnect phase"
        );
        let gateway = self.gateway.as_ref().expect("live gateway");
        gateway.disconnect();
        ensure!(
            !gateway.is_paired() && gateway.device_name().is_none(),
            "Gateway revocation incomplete"
        );
        write_private_json(
            &self.directory.join("disconnected.json"),
            &json!({"schemaVersion":1,"runId":self.traffic.run_id,"status":"disconnected","paired":false,"deviceNameCleared":true}),
        )?;
        self.disconnected = true;
        event("disconnected");
        Ok(())
    }
    async fn verify_stopped(&mut self) -> Result<()> {
        ensure!(
            self.disconnected && !self.stopped,
            "Unexpected stopped phase"
        );
        let deadline = Instant::now() + VERIFY_WAIT;
        loop {
            let gateway = self.gateway.as_ref().expect("live gateway");
            let flows = self.engine.store.list()?;
            let origins_unchanged = {
                let facts = self.facts.lock().unwrap_or_else(|e| e.into_inner());
                ensure!(facts.error.is_none(), "Origin changed after verification");
                facts.http_requests == 2
                    && facts.http_replies == 2
                    && facts.udp_received == [true, true]
                    && facts.udp_echoed == 2
                    && facts.udp_peer.is_some()
                    && facts.dns_received == 1
                    && facts.dns_replied == 1
            };
            let records_unchanged = self.verified_device_name.as_deref().is_some_and(|device| {
                validate_flows(
                    &flows,
                    &self.traffic,
                    self.udp_port,
                    self.dns_wire_port,
                    device,
                )
                .is_ok()
            });
            if !gateway.is_paired()
                && gateway.device_name().is_none()
                && origins_unchanged
                && records_unchanged
                && flows.iter().all(|flow| flow.state != "receiving")
            {
                write_private_json(
                    &self.directory.join("stopped.json"),
                    &json!({"schemaVersion":1,"runId":self.traffic.run_id,"status":"ok","paired":false,"deviceNameCleared":true,"pendingFlows":0,"originCountersUnchanged":true,"recordCountUnchanged":true}),
                )?;
                self.stopped = true;
                event("stopped");
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "Stop verification deadline exceeded"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn control(&mut self, mut input: mpsc::Receiver<Command>) -> Result<()> {
        while let Some(command) = input.recv().await {
            match command {
                Command::Verify => self.verify().await?,
                Command::Disconnect => self.disconnect()?,
                Command::VerifyStopped => self.verify_stopped().await?,
                Command::Quit => return Ok(()),
                Command::Invalid => anyhow::bail!("Invalid fixture control line"),
            }
        }
        Ok(())
    }
    async fn cleanup(&mut self) {
        if let Some(gateway) = self.gateway.take() {
            gateway.stop();
        }
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
        if self.ready_created {
            let _ = fs::remove_file(self.directory.join("ready.json"));
            self.ready_created = false;
        }
    }
}

fn validate_directory(directory: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(directory)
        .context("Fixture needs an existing empty private temporary directory")?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Fixture directory must not be a symlink"
    );
    ensure!(
        fs::read_dir(directory)?.next().is_none(),
        "Fixture refuses a nonempty state directory"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o777 == 0o700,
            "Fixture directory must have mode 0700"
        );
    }
    Ok(())
}
fn write_private_json(path: &Path, value: &Value) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let bytes = serde_json::to_vec(value)?;
    let mut file = options.open(path)?;
    let written = (|| -> Result<()> {
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(())
    })();
    drop(file);
    if written.is_err() {
        let _ = fs::remove_file(path);
    }
    written
}
fn event(name: &str) {
    println!("{{\"event\":\"{name}\"}}");
    let _ = std::io::stdout().flush();
}
#[derive(Clone, Copy)]
enum Command {
    Verify,
    Disconnect,
    VerifyStopped,
    Quit,
    Invalid,
}
fn command(bytes: &[u8]) -> Command {
    match bytes {
        b"verify" => Command::Verify,
        b"disconnect" => Command::Disconnect,
        b"verify-stopped" => Command::VerifyStopped,
        b"quit" => Command::Quit,
        _ => Command::Invalid,
    }
}
fn stdin_commands() -> mpsc::Receiver<Command> {
    let (sender, receiver) = mpsc::channel(4);
    // A detached std thread avoids Tokio's uncancellable blocking-stdin task
    // keeping the runtime alive after the seven-minute cleanup deadline.
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        let mut line = Vec::with_capacity(32);
        let mut byte = [0u8; 1];
        loop {
            match input.read(&mut byte) {
                Ok(0) => {
                    if !line.is_empty() {
                        let _ = sender.blocking_send(Command::Invalid);
                    }
                    break;
                }
                Ok(_) => {
                    if byte[0] == b'\n' {
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        let item = command(&line);
                        line.clear();
                        if sender.blocking_send(item).is_err() {
                            break;
                        }
                    } else if line.len() < 64 {
                        line.push(byte[0]);
                    } else {
                        let _ = sender.blocking_send(Command::Invalid);
                        break;
                    }
                }
                Err(_) => {
                    let _ = sender.blocking_send(Command::Invalid);
                    break;
                }
            }
        }
    });
    receiver
}

async fn run() -> Result<()> {
    let started = Instant::now();
    let mut arguments = std::env::args_os().skip(1);
    let host = arguments
        .next()
        .context("Missing host")?
        .to_str()
        .context("Invalid host")?
        .parse::<Ipv4Addr>()?;
    let directory = PathBuf::from(arguments.next().context("Missing empty state directory")?);
    ensure!(arguments.next().is_none(), "Unexpected fixture arguments");
    let mut fixture = Fixture::start(host, directory).await?;
    let input = stdin_commands();
    let result = timeout(
        LIFETIME.saturating_sub(started.elapsed()),
        fixture.control(input),
    )
    .await;
    fixture.cleanup().await;
    event("bye");
    result.context("Fixture lifetime exceeded")?
}
#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!("fixture-error");
        std::process::exit(1);
    }
}

async fn bind_udp_origin(host: Ipv4Addr, excluded: &[u16]) -> Result<UdpSocket> {
    // The production gateway rejects its own endpoint across protocols too.
    // Ephemeral TCP/UDP port-number collisions must not make this smoke flaky.
    for _ in 0..32 {
        let socket = UdpSocket::bind((host, 0)).await?;
        let port = socket.local_addr()?.port();
        if port >= 1024 && !excluded.contains(&port) {
            return Ok(socket);
        }
    }
    anyhow::bail!("Unable to reserve a distinct high UDP fixture port")
}
async fn http_origin(listener: TcpListener, traffic: Traffic, facts: SharedFacts) -> Result<()> {
    let (mut stream, peer) = listener.accept().await?;
    ensure!(
        peer.ip() == std::net::IpAddr::V4(traffic.host),
        "Unexpected HTTP source address"
    );
    for first in [true, false] {
        let expected = traffic.request(first);
        let mut received = vec![0; expected.len()];
        timeout(Duration::from_secs(20), stream.read_exact(&mut received)).await??;
        ensure!(received == expected, "HTTP origin request bytes differ");
        facts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .http_requests += 1;
        timeout(
            Duration::from_secs(20),
            stream.write_all(&traffic.response(first)),
        )
        .await??;
        facts.lock().unwrap_or_else(|e| e.into_inner()).http_replies += 1;
    }
    stream.shutdown().await?;
    drop(stream);
    // Keep the origin listening until cleanup so a revoked-session test cannot
    // succeed merely because the destination stopped accepting connections.
    let _unexpected = listener.accept().await?;
    anyhow::bail!("Unexpected additional HTTP connection")
}
async fn udp_origin(socket: UdpSocket, traffic: Traffic, facts: SharedFacts) -> Result<()> {
    let mut bytes = [0u8; 2048];
    loop {
        let (length, peer) = socket.recv_from(&mut bytes).await?;
        ensure!(
            peer.ip() == std::net::IpAddr::V4(traffic.host),
            "Unexpected UDP source address"
        );
        let first = bytes[..length] == traffic.udp(true);
        let second = bytes[..length] == traffic.udp(false);
        ensure!(first || second, "Unexpected UDP fixture bytes");
        let index = if first { 0 } else { 1 };
        {
            let mut facts = facts.lock().unwrap_or_else(|e| e.into_inner());
            ensure!(!facts.udp_received[index], "Duplicate UDP fixture datagram");
            ensure!(
                facts.udp_peer.is_none_or(|saved| saved == peer),
                "UDP source port was not reused"
            );
            facts.udp_peer = Some(peer);
            facts.udp_received[index] = true;
        }
        ensure!(
            socket.send_to(&bytes[..length], peer).await? == length,
            "UDP echo was truncated"
        );
        facts.lock().unwrap_or_else(|e| e.into_inner()).udp_echoed += 1;
    }
}
async fn dns_origin(socket: UdpSocket, traffic: Traffic, facts: SharedFacts) -> Result<()> {
    let mut bytes = [0u8; 2048];
    loop {
        let (length, peer) = socket.recv_from(&mut bytes).await?;
        ensure!(
            peer.ip() == std::net::IpAddr::V4(traffic.host),
            "Unexpected DNS-wire source address"
        );
        let reply = protocol::dns_reply(&bytes[..length], &traffic.run_id)?;
        {
            let mut facts = facts.lock().unwrap_or_else(|e| e.into_inner());
            ensure!(facts.dns_received == 0, "Duplicate DNS-wire query");
            facts.dns_received = 1;
        }
        ensure!(
            socket.send_to(&reply, peer).await? == reply.len(),
            "DNS-wire reply was truncated"
        );
        facts.lock().unwrap_or_else(|e| e.into_inner()).dns_replied = 1;
    }
}
fn headers_match(headers: &[Header], expected_raw: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(expected_raw) else {
        return false;
    };
    let expected: Vec<_> = text
        .split("\r\n")
        .skip(1)
        .filter(|line| !line.is_empty())
        .filter_map(|line| line.split_once(": "))
        .collect();
    headers.len() == expected.len()
        && headers.iter().zip(expected).all(|(header, (name, value))| {
            !header.disabled && header.name == name && header.value == value
        })
}
fn raw_matches(encoded: Option<&str>, expected: &[u8]) -> bool {
    encoded
        .and_then(|value| STANDARD.decode(value).ok())
        .is_some_and(|bytes| bytes == expected)
}
fn validate_flows(
    flows: &[Flow],
    traffic: &Traffic,
    udp_port: u16,
    dns_wire_port: u16,
    device: &str,
) -> Result<()> {
    ensure!(
        flows.len() == 4,
        "Expected exactly two HTTP and two UDP metadata records"
    );
    for flow in flows {
        ensure!(
            flow.state == "completed" && flow.error.is_none(),
            "Flow is not completed"
        );
        let origin = flow
            .traffic_origin
            .as_ref()
            .context("Missing Android LAN origin")?;
        ensure!(
            origin.entry == "android-lan"
                && origin.process_status == "remote"
                && origin.device_name.as_deref() == Some(device),
            "Wrong remote origin or device name"
        );
        ensure!(
            origin.process.is_none() && origin.application.is_none(),
            "Desktop fixture must not invent Android app identity"
        );
        ensure!(
            flow.body_capture.len() == 4
                && VARIANTS
                    .iter()
                    .all(|variant| flow.body_capture.get(*variant)
                        == Some(&BodyCaptureState::NotRecorded)),
            "Body availability must be explicitly notRecorded"
        );
        ensure!(
            flow.request.body_base64.is_empty()
                && flow
                    .response
                    .as_ref()
                    .is_none_or(|response| response.body_base64.is_empty()),
            "Metadata fixture retained entity bytes"
        );
        ensure!(
            flow.original_request.is_none() && flow.original_response.is_none(),
            "Unexpected original entity snapshot"
        );
    }
    for first in [true, false] {
        let found: Vec<_> = flows
            .iter()
            .filter(|flow| flow.request.url == traffic.url(first))
            .collect();
        ensure!(found.len() == 1, "HTTP URL was missing or duplicated");
        let flow = found[0];
        ensure!(
            flow.request.method == if first { "POST" } else { "GET" },
            "HTTP method differs"
        );
        ensure!(
            flow.client_protocol.as_deref() == Some("HTTP/1.1"),
            "Wrong observed HTTP protocol"
        );
        let request_head = traffic.request_head(first);
        ensure!(
            raw_matches(flow.raw_request_head_base64.as_deref(), &request_head),
            "Raw request head differs"
        );
        ensure!(
            headers_match(&flow.request.headers, &request_head),
            "Request header order or case differs"
        );
        let response = flow
            .response
            .as_ref()
            .context("Missing HTTP response metadata")?;
        ensure!(
            response.status == if first { 200 } else { 204 } && response.version == "HTTP/1.1",
            "Wrong HTTP response metadata"
        );
        let response_head = traffic.response_head(first);
        ensure!(
            raw_matches(response.raw_head_base64.as_deref(), &response_head),
            "Raw response head differs"
        );
        ensure!(
            headers_match(&response.headers, &response_head),
            "Response header order or case differs"
        );
    }
    for port in [udp_port, dns_wire_port] {
        let url = format!("udp://{}:{port}", traffic.host);
        let found: Vec<_> = flows
            .iter()
            .filter(|flow| flow.request.url == url)
            .collect();
        ensure!(
            found.len() == 1,
            "UDP destination metadata was missing or duplicated"
        );
        let flow = found[0];
        ensure!(
            flow.request.method == "UDP"
                && flow.client_protocol.as_deref() == Some("UDP")
                && flow.response.is_none(),
            "Wrong UDP metadata protocol"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_directories_are_not_silently_reused() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        validate_directory(directory.path()).unwrap();
        fs::write(directory.path().join("existing-capture"), b"preserve").unwrap();
        assert!(validate_directory(directory.path()).is_err());
        assert_eq!(
            fs::read(directory.path().join("existing-capture")).unwrap(),
            b"preserve"
        );
    }
    #[test]
    fn private_reports_are_create_new_and_never_overwrite_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ready.json");
        write_private_json(&path, &json!({"fixture":true})).unwrap();
        assert!(write_private_json(&path, &json!({"fixture":false})).is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap()["fixture"],
            true
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn control_channel_only_accepts_fixed_commands_and_headers_are_case_order_sensitive() {
        assert!(matches!(command(b"verify"), Command::Verify));
        assert!(matches!(command(b"verify-stopped"), Command::VerifyStopped));
        assert!(matches!(command(b" verify"), Command::Invalid));
        assert!(matches!(command(b"verify\0"), Command::Invalid));
        let headers = vec![
            Header {
                disabled: false,
                name: "X-Order".into(),
                value: "one".into(),
            },
            Header {
                disabled: false,
                name: "x-order".into(),
                value: "two".into(),
            },
        ];
        assert!(headers_match(
            &headers,
            b"HTTP/1.1 204 No Content\r\nX-Order: one\r\nx-order: two\r\n\r\n"
        ));
        assert!(!headers_match(
            &headers,
            b"HTTP/1.1 204 No Content\r\nx-order: two\r\nX-Order: one\r\n\r\n"
        ));
        assert!(!raw_matches(Some("not base64"), b"HTTP"));
    }
    #[test]
    fn verification_rejects_missing_body_state_changed_order_device_and_unfinished_records() {
        let traffic = Traffic::new("a".repeat(32), Ipv4Addr::new(192, 168, 1, 10), 32001).unwrap();
        let mut flows = Vec::new();
        for index in 0..4 {
            let http = index < 2;
            let first = index == 0;
            let request_head = traffic.request_head(first);
            let response_head = traffic.response_head(first);
            let parse_headers = |head: &[u8]| -> Vec<Value> {
                String::from_utf8(head.to_vec())
                    .unwrap()
                    .split("\r\n")
                    .skip(1)
                    .filter(|line| !line.is_empty())
                    .map(|line| {
                        let (name, value) = line.split_once(": ").unwrap();
                        json!({"name":name,"value":value})
                    })
                    .collect()
            };
            let response = if http {
                json!({"status":if first {200}else{204},"version":"HTTP/1.1","headers":parse_headers(&response_head),"bodyBase64":"","rawHeadBase64":STANDARD.encode(response_head)})
            } else {
                Value::Null
            };
            let flow:Flow=serde_json::from_value(json!({"id":format!("fixture-{index}"),"parentId":null,"startedAt":0,"durationMs":0,"source":"capture","state":"completed",
                "bodyCapture":{"request":"notRecorded","response":"notRecorded","originalRequest":"notRecorded","originalResponse":"notRecorded"},
                "trafficOrigin":{"entry":"android-lan","processStatus":"remote","deviceName":"Android emulator","clientAddress":null,"process":null},
                "request":{"method":if http {if first {"POST"}else{"GET"}}else{"UDP"},
                    "url":if http {traffic.url(first)}else{format!("udp://{}:{}",traffic.host,if index==2 {32002}else{32003})},
                    "headers":if http {parse_headers(&request_head)}else{vec![]},"bodyBase64":""},
                "response":response,"error":null,"rawRequestHeadBase64":if http {Some(STANDARD.encode(request_head))}else{None},
                "clientProtocol":if http {"HTTP/1.1"}else{"UDP"},"notes":[]})).unwrap();
            flows.push(flow);
        }
        let valid =
            |flows: &[Flow]| validate_flows(flows, &traffic, 32002, 32003, "Android emulator");
        assert!(valid(&flows).is_ok());
        let mut changed = flows.clone();
        changed[0].body_capture.remove("response");
        assert!(valid(&changed).is_err());
        let mut changed = flows.clone();
        changed[0]
            .body_capture
            .insert("response".into(), BodyCaptureState::Recorded);
        assert!(valid(&changed).is_err());
        let mut changed = flows.clone();
        changed[1].request.headers.swap(1, 2);
        assert!(valid(&changed).is_err());
        let mut changed = flows.clone();
        changed[0].traffic_origin.as_mut().unwrap().device_name = Some("different".into());
        assert!(valid(&changed).is_err());
        let mut changed = flows.clone();
        changed[0].state = "receiving".into();
        assert!(valid(&changed).is_err());
        let mut changed = flows.clone();
        changed.push(flows[0].clone());
        assert!(valid(&changed).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_and_shared_directories_are_rejected_before_opening_engine() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let parent = tempfile::tempdir().unwrap();
        let actual = parent.path().join("actual");
        fs::create_dir(&actual).unwrap();
        fs::set_permissions(&actual, fs::Permissions::from_mode(0o700)).unwrap();
        let link = parent.path().join("link");
        symlink(&actual, &link).unwrap();
        assert!(validate_directory(&link).is_err());
        fs::set_permissions(&actual, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(validate_directory(&actual).is_err());
    }
}
