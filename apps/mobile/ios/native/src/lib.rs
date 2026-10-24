//! C ABI for the iOS Packet Tunnel extension. The host app talks to the
//! extension through NetworkExtension provider messages, never by opening a
//! second copy of the capture database or the loopback proxy.
use capture_core::store::McpRequestFilter;
use capture_core::{model::RequestDraft, CaptureRuntime, Engine};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::ffi::{c_char, c_void, CStr, CString};
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tun2proxy::{ArgDns, ArgProxy, Args, CancellationToken};

type PacketCallback = unsafe extern "C" fn(*const u8, usize, *mut c_void);
const MTU: u16 = 1500;
const MAX_PACKET: usize = MTU as usize;
const QUEUE_DEPTH: usize = 256;
const MAX_UPLOAD_BODY: usize = 1024 * 1024;
const MAX_UPLOAD_CHUNK: usize = 16 * 1024;
static UPLOAD_IDS: AtomicU64 = AtomicU64::new(1);

struct RequestUpload {
    token: u64,
    draft: RequestDraft,
    expected: usize,
    received: usize,
}

struct PacketDevice {
    incoming: mpsc::Receiver<Vec<u8>>,
    pending: Option<Vec<u8>>,
    callback: PacketCallback,
    context: usize,
}

impl AsyncRead for PacketDevice {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.pending.is_none() {
            self.pending = match self.incoming.poll_recv(cx) {
                Poll::Ready(packet) => packet,
                Poll::Pending => return Poll::Pending,
            };
        }
        let Some(packet) = self.pending.take() else {
            return Poll::Ready(Ok(()));
        };
        if packet.len() > buf.remaining() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "IP packet exceeds TUN read buffer",
            )));
        }
        buf.put_slice(&packet);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for PacketDevice {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if buf.len() > MAX_PACKET {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "IP packet exceeds maximum size",
            )));
        }
        // Swift copies the packet synchronously, then schedules writePackets.
        unsafe { (self.callback)(buf.as_ptr(), buf.len(), self.context as *mut c_void) };
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct Active {
    runtime: Arc<CaptureRuntime>,
    executor: tokio::runtime::Runtime,
    bridge: capture_core::proxy::ProxyHandle,
    quic: Option<capture_core::quic_proxy::QuicHandle>,
    sender: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<Result<usize, String>>,
    upload: Option<RequestUpload>,
}

static ACTIVE: OnceLock<Mutex<Option<Active>>> = OnceLock::new();
fn active() -> &'static Mutex<Option<Active>> {
    ACTIVE.get_or_init(|| Mutex::new(None))
}

fn start(
    path: &Path,
    port: u16,
    capture_quic: bool,
    callback: PacketCallback,
    context: usize,
) -> Result<serde_json::Value, String> {
    if port == 0 {
        return Err("Port must be between 1 and 65535".into());
    }
    let mut guard = active()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    if guard.is_some() {
        return Err("Capture is already running".into());
    }
    let executor = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let runtime = CaptureRuntime::new(Engine::open(path).map_err(|e| format!("{e:#}"))?);
    executor.block_on(runtime.start_proxy("127.0.0.1".into(), port))?;
    let status = executor.block_on(runtime.status());
    // VPN-originated TCP uses CONNECT for both HTTP and HTTPS. Only the
    // transparent bridge can distinguish plain HTTP after CONNECT from TLS.
    let bridge = executor
        .block_on(capture_core::proxy::start_tun_bridge(
            runtime.engine.clone(),
        ))
        .map_err(|e| format!("{e:#}"))?;
    let (sender, incoming) = mpsc::channel(QUEUE_DEPTH);
    let device = PacketDevice {
        incoming,
        pending: None,
        callback,
        context,
    };
    let mut args = Args::default();
    args.proxy = ArgProxy::try_from(format!("http://{}", bridge.address).as_str())
        .map_err(|e| e.to_string())?;
    args.dns = ArgDns::Virtual;
    args.direct_udp_for_http = true;
    args.ipv6_enabled = true;
    args.virtual_dns_pool_v6 = Some("fd00:198:19::/96".parse().map_err(|e| format!("{e}"))?);
    let quic = if capture_quic {
        match executor.block_on(capture_core::quic_proxy::start_tun(runtime.engine.clone())) {
            Ok(quic) => {
                args.udp_443_redirect = Some(quic.address);
                Some(quic)
            }
            Err(error) => {
                executor.block_on(bridge.stop());
                let _ = executor.block_on(runtime.shutdown_network());
                executor.shutdown_background();
                return Err(format!("HTTP/3 capture could not start: {error:#}"));
            }
        }
    } else {
        None
    };
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = executor.spawn(async move {
        tun2proxy::run(device, MTU, args, task_cancel)
            .await
            .map_err(|e| e.to_string())
    });
    *guard = Some(Active {
        runtime,
        executor,
        bridge,
        quic,
        sender,
        cancel,
        task,
        upload: None,
    });
    serde_json::to_value(status).map_err(|e| e.to_string())
}

fn stop() -> Result<serde_json::Value, String> {
    let mut guard = active()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let Some(mut capture) = guard.take()
    else {
        return Ok(serde_json::Value::Null);
    };
    // Keep startup and app messages out until the old proxy, bridge and
    // executor have actually stopped. Releasing this lock after take() would
    // let a new tunnel open the same port and database during teardown.
    capture.cancel.cancel();
    let tunnel_result = capture.executor.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), &mut capture.task).await
    });
    if tunnel_result.is_err() {
        capture.task.abort();
    }
    capture.executor.block_on(capture.bridge.stop());
    if let Some(quic) = capture.quic.take() {
        capture.executor.block_on(quic.stop());
    }
    let network_result = capture
        .executor
        .block_on(capture.runtime.shutdown_network());
    capture.executor.shutdown_background();
    network_result?;
    match tunnel_result {
        Ok(Ok(Ok(_))) => Ok(serde_json::Value::Null),
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("Tunnel did not stop within five seconds".into()),
    }
}

fn replay_request(draft: RequestDraft) -> Result<serde_json::Value, String> {
    let (runtime, handle) = {
        let guard = active()
            .lock()
            .map_err(|_| "Capture state is unavailable")?;
        let capture = guard.as_ref().ok_or("Capture is not running")?;
        (capture.runtime.clone(), capture.executor.handle().clone())
    };
    let flow = handle
        .block_on(runtime.engine.replay(draft, None, None))
        .map_err(|e| format!("{e:#}"))?;
    Ok(serde_json::json!({
        "id": flow.id,
        "status": flow.response.as_ref().map(|response| response.status),
        "error": flow.error,
    }))
}

fn command(request: &str) -> Result<serde_json::Value, String> {
    let request: serde_json::Value = serde_json::from_str(request).map_err(|e| e.to_string())?;
    let method = request
        .get("method")
        .and_then(|v| v.as_str())
        .ok_or("Missing method")?;
    if method == "send_request" {
        let draft: RequestDraft = serde_json::from_value(
            request
                .get("request")
                .cloned()
                .ok_or("Missing request draft")?,
        )
        .map_err(|e| e.to_string())?;
        return replay_request(draft);
    }
    if method == "begin_request_upload" {
        let expected = request.get("totalBytes").and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok())
            .ok_or("Invalid upload size")?;
        if expected > MAX_UPLOAD_BODY { return Err("Request body exceeds 1 MiB".into()); }
        let draft: RequestDraft = serde_json::from_value(request.get("request").cloned()
            .ok_or("Missing request draft")?).map_err(|e| e.to_string())?;
        if !draft.body_base64.is_empty() { return Err("Upload draft must have an empty body".into()); }
        let token = UPLOAD_IDS.fetch_add(1, Ordering::Relaxed);
        let mut guard = active().lock().map_err(|_| "Capture state is unavailable")?;
        let capture = guard.as_mut().ok_or("Capture is not running")?;
        capture.upload = Some(RequestUpload { token, draft, expected, received: 0 });
        return Ok(serde_json::json!({"token": token, "nextOffset": 0}));
    }
    if method == "append_request_upload" {
        let token = request.get("token").and_then(|value| value.as_u64()).ok_or("Invalid upload token")?;
        let offset = request.get("offset").and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok()).ok_or("Invalid upload offset")?;
        let encoded = request.get("bodyBase64").and_then(|value| value.as_str())
            .ok_or("Missing upload body chunk")?;
        if encoded.len() > MAX_UPLOAD_CHUNK.div_ceil(3) * 4 {
            return Err("Upload chunk exceeds 16 KiB".into());
        }
        let bytes = STANDARD.decode(encoded).map_err(|e| e.to_string())?;
        if bytes.is_empty() || bytes.len() > MAX_UPLOAD_CHUNK {
            return Err("Upload chunk must contain 1 to 16384 bytes".into());
        }
        let mut guard = active().lock().map_err(|_| "Capture state is unavailable")?;
        let capture = guard.as_mut().ok_or("Capture is not running")?;
        let upload = capture.upload.as_mut().ok_or("No request upload is active")?;
        if upload.token != token || upload.received != offset {
            return Err("Upload token or offset does not match".into());
        }
        let next = upload.received.checked_add(bytes.len()).ok_or("Upload size overflow")?;
        if next > upload.expected { return Err("Upload exceeds declared request body size".into()); }
        upload.draft.body_base64.append_complete_base64(&bytes).map_err(|e| e.to_string())?;
        upload.received = next;
        return Ok(serde_json::json!({"nextOffset": next}));
    }
    if method == "cancel_request_upload" || method == "commit_request_upload" {
        let token = request.get("token").and_then(|value| value.as_u64()).ok_or("Invalid upload token")?;
        let mut guard = active().lock().map_err(|_| "Capture state is unavailable")?;
        let capture = guard.as_mut().ok_or("Capture is not running")?;
        let upload = capture.upload.as_ref().ok_or("No request upload is active")?;
        if upload.token != token { return Err("Upload token does not match".into()); }
        if method == "commit_request_upload" && upload.received != upload.expected {
            return Err("Request body upload is incomplete".into());
        }
        let upload = capture.upload.take().unwrap();
        drop(guard);
        if method == "cancel_request_upload" { return Ok(serde_json::Value::Null); }
        return replay_request(upload.draft);
    }
    let guard = active()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = guard.as_ref().ok_or("Capture is not running")?;
    match method {
        "status" => {
            if capture.task.is_finished() {
                return Err("Tunnel stopped unexpectedly".into());
            }
            serde_json::to_value(capture.executor.block_on(capture.runtime.status()))
                .map_err(|e| e.to_string())
        }
        "recent" => {
            let cursor = request.get("cursor").and_then(|v| v.as_i64());
            let url_contains = match request.get("urlContains") {
                Some(value) => Some(value.as_str().ok_or("Invalid URL filter")?.to_owned())
                    .filter(|value| !value.is_empty()),
                None => None,
            };
            let flows = capture
                .runtime
                .engine
                .store
                .mcp_list_requests(McpRequestFilter {
                    cursor,
                    url_contains,
                    limit: 50,
                    ..Default::default()
                })
                .map_err(|e| format!("{e:#}"))?;
            serde_json::to_value(flows).map_err(|e| e.to_string())
        }
        "metadata" => {
            let id = request
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or("Missing request ID")?;
            let flow = capture
                .runtime
                .engine
                .store
                .metadata(id, None)
                .map_err(|e| format!("{e:#}"))?
                .ok_or("Recorded flow not found")?;
            serde_json::to_value(flow).map_err(|e| e.to_string())
        }
        "certificate" => std::fs::read_to_string(&capture.runtime.engine.ca.cert_path)
            .map(serde_json::Value::String)
            .map_err(|e| e.to_string()),
        "body_preview" => {
            let id = request
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or("Missing request ID")?;
            let variant = request
                .get("variant")
                .and_then(|v| v.as_str())
                .ok_or("Missing body variant")?;
            let revision = request
                .get("revision")
                .and_then(|v| v.as_str())
                .ok_or("Missing body revision")?;
            let offset = match request.get("offset") {
                Some(value) => value.as_u64().ok_or("Invalid body offset")?,
                None => 0,
            };
            let mut chunk = capture
                .runtime
                .engine
                .store
                .read_body_at_revision(id, variant, revision, offset, 16 * 1024)
                .map_err(|e| format!("{e:#}"))?;
            chunk.align_utf8_boundary().map_err(|e| e.to_string())?;
            serde_json::to_value(chunk).map_err(|e| e.to_string())
        }
        "blocking" => serde_json::to_value(capture.runtime.engine.blocking.snapshot())
            .map_err(|e| e.to_string()),
        "configure_blocking" => {
            let config: capture_core::blocking::Config = serde_json::from_value(
                request
                    .get("config")
                    .cloned()
                    .ok_or("Missing blocking config")?,
            )
            .map_err(|e| e.to_string())?;
            capture
                .runtime
                .engine
                .blocking
                .configure(config)
                .map_err(|e| format!("{e:#}"))?;
            serde_json::to_value(capture.runtime.engine.blocking.snapshot())
                .map_err(|e| e.to_string())
        }
        "response_rewrite" => serde_json::to_value(capture.runtime.engine.response_rewrite.snapshot())
            .map_err(|e| e.to_string()),
        "configure_response_rewrite" => {
            let config: capture_core::response_rewrite::Config = serde_json::from_value(
                request.get("config").cloned().ok_or("Missing response rewrite config")?,
            )
            .map_err(|e| e.to_string())?;
            let expected_version = request.get("expectedVersion")
                .and_then(|value| value.as_str())
                .ok_or("Missing response rewrite version")?;
            serde_json::to_value(capture.runtime.engine.response_rewrite
                .configure(config, expected_version)
                .map_err(|e| format!("{e:#}"))?)
                .map_err(|e| e.to_string())
        }
        _ => Err("Unknown command".into()),
    }
}

fn reply(result: Result<serde_json::Value, String>) -> *mut c_char {
    let value = match result {
        Ok(value) => serde_json::json!({"ok":true,"value":value}),
        Err(error) => serde_json::json!({"ok":false,"error":error}),
    };
    CString::new(value.to_string())
        .expect("JSON response contains no NUL")
        .into_raw()
}

unsafe fn input_string<'a>(ptr: *const c_char) -> Result<&'a str, String> {
    if ptr.is_null() {
        return Err("Missing string argument".into());
    }
    CStr::from_ptr(ptr).to_str().map_err(|e| e.to_string())
}

#[no_mangle]
pub unsafe extern "C" fn hc_ios_start(
    path: *const c_char,
    port: u16,
    capture_quic: bool,
    callback: Option<PacketCallback>,
    context: *mut c_void,
) -> *mut c_char {
    reply((|| {
        let path = input_string(path)?;
        let callback = callback.ok_or("Missing packet callback")?;
        start(Path::new(path), port, capture_quic, callback, context as usize)
    })())
}

#[no_mangle]
pub extern "C" fn hc_ios_stop() -> *mut c_char {
    reply(stop())
}

#[no_mangle]
pub unsafe extern "C" fn hc_ios_command(request: *const c_char) -> *mut c_char {
    reply(input_string(request).and_then(command))
}

#[no_mangle]
pub unsafe extern "C" fn hc_ios_input_packet(packet: *const u8, length: usize) -> bool {
    if packet.is_null() || length < 20 || length > MAX_PACKET {
        return false;
    }
    let bytes = std::slice::from_raw_parts(packet, length);
    if !matches!(bytes[0] >> 4, 4 | 6) {
        return false;
    }
    active()
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .map(|capture| capture.sender.try_send(bytes.to_vec()).is_ok())
        })
        .unwrap_or(false)
}

#[no_mangle]
pub unsafe extern "C" fn hc_ios_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Read, Write};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn tunnel_to(
        bridge_address: std::net::SocketAddr,
        origin_address: std::net::SocketAddr,
    ) -> std::io::BufReader<std::net::TcpStream> {
        let mut client =
            std::io::BufReader::new(std::net::TcpStream::connect(bridge_address).unwrap());
        client
            .get_mut()
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        client
            .get_mut()
            .write_all(
                format!("CONNECT {origin_address} HTTP/1.1\r\nHost: {origin_address}\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
        let mut status = String::new();
        client.read_line(&mut status).unwrap();
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
        loop {
            let mut line = String::new();
            client.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
        }
        client
    }
    #[test]
    fn rejects_bad_port() {
        let dir = tempfile::tempdir().unwrap();
        unsafe extern "C" fn packet(_: *const u8, _: usize, _: *mut c_void) {}
        assert!(start(dir.path(), 0, false, packet, 0).is_err());
    }

    #[test]
    fn rejects_invalid_packets() {
        assert!(!unsafe { hc_ios_input_packet(std::ptr::null(), 0) });
        let invalid = [0_u8; 20];
        assert!(!unsafe { hc_ios_input_packet(invalid.as_ptr(), invalid.len()) });
    }

    #[tokio::test]
    async fn packet_device_preserves_packet_boundaries() {
        unsafe extern "C" fn packet(_: *const u8, _: usize, _: *mut c_void) {}
        let (sender, incoming) = mpsc::channel(2);
        let mut device = PacketDevice {
            incoming,
            pending: None,
            callback: packet,
            context: 0,
        };
        sender.send(vec![0x45, 1, 2]).await.unwrap();
        sender.send(vec![0x60, 3]).await.unwrap();
        let mut buffer = [0; 16];
        assert_eq!(device.read(&mut buffer).await.unwrap(), 3);
        assert_eq!(&buffer[..3], &[0x45, 1, 2]);
        assert_eq!(device.read(&mut buffer).await.unwrap(), 2);
        assert_eq!(&buffer[..2], &[0x60, 3]);
        assert!(device.write_all(&[0x45, 4, 5]).await.is_ok());
    }

    #[test]
    fn extension_lifecycle_and_blocking_commands() {
        unsafe extern "C" fn packet(_: *const u8, _: usize, _: *mut c_void) {}
        let directory = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let result = start(directory.path(), port, false, packet, 0).unwrap();
        assert!(result.is_object());
        assert!(command(r#"{"method":"status"}"#).is_ok());
        assert_eq!(
            command(r#"{"method":"blocking"}"#).unwrap()["enabled"],
            false
        );
        let configured = command(r#"{"method":"configure_blocking","config":{"enabled":true,"scope":"capture","rules":[{"id":"mobile-test","enabled":true,"matchType":"host","pattern":"example.invalid","method":""}]}}"#).unwrap();
        assert_eq!(configured["rules"][0]["pattern"], "example.invalid");
        let snapshot = command(r#"{"method":"response_rewrite"}"#).unwrap();
        let update = serde_json::json!({"method":"configure_response_rewrite",
            "expectedVersion":snapshot["version"],
            "config":{"enabled":true,"scope":"capture","rules":[{
                "id":"ios-response-test","name":"Replace text","enabled":true,
                "matchType":"host","pattern":"example.invalid","method":"",
                "action":"replace","replacement":{"status":202,"headers":[],"bodyBase64":"b2s="}
            }]}});
        let saved = command(&update.to_string()).unwrap();
        assert_eq!(saved["config"]["rules"][0]["replacement"]["status"], 202);
        assert!(command(&update.to_string()).is_err());

        let origin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin_port = origin.local_addr().unwrap().port();
        let origin_task = std::thread::spawn(move || {
            let (mut stream, _) = origin.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = [0_u8; 1024];
            let count = stream.read(&mut request).unwrap();
            assert!(request[..count].starts_with(b"GET /mobile HTTP/1.1"));
            stream.write_all(
                b"HTTP/1.1 201 Created\r\nContent-Length: 20000\r\nConnection: close\r\n\r\n",
            ).unwrap();
            let mut response_body = vec![b'a'; 20_000];
            response_body[16 * 1024 - 1] = 0xC3;
            response_body[16 * 1024] = 0xA9;
            stream.write_all(&response_body).unwrap();
        });
        let url = format!("http://127.0.0.1:{origin_port}/mobile");
        let sent = command(
            &serde_json::json!({"method":"send_request","request":{
                "engine":"native","method":"GET","url":url,"headers":[],"bodyBase64":""
            }})
            .to_string(),
        )
        .unwrap();
        assert_eq!(sent["status"], 201, "{sent}");
        let id = sent["id"].as_str().unwrap();
        let metadata =
            command(&serde_json::json!({"method":"metadata","id":id}).to_string()).unwrap();
        assert_eq!(metadata["response"]["status"], 201);
        let body = command(
            &serde_json::json!({
                "method":"body_preview","id":id,"variant":"response",
                "revision":metadata["bodyRevision"]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(body["text"].as_str().unwrap().len(), 16 * 1024 - 1);
        assert_eq!(body["nextOffset"], 16 * 1024 - 1);
        let second_chunk = command(&serde_json::json!({
            "method":"body_preview","id":id,"variant":"response",
            "revision":metadata["bodyRevision"],"offset":16 * 1024 - 1
        }).to_string()).unwrap();
        assert!(second_chunk["text"].as_str().unwrap().starts_with("é"));
        assert_eq!(
            second_chunk["text"].as_str().unwrap().len(),
            20_000 - (16 * 1024 - 1)
        );
        assert_eq!(second_chunk["offset"], 16 * 1024 - 1);
        assert!(second_chunk["nextOffset"].is_null());
        assert!(command(&serde_json::json!({
            "method":"body_preview","id":id,"variant":"response",
            "revision":metadata["bodyRevision"],"offset":-1
        }).to_string()).is_err());
        let page = command(r#"{"method":"recent"}"#).unwrap();
        assert_eq!(page["requests"][0]["id"], id);
        let filtered = command(r#"{"method":"recent","urlContains":"/mobile"}"#).unwrap();
        assert_eq!(filtered["requests"][0]["id"], id);
        let missing = command(r#"{"method":"recent","urlContains":"/unmatched-path"}"#).unwrap();
        assert!(missing["requests"].as_array().unwrap().is_empty());
        origin_task.join().unwrap();

        // The packet adapter always sends CONNECT, including for plain HTTP.
        // Verify that its dedicated listener accepts the HTTP request after it.
        let bridge_address = active().lock().unwrap().as_ref().unwrap().bridge.address;
        let plain_origin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let plain_address = plain_origin.local_addr().unwrap();
        let plain_task = std::thread::spawn(move || {
            let (mut stream, _) = plain_origin.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = [0_u8; 1024];
            let count = stream.read(&mut request).unwrap();
            assert!(request[..count].starts_with(b"GET /bridge HTTP/1.1"));
            stream
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                )
                .unwrap();
        });
        let mut bridge =
            std::io::BufReader::new(std::net::TcpStream::connect(bridge_address).unwrap());
        bridge
            .get_mut()
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        bridge
            .get_mut()
            .write_all(
                format!("CONNECT {plain_address} HTTP/1.1\r\nHost: {plain_address}\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
        let mut head = String::new();
        loop {
            let mut line = String::new();
            bridge.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            head.push_str(&line);
        }
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        bridge
            .get_mut()
            .write_all(
                format!(
                    "GET /bridge HTTP/1.1\r\nHost: {plain_address}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        head.clear();
        loop {
            let mut line = String::new();
            bridge.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            head.push_str(&line);
        }
        assert!(head.starts_with("HTTP/1.1 202"), "{head}");
        let mut body = [0_u8; 2];
        bridge.read_exact(&mut body).unwrap();
        assert_eq!(&body, b"ok");
        plain_task.join().unwrap();

        // VPN forwarding must not turn unrelated binary TCP traffic into HTTP.
        let binary_origin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let binary_address = binary_origin.local_addr().unwrap();
        let binary_task = std::thread::spawn(move || {
            let (mut stream, _) = binary_origin.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut input = [0_u8; 4];
            stream.read_exact(&mut input).unwrap();
            assert_eq!(&input, &[b'G', 0, 1, 2]);
            stream.write_all(&[3, 2, 1, 0]).unwrap();
        });
        let mut binary = tunnel_to(bridge_address, binary_address);
        binary.get_mut().write_all(&[b'G', 0, 1, 2]).unwrap();
        let mut echoed = [0_u8; 4];
        binary.read_exact(&mut echoed).unwrap();
        assert_eq!(&echoed, &[3, 2, 1, 0]);
        binary_task.join().unwrap();

        // A server-first protocol must also work when the client has no bytes
        // to classify immediately after CONNECT.
        let greeting_origin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let greeting_address = greeting_origin.local_addr().unwrap();
        let greeting_task = std::thread::spawn(move || {
            let (mut stream, _) = greeting_origin.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            stream.write_all(b"HELLO").unwrap();
            let mut input = [0_u8; 4];
            stream.read_exact(&mut input).unwrap();
            assert_eq!(&input, b"PING");
            stream.write_all(b"PONG").unwrap();
        });
        let mut greeting = tunnel_to(bridge_address, greeting_address);
        let mut response = [0_u8; 5];
        greeting.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"HELLO");
        greeting.get_mut().write_all(b"PING").unwrap();
        let mut response = [0_u8; 4];
        greeting.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"PONG");
        greeting_task.join().unwrap();

        // A ClientHello advertising only a non-HTTP protocol must reach the
        // origin unchanged, rather than being answered with an HTTPS MITM cert.
        let tls_origin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tls_address = tls_origin.local_addr().unwrap();
        let tls_task = std::thread::spawn(move || {
            let (mut stream, _) = tls_origin.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut input = [0_u8; 4096];
            let count = stream.read(&mut input).unwrap();
            assert!(count > 5 && input.starts_with(&[22, 3]));
            stream.write_all(b"RAW").unwrap();
        });
        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
        config.alpn_protocols = vec![b"smtp".to_vec()];
        let mut connection = rustls::ClientConnection::new(
            Arc::new(config),
            rustls::pki_types::ServerName::try_from("localhost").unwrap(),
        )
        .unwrap();
        let mut hello = Vec::new();
        connection.write_tls(&mut hello).unwrap();
        let mut tls_client = tunnel_to(bridge_address, tls_address);
        tls_client.get_mut().write_all(&hello).unwrap();
        let mut response = [0_u8; 3];
        tls_client.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"RAW");
        tls_task.join().unwrap();

        let upload_origin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let upload_port = upload_origin.local_addr().unwrap().port();
        let payload: Vec<u8> = (0..160 * 1024).map(|offset| (offset % 251) as u8).collect();
        let expected = payload.clone();
        let upload_task = std::thread::spawn(move || {
            let (stream, _) = upload_origin.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
            let mut reader = std::io::BufReader::new(stream);
            let mut head = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" { break; }
                head.push_str(&line);
            }
            assert!(head.starts_with("POST /upload HTTP/1.1"));
            assert!(head.to_ascii_lowercase().contains("content-length: 163840"));
            let mut received = vec![0; expected.len()];
            reader.read_exact(&mut received).unwrap();
            assert_eq!(received, expected);
            reader.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
        });
        let upload_draft = serde_json::json!({
            "engine":"native", "method":"POST",
            "url":format!("http://127.0.0.1:{upload_port}/upload"),
            "headers":[], "bodyBase64":""
        });
        assert!(command(&serde_json::json!({"method":"begin_request_upload",
            "request":upload_draft,"totalBytes":MAX_UPLOAD_BODY+1}).to_string()).is_err());
        let begun = command(&serde_json::json!({"method":"begin_request_upload",
            "request":upload_draft,"totalBytes":payload.len()}).to_string()).unwrap();
        let token = begun["token"].as_u64().unwrap();
        assert!(command(&serde_json::json!({"method":"append_request_upload",
            "token":token,"offset":1,"bodyBase64":"YQ=="}).to_string()).is_err());
        assert!(command(&serde_json::json!({"method":"append_request_upload",
            "token":token,"offset":0,"bodyBase64":"%%%"}).to_string()).is_err());
        assert!(command(&serde_json::json!({"method":"append_request_upload",
            "token":token+1,"offset":0,"bodyBase64":"YQ=="}).to_string()).is_err());
        assert!(command(&serde_json::json!({"method":"commit_request_upload",
            "token":token}).to_string()).is_err());
        for (index, chunk) in payload.chunks(MAX_UPLOAD_CHUNK).enumerate() {
            let next = command(&serde_json::json!({"method":"append_request_upload",
                "token":token,"offset":index*MAX_UPLOAD_CHUNK,
                "bodyBase64":STANDARD.encode(chunk)}).to_string()).unwrap();
            assert_eq!(next["nextOffset"], (index+1)*MAX_UPLOAD_CHUNK);
        }
        assert!(command(&serde_json::json!({"method":"append_request_upload",
            "token":token,"offset":0,"bodyBase64":"YQ=="}).to_string()).is_err());
        let uploaded = command(&serde_json::json!({"method":"commit_request_upload",
            "token":token}).to_string()).unwrap();
        assert_eq!(uploaded["status"], 200, "{uploaded}");
        upload_task.join().unwrap();
        let cancelled = command(&serde_json::json!({"method":"begin_request_upload",
            "request":upload_draft,"totalBytes":1}).to_string()).unwrap();
        let cancelled_token = cancelled["token"].as_u64().unwrap();
        command(&serde_json::json!({"method":"cancel_request_upload",
            "token":cancelled_token}).to_string()).unwrap();
        assert!(command(&serde_json::json!({"method":"commit_request_upload",
            "token":cancelled_token}).to_string()).is_err());
        let full = command(&serde_json::json!({"method":"begin_request_upload",
            "request":upload_draft,"totalBytes":MAX_UPLOAD_BODY}).to_string()).unwrap();
        let full_token = full["token"].as_u64().unwrap();
        let full_chunk = vec![0xA5; MAX_UPLOAD_CHUNK];
        for index in 0..MAX_UPLOAD_BODY / MAX_UPLOAD_CHUNK {
            command(&serde_json::json!({"method":"append_request_upload",
                "token":full_token,"offset":index*MAX_UPLOAD_CHUNK,
                "bodyBase64":STANDARD.encode(&full_chunk)}).to_string()).unwrap();
        }
        {
            let guard = active().lock().unwrap();
            let upload = guard.as_ref().unwrap().upload.as_ref().unwrap();
            assert_eq!(upload.received, MAX_UPLOAD_BODY);
            assert_eq!(STANDARD.decode(upload.draft.body_base64.as_str()).unwrap(),
                vec![0xA5; MAX_UPLOAD_BODY]);
        }
        let replaced = command(&serde_json::json!({"method":"begin_request_upload",
            "request":upload_draft,"totalBytes":0}).to_string()).unwrap();
        assert!(command(&serde_json::json!({"method":"commit_request_upload",
            "token":full_token}).to_string()).is_err());
        command(&serde_json::json!({"method":"cancel_request_upload",
            "token":replaced["token"]}).to_string()).unwrap();
        assert!(stop().is_ok());
        assert!(command(r#"{"method":"status"}"#).is_err());
        assert!(command(r#"{"method":"blocking"}"#).is_err());

        // The optional HTTP/3 listener is owned by the extension lifecycle.
        start(directory.path(), port, true, packet, 0).unwrap();
        let quic_address = active()
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .quic
            .as_ref()
            .unwrap()
            .address;
        assert!(quic_address.ip().is_loopback());
        assert!(stop().is_ok());
        std::net::UdpSocket::bind(quic_address)
            .expect("stopping the extension must release the HTTP/3 listener");
    }
}
