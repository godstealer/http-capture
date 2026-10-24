//! C ABI for the iOS Packet Tunnel extension and host-side history access.
//! The host uses provider messages while the tunnel is connected and the
//! shared App Group store and replay engine when it is stopped.
use capture_core::store::McpRequestFilter;
use capture_core::store::{Collection, SavedRequest};
use capture_core::{model::RequestDraft, CaptureRuntime, Engine};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::ffi::{c_char, c_void, CStr, CString};
use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tun2proxy::{ArgDns, ArgProxy, Args, CancellationToken};
use uuid::Uuid;

type PacketCallback = unsafe extern "C" fn(*const u8, usize, *mut c_void);
const MTU: u16 = 1500;
const MAX_PACKET: usize = MTU as usize;
const QUEUE_DEPTH: usize = 256;
const MAX_UPLOAD_BODY: usize = 1024 * 1024;
const MAX_UPLOAD_CHUNK: usize = 16 * 1024;
static UPLOAD_IDS: AtomicU64 = AtomicU64::new(1);
const SESSION_EXPORT_CHUNK: usize = 32 * 1024;
static OFFLINE_STORE: OnceLock<Mutex<HashMap<PathBuf, Arc<capture_core::store::Store>>>> = OnceLock::new();
static OFFLINE_SAVED_READ: OnceLock<Mutex<Option<SavedRead>>> = OnceLock::new();
static OFFLINE_CERTIFICATES: OnceLock<Mutex<HashMap<PathBuf, Arc<capture_core::certificates::Certificates>>>> = OnceLock::new();

struct RequestUpload {
    token: u64,
    draft: RequestDraft,
    expected: usize,
    received: usize,
    action: UploadAction,
}

enum UploadAction {
    Replay,
    Save { id: Option<String>, name: String, collection_id: Option<String>, new_collection_name: Option<String> },
}

struct SavedRead {
    id: String,
    token: u64,
    body_base64: String,
}

struct SessionExport {
    token: u64,
    path: std::path::PathBuf,
    file: std::fs::File,
    total: u64,
}

impl Drop for SessionExport {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.path); }
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
    saved_read: Option<SavedRead>,
    session_export: Option<SessionExport>,
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
    let bridge = match executor.block_on(capture_core::proxy::start_tun_bridge(
        runtime.engine.clone(),
    )) {
        Ok(bridge) => bridge,
        Err(error) => {
            let _ = executor.block_on(runtime.shutdown_network());
            executor.shutdown_background();
            return Err(format!("Transparent bridge could not start: {error:#}"));
        }
    };
    let (sender, incoming) = mpsc::channel(QUEUE_DEPTH);
    let device = PacketDevice {
        incoming,
        pending: None,
        callback,
        context,
    };
    let mut args = Args::default();
    args.proxy = match ArgProxy::try_from(format!("http://{}", bridge.address).as_str()) {
        Ok(proxy) => proxy,
        Err(error) => {
            executor.block_on(bridge.stop());
            let _ = executor.block_on(runtime.shutdown_network());
            executor.shutdown_background();
            return Err(format!("Transparent bridge address is invalid: {error}"));
        }
    };
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
        saved_read: None,
        session_export: None,
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

fn replay_request_offline(data_dir: &Path, draft: RequestDraft)
    -> Result<serde_json::Value, String> {
    // The Packet Tunnel performs the first-run migration before the host may
    // open this directory. Do not create a new shared database ahead of it.
    offline_store(data_dir)?;
    let executor = tokio::runtime::Builder::new_multi_thread()
        .enable_all().build().map_err(|error| error.to_string())?;
    let engine = Engine::open(data_dir).map_err(|error| format!("{error:#}"))?;
    let result = executor.block_on(engine.replay(draft, None, None));
    executor.shutdown_background();
    let flow = result.map_err(|error| format!("{error:#}"))?;
    Ok(serde_json::json!({
        "id": flow.id,
        "status": flow.response.as_ref().map(|response| response.status),
        "error": flow.error,
    }))
}

fn save_request(id: Option<&str>, name: &str, draft: RequestDraft,
                collection_id: Option<&str>, new_collection_name: Option<&str>) -> Result<serde_json::Value, String> {
    let guard = active().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = guard.as_ref().ok_or("Capture is not running")?;
    save_request_in(&capture.runtime.engine.store, id, name, draft, collection_id, new_collection_name)
}

fn save_request_in(store: &capture_core::store::Store, id: Option<&str>, name: &str,
                   draft: RequestDraft, collection_id: Option<&str>,
                   new_collection_name: Option<&str>) -> Result<serde_json::Value, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 512 { return Err("Request name must be 1–512 bytes".into()); }
    if draft.body_base64.len() > MAX_UPLOAD_BODY.div_ceil(3) * 4 {
        return Err("Request body exceeds 1 MiB".into());
    }
    capture_core::http1::validate_draft(&draft).map_err(|error| error.to_string())?;
    let mut library = store.library().map_err(|error| error.to_string())?;
    if collection_id.is_some() && new_collection_name.is_some() {
        return Err("Choose an existing collection or create a new one".into());
    }
    let selected_collection = if let Some(new_name) = new_collection_name {
        let new_name = new_name.trim();
        if new_name.is_empty() || new_name.len() > 512 {
            return Err("Collection name must be 1–512 bytes".into());
        }
        if library.collections.iter().any(|collection| collection.name.eq_ignore_ascii_case(new_name)) {
            return Err("Collection name already exists".into());
        }
        let id = Uuid::new_v4().to_string();
        library.collections.push(Collection { id: id.clone(), name: new_name.into() });
        Some(id)
    } else if let Some(id) = collection_id {
        if !library.collections.iter().any(|collection| collection.id == id) {
            return Err("Selected collection no longer exists".into());
        }
        Some(id.to_owned())
    } else { None };
    let id = if let Some(id) = id {
        let request = library.requests.iter_mut().find(|request| request.id == id)
            .ok_or("Saved request not found")?;
        if request.grpc_draft.is_some() || request.request_mode.as_deref() == Some("grpc") {
            return Err("gRPC requests cannot be edited in the mobile HTTP editor".into());
        }
        request.name = name.into();
        request.draft = draft;
        if let Some(collection_id) = selected_collection { request.collection_id = collection_id; }
        id.to_owned()
    } else {
        let collection_id = if let Some(id) = selected_collection { id }
        else if let Some(collection) = library.collections.iter()
            .find(|collection| collection.name == "我的请求") {
            collection.id.clone()
        } else {
            let id = Uuid::new_v4().to_string();
            library.collections.push(Collection { id: id.clone(), name: "我的请求".into() });
            id
        };
        let id = Uuid::new_v4().to_string();
        library.requests.push(SavedRequest { id: id.clone(), name: name.into(), collection_id,
            draft, grpc_draft: None, request_mode: Some("http".into()) });
        id
    };
    let revision = store.save_library(library).map_err(|error| error.to_string())?;
    Ok(serde_json::json!({"id": id, "revision": revision}))
}

fn list_saved_collections() -> Result<serde_json::Value, String> {
    let guard = active().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = guard.as_ref().ok_or("Capture is not running")?;
    list_saved_collections_in(&capture.runtime.engine.store)
}

fn list_saved_collections_in(store: &capture_core::store::Store) -> Result<serde_json::Value, String> {
    let library = store.library().map_err(|error| error.to_string())?;
    Ok(serde_json::json!({"collections": library.collections.iter().map(|collection| {
        let count = library.requests.iter().filter(|request| request.collection_id == collection.id).count();
        serde_json::json!({"id": collection.id, "name": collection.name, "count": count})
    }).collect::<Vec<_>>() }))
}

fn change_saved_collection(action: &str, id: &str, name: Option<&str>) -> Result<serde_json::Value, String> {
    let guard = active().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = guard.as_ref().ok_or("Capture is not running")?;
    change_saved_collection_in(&capture.runtime.engine.store, action, id, name)
}

fn change_saved_collection_in(store: &capture_core::store::Store, action: &str, id: &str,
                              name: Option<&str>) -> Result<serde_json::Value, String> {
    let mut library = store.library().map_err(|error| error.to_string())?;
    let index = library.collections.iter().position(|collection| collection.id == id)
        .ok_or("Collection not found")?;
    match action {
        "rename" => {
            let name = name.ok_or("Missing collection name")?.trim();
            if name.is_empty() || name.len() > 512 {
                return Err("Collection name must be 1–512 bytes".into());
            }
            if library.collections.iter().enumerate().any(|(other, collection)|
                other != index && collection.name.eq_ignore_ascii_case(name)) {
                return Err("Collection name already exists".into());
            }
            library.collections[index].name = name.into();
        }
        "delete" => {
            if library.requests.iter().any(|request| request.collection_id == id) {
                return Err("Move or delete requests before deleting this collection".into());
            }
            library.collections.remove(index);
        }
        _ => return Err("Unsupported collection action".into()),
    }
    let revision = store.save_library(library).map_err(|error| error.to_string())?;
    Ok(serde_json::json!({"id": id, "revision": revision}))
}

fn list_saved_requests(offset: usize, query: &str, collection_id: &str) -> Result<serde_json::Value, String> {
    let guard = active().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = guard.as_ref().ok_or("Capture is not running")?;
    list_saved_requests_in(&capture.runtime.engine.store, offset, query, collection_id)
}

fn list_saved_requests_in(store: &capture_core::store::Store, offset: usize, query: &str,
                          collection_id: &str) -> Result<serde_json::Value, String> {
    let query = query.trim().to_lowercase();
    if query.len() > 256 { return Err("Saved request search is limited to 256 bytes".into()); }
    let library = store.library().map_err(|error| error.to_string())?;
    if !collection_id.is_empty() && !library.collections.iter().any(|collection| collection.id == collection_id) {
        return Err("Selected collection no longer exists".into());
    }
    let matches = |request: &&SavedRequest| (collection_id.is_empty() || request.collection_id == collection_id)
        && (query.is_empty() || request.name.to_lowercase().contains(&query)
            || request.draft.method.to_lowercase().contains(&query)
            || request.draft.url.to_lowercase().contains(&query));
    let total = library.requests.iter().filter(&matches).count();
    let requests: Vec<_> = library.requests.iter().rev().filter(matches).skip(offset).take(100).map(|request| {
        let collection_name = library.collections.iter()
            .find(|collection| collection.id == request.collection_id)
            .map(|collection| collection.name.as_str()).unwrap_or("");
        serde_json::json!({"id":request.id,
            "name":request.name.chars().take(32).collect::<String>(),
            "collectionName":collection_name.chars().take(16).collect::<String>(),
            "method":request.draft.method.chars().take(16).collect::<String>(),
            "url":request.draft.url.chars().take(128).collect::<String>()})
    }).collect();
    Ok(serde_json::json!({"total":total,"nextOffset":
        (offset + requests.len() < total).then_some(offset + requests.len()),"requests":requests}))
}

fn delete_saved_request(id: &str) -> Result<serde_json::Value, String> {
    let mut guard = active().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = guard.as_mut().ok_or("Capture is not running")?;
    let result = delete_saved_request_in(&capture.runtime.engine.store, id)?;
    if capture.saved_read.as_ref().is_some_and(|read| read.id == id) {
        capture.saved_read = None;
    }
    Ok(result)
}

fn delete_saved_request_in(store: &capture_core::store::Store, id: &str)
    -> Result<serde_json::Value, String> {
    let mut library = store.library().map_err(|error| error.to_string())?;
    let index = library.requests.iter().position(|request| request.id == id)
        .ok_or("Saved request not found")?;
    library.requests.remove(index);
    let revision = store.save_library(library).map_err(|error| error.to_string())?;
    Ok(serde_json::json!({"id":id,"revision":revision}))
}

fn open_saved_request(id: &str) -> Result<serde_json::Value, String> {
    let mut guard = active().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = guard.as_mut().ok_or("Capture is not running")?;
    open_saved_request_in(&capture.runtime.engine.store, &mut capture.saved_read, id)
}

fn open_saved_request_in(store: &capture_core::store::Store,
                         saved_read: &mut Option<SavedRead>, id: &str)
    -> Result<serde_json::Value, String> {
    let library = store.library().map_err(|error| error.to_string())?;
    let request = library.requests.iter().find(|request| request.id == id)
        .ok_or("Saved request not found")?;
    if request.draft.body_base64.len() > MAX_UPLOAD_BODY.div_ceil(3) * 4 {
        return Err("Saved request body exceeds 1 MiB".into());
    }
    let body = STANDARD.decode(request.draft.body_base64.as_bytes()).map_err(|error| error.to_string())?;
    if body.len() > MAX_UPLOAD_BODY { return Err("Saved request body exceeds 1 MiB".into()); }
    let mut value = serde_json::to_value(request).map_err(|error| error.to_string())?;
    value["draft"]["bodyBase64"] = serde_json::json!("");
    let token = UPLOAD_IDS.fetch_add(1, Ordering::Relaxed);
    *saved_read = Some(SavedRead { id: id.to_owned(), token,
        body_base64: request.draft.body_base64.as_str().to_owned() });
    let result = serde_json::json!({"request":value,"totalBytes":body.len(),"readToken":token});
    if serde_json::to_vec(&result).map_err(|error| error.to_string())?.len() > 96 * 1024 {
        *saved_read = None;
        return Err("Saved request headers exceed the tunnel message limit".into());
    }
    Ok(result)
}

fn saved_request_body(token: u64, offset: usize) -> Result<serde_json::Value, String> {
    let mut guard = active().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = guard.as_mut().ok_or("Capture is not running")?;
    saved_request_body_in(&mut capture.saved_read, token, offset)
}

fn saved_request_body_in(saved_read: &mut Option<SavedRead>, token: u64, offset: usize)
    -> Result<serde_json::Value, String> {
    let read = saved_read.as_ref().ok_or("No saved request is open")?;
    if read.token != token { return Err("Saved request read token has expired".into()); }
    let encoded = read.body_base64.as_str();
    let total = encoded.len() / 4 * 3 - encoded.as_bytes().iter().rev().take_while(|b| **b == b'=').count();
    const CHUNK: usize = 12 * 1024;
    if offset >= total || offset % CHUNK != 0 { return Err("Invalid saved body offset".into()); }
    let end = (offset + CHUNK).min(total);
    let begin_char = offset / 3 * 4;
    let end_char = if end == total { encoded.len() } else { end / 3 * 4 };
    let part = encoded[begin_char..end_char].to_owned();
    if end == total { *saved_read = None; }
    Ok(serde_json::json!({"offset":offset,"totalBytes":total,
        "bodyBase64":part,
        "nextOffset":(end < total).then_some(end)}))
}

fn begin_session_export() -> Result<serde_json::Value, String> {
    let store = {
        let guard = active().lock().map_err(|_| "Capture state is unavailable")?;
        guard.as_ref().ok_or("Capture is not running")?.runtime.engine.store.clone()
    };
    let token = UPLOAD_IDS.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("http-capture-session-{}-{token}.json", Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().read(true).write(true).create_new(true)
            .open(&path).map_err(|error| error.to_string())?;
        let count = store.export_session_json(&mut file).map_err(|error| format!("{error:#}"))?;
        file.flush().map_err(|error| error.to_string())?;
        let total = file.metadata().map_err(|error| error.to_string())?.len();
        let mut guard = active().lock().map_err(|_| "Capture state is unavailable")?;
        let capture = guard.as_mut().ok_or("Capture stopped during export")?;
        capture.session_export = Some(SessionExport { token, path: path.clone(), file, total });
        Ok(serde_json::json!({"token":token,"count":count,"totalBytes":total}))
    })();
    if result.is_err() { let _ = std::fs::remove_file(&path); }
    result
}

fn read_session_export(token: u64, offset: u64) -> Result<serde_json::Value, String> {
    let mut guard = active().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = guard.as_mut().ok_or("Capture is not running")?;
    let export = capture.session_export.as_mut().ok_or("No session export is active")?;
    if export.token != token || offset > export.total { return Err("Export token or offset does not match".into()); }
    export.file.seek(SeekFrom::Start(offset)).map_err(|error| error.to_string())?;
    let length = usize::try_from((export.total - offset).min(SESSION_EXPORT_CHUNK as u64))
        .map_err(|error| error.to_string())?;
    let mut bytes = vec![0; length];
    export.file.read_exact(&mut bytes).map_err(|error| error.to_string())?;
    let next = offset + length as u64;
    Ok(serde_json::json!({"offset":offset,"totalBytes":export.total,
        "chunkBase64":STANDARD.encode(bytes),"nextOffset":(next < export.total).then_some(next)}))
}

fn finish_session_export(token: u64) -> Result<serde_json::Value, String> {
    let mut guard = active().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = guard.as_mut().ok_or("Capture is not running")?;
    if capture.session_export.as_ref().is_none_or(|export| export.token != token) {
        return Err("Export token does not match".into());
    }
    capture.session_export = None;
    Ok(serde_json::Value::Null)
}

fn offline_store(data_dir: &Path) -> Result<Arc<capture_core::store::Store>, String> {
    let path = data_dir.join("sessions.db");
    if !path.is_file() {
        return Err("尚无共享抓包记录；升级后请先启动一次 VPN 迁移旧记录".into());
    }
    let mut cached = OFFLINE_STORE.get_or_init(|| Mutex::new(HashMap::new()))
        .lock().map_err(|_| "Offline history is unavailable")?;
    if let Some(store) = cached.get(&path) { return Ok(store.clone()); }
    let store = Arc::new(capture_core::store::Store::open(&path)
        .map_err(|error| format!("{error:#}"))?);
    cached.insert(path, store.clone());
    Ok(store)
}

fn offline_certificates(data_dir: &Path)
    -> Result<Arc<capture_core::certificates::Certificates>, String> {
    let mut cached = OFFLINE_CERTIFICATES.get_or_init(|| Mutex::new(HashMap::new()))
        .lock().map_err(|_| "Offline certificates are unavailable")?;
    if let Some(certificates) = cached.get(data_dir) { return Ok(certificates.clone()); }
    let certificates = capture_core::certificates::Certificates::open(data_dir)
        .map_err(|error| format!("{error:#}"))?;
    cached.insert(data_dir.to_path_buf(), certificates.clone());
    Ok(certificates)
}

fn history_command(store: &capture_core::store::Store, method: &str,
                   request: &serde_json::Value) -> Result<serde_json::Value, String> {
    match method {
        "recent" => {
            let cursor = request.get("cursor").and_then(|value| value.as_i64());
            let url_contains = match request.get("urlContains") {
                Some(value) => Some(value.as_str().ok_or("Invalid URL filter")?.to_owned())
                    .filter(|value| !value.is_empty()),
                None => None,
            };
            let flows = store.mcp_list_requests(McpRequestFilter {
                cursor, url_contains, limit: 50, ..Default::default()
            }).map_err(|error| format!("{error:#}"))?;
            serde_json::to_value(flows).map_err(|error| error.to_string())
        }
        "metadata" => {
            let id = request.get("id").and_then(|value| value.as_str())
                .ok_or("Missing request ID")?;
            let flow = store.metadata(id, None).map_err(|error| format!("{error:#}"))?
                .ok_or("Recorded flow not found")?;
            serde_json::to_value(flow).map_err(|error| error.to_string())
        }
        "body_preview" => {
            let id = request.get("id").and_then(|value| value.as_str())
                .ok_or("Missing request ID")?;
            let variant = request.get("variant").and_then(|value| value.as_str())
                .ok_or("Missing body variant")?;
            let revision = request.get("revision").and_then(|value| value.as_str())
                .ok_or("Missing body revision")?;
            let offset = match request.get("offset") {
                Some(value) => value.as_u64().ok_or("Invalid body offset")?,
                None => 0,
            };
            let mut chunk = store.read_body_at_revision(id, variant, revision, offset, 16 * 1024)
                .map_err(|error| format!("{error:#}"))?;
            chunk.align_utf8_boundary().map_err(|error| error.to_string())?;
            serde_json::to_value(chunk).map_err(|error| error.to_string())
        }
        _ => Err("Offline command is read-only and limited to capture history".into()),
    }
}

fn offline_command(data_dir: &Path, request: &str) -> Result<serde_json::Value, String> {
    let request_size = request.len();
    if request_size > 2 * 1024 * 1024 { return Err("Request exceeds 2 MiB".into()); }
    let request: serde_json::Value = serde_json::from_str(request).map_err(|error| error.to_string())?;
    let method = request.get("method").and_then(|value| value.as_str()).ok_or("Missing method")?;
    if !matches!(method, "save_request" | "send_request") && request_size > 128 * 1024 {
        return Err("Request exceeds 128 KiB".into());
    }
    if method == "send_request" {
        let draft: RequestDraft = serde_json::from_value(request.get("request").cloned()
            .ok_or("Missing request draft")?).map_err(|error| error.to_string())?;
        return replay_request_offline(data_dir, draft);
    }
    let store = offline_store(data_dir)?;
    if matches!(method, "recent" | "metadata" | "body_preview") {
        return history_command(store.as_ref(), method, &request);
    }
    match method {
        "certificate" => {
            let engine = Engine::open(data_dir).map_err(|error| format!("{error:#}"))?;
            std::fs::read_to_string(&engine.ca.cert_path)
                .map(serde_json::Value::String).map_err(|error| error.to_string())
        }
        "certificates" => {
            let certificates = offline_certificates(data_dir)?;
            serde_json::to_value(certificates.snapshot().map_err(|error| format!("{error:#}"))?)
                .map_err(|error| error.to_string())
        }
        "import_certificate" => {
            let input: capture_core::certificates::Import = serde_json::from_value(
                request.get("input").cloned().ok_or("Missing certificate input")?)
                .map_err(|error| error.to_string())?;
            let certificates = offline_certificates(data_dir)?;
            certificates.snapshot().map_err(|error| format!("{error:#}"))?;
            serde_json::to_value(certificates.import(input).map_err(|error| format!("{error:#}"))?)
                .map_err(|error| error.to_string())
        }
        "change_certificate" => {
            let input: capture_core::certificates::Change = serde_json::from_value(
                request.get("input").cloned().ok_or("Missing certificate change")?)
                .map_err(|error| error.to_string())?;
            let certificates = offline_certificates(data_dir)?;
            certificates.snapshot().map_err(|error| format!("{error:#}"))?;
            serde_json::to_value(certificates.change(input).map_err(|error| format!("{error:#}"))?)
                .map_err(|error| error.to_string())
        }
        "blocking" => {
            let blocking = capture_core::blocking::Blocking::open(store.clone())
                .map_err(|error| format!("{error:#}"))?;
            serde_json::to_value(blocking.snapshot()).map_err(|error| error.to_string())
        }
        "configure_blocking" => {
            let config: capture_core::blocking::Config = serde_json::from_value(
                request.get("config").cloned().ok_or("Missing blocking config")?)
                .map_err(|error| error.to_string())?;
            let blocking = capture_core::blocking::Blocking::open(store.clone())
                .map_err(|error| format!("{error:#}"))?;
            blocking.configure(config).map_err(|error| format!("{error:#}"))?;
            serde_json::to_value(blocking.snapshot()).map_err(|error| error.to_string())
        }
        "response_rewrite" => {
            let rewrite = capture_core::response_rewrite::ResponseRewrite::open(store.clone())
                .map_err(|error| format!("{error:#}"))?;
            serde_json::to_value(rewrite.snapshot()).map_err(|error| error.to_string())
        }
        "configure_response_rewrite" => {
            let config: capture_core::response_rewrite::Config = serde_json::from_value(
                request.get("config").cloned().ok_or("Missing response rewrite config")?)
                .map_err(|error| error.to_string())?;
            let expected = request.get("expectedVersion").and_then(|value| value.as_str())
                .ok_or("Missing response rewrite version")?;
            let rewrite = capture_core::response_rewrite::ResponseRewrite::open(store.clone())
                .map_err(|error| format!("{error:#}"))?;
            serde_json::to_value(rewrite.configure(config, expected)
                .map_err(|error| format!("{error:#}"))?).map_err(|error| error.to_string())
        }
        "list_saved_collections" => list_saved_collections_in(store.as_ref()),
        "change_saved_collection" => {
            let action = request.get("action").and_then(|value| value.as_str()).ok_or("Missing collection action")?;
            let id = request.get("id").and_then(|value| value.as_str()).ok_or("Missing collection ID")?;
            change_saved_collection_in(store.as_ref(), action, id,
                request.get("name").and_then(|value| value.as_str()))
        }
        "list_saved_requests" => {
            let offset = request.get("offset").and_then(|value| value.as_u64())
                .and_then(|value| usize::try_from(value).ok()).unwrap_or(0);
            list_saved_requests_in(store.as_ref(), offset,
                request.get("query").and_then(|value| value.as_str()).unwrap_or(""),
                request.get("collectionId").and_then(|value| value.as_str()).unwrap_or(""))
        }
        "save_request" => {
            let draft: RequestDraft = serde_json::from_value(request.get("request").cloned()
                .ok_or("Missing request draft")?).map_err(|error| error.to_string())?;
            save_request_in(store.as_ref(), request.get("id").and_then(|value| value.as_str()),
                request.get("name").and_then(|value| value.as_str()).ok_or("Missing request name")?,
                draft, request.get("collectionId").and_then(|value| value.as_str()),
                request.get("newCollectionName").and_then(|value| value.as_str()))
        }
        "delete_saved_request" => {
            let id = request.get("id").and_then(|value| value.as_str()).ok_or("Missing request ID")?;
            let result = delete_saved_request_in(store.as_ref(), id)?;
            let mut read = OFFLINE_SAVED_READ.get_or_init(|| Mutex::new(None))
                .lock().map_err(|_| "Saved request reader is unavailable")?;
            if read.as_ref().is_some_and(|entry| entry.id == id) { *read = None; }
            Ok(result)
        }
        "open_saved_request" => {
            let id = request.get("id").and_then(|value| value.as_str()).ok_or("Missing request ID")?;
            let mut read = OFFLINE_SAVED_READ.get_or_init(|| Mutex::new(None))
                .lock().map_err(|_| "Saved request reader is unavailable")?;
            open_saved_request_in(store.as_ref(), &mut read, id)
        }
        "saved_request_body" => {
            let token = request.get("readToken").and_then(|value| value.as_u64())
                .ok_or("Missing saved request read token")?;
            let offset = request.get("offset").and_then(|value| value.as_u64())
                .and_then(|value| usize::try_from(value).ok()).ok_or("Invalid saved body offset")?;
            let mut read = OFFLINE_SAVED_READ.get_or_init(|| Mutex::new(None))
                .lock().map_err(|_| "Saved request reader is unavailable")?;
            saved_request_body_in(&mut read, token, offset)
        }
        _ => Err("Offline command is limited to history, saved requests, rules, certificates and manual sending".into()),
    }
}

fn offline_export_session(data_dir: &Path, output: &Path) -> Result<serde_json::Value, String> {
    let store = offline_store(data_dir)?;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true)
        .open(output).map_err(|error| error.to_string())?;
    let result = (|| {
        let count = store.export_session_json(&mut file)
            .map_err(|error| format!("{error:#}"))?;
        file.flush().map_err(|error| error.to_string())?;
        Ok(serde_json::json!({"count": count}))
    })();
    if result.is_err() { let _ = std::fs::remove_file(output); }
    result
}

fn command(request: &str) -> Result<serde_json::Value, String> {
    let request: serde_json::Value = serde_json::from_str(request).map_err(|e| e.to_string())?;
    let method = request
        .get("method")
        .and_then(|v| v.as_str())
        .ok_or("Missing method")?;
    if method == "begin_session_export" { return begin_session_export(); }
    if method == "read_session_export" || method == "finish_session_export" {
        let token = request.get("token").and_then(|value| value.as_u64()).ok_or("Invalid export token")?;
        if method == "finish_session_export" { return finish_session_export(token); }
        let offset = request.get("offset").and_then(|value| value.as_u64()).ok_or("Invalid export offset")?;
        return read_session_export(token, offset);
    }
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
    if method == "save_request" {
        let draft: RequestDraft = serde_json::from_value(request.get("request").cloned()
            .ok_or("Missing request draft")?).map_err(|error| error.to_string())?;
        let name = request.get("name").and_then(|value| value.as_str()).ok_or("Missing request name")?;
        let id = request.get("id").and_then(|value| value.as_str());
        let collection_id = request.get("collectionId").and_then(|value| value.as_str());
        let new_collection_name = request.get("newCollectionName").and_then(|value| value.as_str());
        return save_request(id, name, draft, collection_id, new_collection_name);
    }
    if method == "list_saved_collections" { return list_saved_collections(); }
    if method == "change_saved_collection" {
        let action = request.get("action").and_then(|value| value.as_str()).ok_or("Missing collection action")?;
        let id = request.get("id").and_then(|value| value.as_str()).ok_or("Missing collection ID")?;
        let name = request.get("name").and_then(|value| value.as_str());
        return change_saved_collection(action, id, name);
    }
    if method == "list_saved_requests" {
        let offset = request.get("offset").and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok()).unwrap_or(0);
        let query = request.get("query").and_then(|value| value.as_str()).unwrap_or("");
        let collection_id = request.get("collectionId").and_then(|value| value.as_str()).unwrap_or("");
        return list_saved_requests(offset, query, collection_id);
    }
    if method == "delete_saved_request" {
        let id = request.get("id").and_then(|value| value.as_str()).ok_or("Missing request ID")?;
        return delete_saved_request(id);
    }
    if method == "open_saved_request" {
        let id = request.get("id").and_then(|value| value.as_str()).ok_or("Missing request ID")?;
        return open_saved_request(id);
    }
    if method == "saved_request_body" {
        let token = request.get("readToken").and_then(|value| value.as_u64())
            .ok_or("Missing saved request read token")?;
        let offset = request.get("offset").and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok()).ok_or("Invalid saved body offset")?;
        return saved_request_body(token, offset);
    }
    if method == "begin_request_upload" || method == "begin_saved_request_upload" {
        let expected = request.get("totalBytes").and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok())
            .ok_or("Invalid upload size")?;
        if expected > MAX_UPLOAD_BODY { return Err("Request body exceeds 1 MiB".into()); }
        let draft: RequestDraft = serde_json::from_value(request.get("request").cloned()
            .ok_or("Missing request draft")?).map_err(|e| e.to_string())?;
        if !draft.body_base64.is_empty() { return Err("Upload draft must have an empty body".into()); }
        let action = if method == "begin_saved_request_upload" {
            let name = request.get("name").and_then(|value| value.as_str())
                .ok_or("Missing request name")?.trim();
            if name.is_empty() || name.len() > 512 { return Err("Request name must be 1–512 bytes".into()); }
            UploadAction::Save { id: request.get("id").and_then(|value| value.as_str()).map(str::to_owned),
                name: name.to_owned(),
                collection_id: request.get("collectionId").and_then(|value| value.as_str()).map(str::to_owned),
                new_collection_name: request.get("newCollectionName").and_then(|value| value.as_str()).map(str::to_owned) }
        } else { UploadAction::Replay };
        let token = UPLOAD_IDS.fetch_add(1, Ordering::Relaxed);
        let mut guard = active().lock().map_err(|_| "Capture state is unavailable")?;
        let capture = guard.as_mut().ok_or("Capture is not running")?;
        capture.upload = Some(RequestUpload { token, draft, expected, received: 0, action });
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
    if method == "cancel_request_upload" || method == "commit_request_upload"
        || method == "commit_saved_request_upload" {
        let token = request.get("token").and_then(|value| value.as_u64()).ok_or("Invalid upload token")?;
        let mut guard = active().lock().map_err(|_| "Capture state is unavailable")?;
        let capture = guard.as_mut().ok_or("Capture is not running")?;
        let upload = capture.upload.as_ref().ok_or("No request upload is active")?;
        if upload.token != token { return Err("Upload token does not match".into()); }
        if method != "cancel_request_upload" && upload.received != upload.expected {
            return Err("Request body upload is incomplete".into());
        }
        if (method == "commit_request_upload") != matches!(&upload.action, UploadAction::Replay)
            && method != "cancel_request_upload" {
            return Err("Upload action does not match commit command".into());
        }
        let upload = capture.upload.take().unwrap();
        drop(guard);
        if method == "cancel_request_upload" { return Ok(serde_json::Value::Null); }
        return match upload.action {
            UploadAction::Replay => replay_request(upload.draft),
            UploadAction::Save { id, name, collection_id, new_collection_name } =>
                save_request(id.as_deref(), &name, upload.draft,
                    collection_id.as_deref(), new_collection_name.as_deref()),
        };
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
        "recent" | "metadata" | "body_preview" =>
            history_command(&capture.runtime.engine.store, method, &request),
        "certificate" => std::fs::read_to_string(&capture.runtime.engine.ca.cert_path)
            .map(serde_json::Value::String)
            .map_err(|e| e.to_string()),
        "certificates" => serde_json::to_value(capture.runtime.engine.certificates
            .snapshot().map_err(|e| format!("{e:#}"))?)
            .map_err(|e| e.to_string()),
        "import_certificate" => {
            let input: capture_core::certificates::Import = serde_json::from_value(
                request.get("input").cloned().ok_or("Missing certificate input")?)
                .map_err(|e| e.to_string())?;
            serde_json::to_value(capture.runtime.engine.certificates.import(input)
                .map_err(|e| format!("{e:#}"))?).map_err(|e| e.to_string())
        }
        "change_certificate" => {
            let input: capture_core::certificates::Change = serde_json::from_value(
                request.get("input").cloned().ok_or("Missing certificate change")?)
                .map_err(|e| e.to_string())?;
            serde_json::to_value(capture.runtime.engine.certificates.change(input)
                .map_err(|e| format!("{e:#}"))?).map_err(|e| e.to_string())
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
pub unsafe extern "C" fn hc_ios_offline_command(
    data_dir: *const c_char, request: *const c_char,
) -> *mut c_char {
    reply((|| {
        let data_dir = input_string(data_dir)?;
        let request = input_string(request)?;
        offline_command(Path::new(data_dir), request)
    })())
}

#[no_mangle]
pub unsafe extern "C" fn hc_ios_offline_export_session(
    data_dir: *const c_char, output_path: *const c_char,
) -> *mut c_char {
    reply((|| {
        let data_dir = input_string(data_dir)?;
        let output_path = input_string(output_path)?;
        offline_export_session(Path::new(data_dir), Path::new(output_path))
    })())
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

    #[test]
    fn offline_history_is_read_only_and_keeps_body_revision_valid() {
        let dir = tempfile::tempdir().unwrap();
        assert!(offline_command(dir.path(), r#"{"method":"recent"}"#).is_err());
        assert!(!dir.path().join("sessions.db").exists());
        let store = capture_core::store::Store::open(&dir.path().join("sessions.db")).unwrap();
        let flow = serde_json::from_value(serde_json::json!({
            "id": "offline-ios-flow", "parentId": null, "startedAt": 1,
            "durationMs": 2, "source": "capture", "request": {
                "method": "POST", "url": "https://example.test/offline-ios",
                "headers": [], "bodyBase64": "eHl6"
            },
            "response": null, "error": null, "rawRequestHeadBase64": null, "notes": []
        })).unwrap();
        store.insert(&flow).unwrap();
        drop(store);
        let page = offline_command(dir.path(), r#"{"method":"recent","urlContains":"offline-ios"}"#).unwrap();
        assert_eq!(page["requests"][0]["id"], "offline-ios-flow");
        let detail = offline_command(dir.path(), r#"{"method":"metadata","id":"offline-ios-flow"}"#).unwrap();
        let chunk = offline_command(dir.path(), &serde_json::json!({
            "method": "body_preview", "id": "offline-ios-flow", "variant": "request",
            "revision": detail["bodyRevision"], "offset": 0
        }).to_string()).unwrap();
        assert_eq!(chunk["bodyBase64"], "eHl6");
        assert!(offline_command(dir.path(), r#"{"method":"configure_blocking"}"#).is_err());
        let exported = dir.path().join("session.json");
        let info = offline_export_session(dir.path(), &exported).unwrap();
        assert_eq!(info["count"], 1);
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&exported).unwrap()).unwrap();
        assert!(json.to_string().contains("offline-ios-flow"));
        assert!(json.to_string().contains("eHl6"));
        assert!(offline_export_session(dir.path(), &exported).is_err());
        assert!(exported.exists());
    }

    #[test]
    fn offline_saved_requests_keep_body_and_collections_without_vpn() {
        let dir = tempfile::tempdir().unwrap();
        assert!(offline_command(dir.path(), r#"{"method":"list_saved_collections"}"#).is_err());
        capture_core::store::Store::open(&dir.path().join("sessions.db")).unwrap();
        let bytes = vec![0xa5; 160 * 1024];
        let draft = serde_json::json!({"engine":"auto", "method":"POST",
            "url":"https://example.com/offline", "headers":[], "bodyBase64":STANDARD.encode(&bytes)});
        let saved = offline_command(dir.path(), &serde_json::json!({"method":"save_request",
            "name":"offline request", "newCollectionName":"Offline collection", "request":draft}).to_string()).unwrap();
        let id = saved["id"].as_str().unwrap();
        let collections = offline_command(dir.path(), r#"{"method":"list_saved_collections"}"#).unwrap();
        let collection_id = collections["collections"][0]["id"].as_str().unwrap();
        let listed = offline_command(dir.path(), &serde_json::json!({"method":"list_saved_requests",
            "collectionId":collection_id,"query":"OFFLINE"}).to_string()).unwrap();
        assert_eq!(listed["total"], 1);
        let opened = offline_command(dir.path(), &serde_json::json!({"method":"open_saved_request",
            "id":id}).to_string()).unwrap();
        let token = opened["readToken"].as_u64().unwrap();
        let mut copied = Vec::new();
        while copied.len() < bytes.len() {
            let chunk = offline_command(dir.path(), &serde_json::json!({"method":"saved_request_body",
                "readToken":token,"offset":copied.len()}).to_string()).unwrap();
            copied.extend(STANDARD.decode(chunk["bodyBase64"].as_str().unwrap()).unwrap());
        }
        assert_eq!(copied, bytes);
        offline_command(dir.path(), &serde_json::json!({"method":"change_saved_collection",
            "action":"rename","id":collection_id,"name":"Renamed offline"}).to_string()).unwrap();
        offline_command(dir.path(), &serde_json::json!({"method":"delete_saved_request",
            "id":id}).to_string()).unwrap();
        assert_eq!(offline_command(dir.path(), r#"{"method":"list_saved_requests"}"#).unwrap()["total"], 0);
        offline_command(dir.path(), &serde_json::json!({"method":"change_saved_collection",
            "action":"delete","id":collection_id}).to_string()).unwrap();
    }

    #[test]
    fn offline_manual_send_records_the_upstream_response() {
        let dir = tempfile::tempdir().unwrap();
        capture_core::store::Store::open(&dir.path().join("sessions.db")).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/ios-offline-send", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
            let mut input = [0; 1024];
            let length = socket.read(&mut input).unwrap();
            assert!(String::from_utf8_lossy(&input[..length]).contains("GET /ios-offline-send"));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
        });
        let sent = offline_command(dir.path(), &serde_json::json!({"method":"send_request",
            "request":{"engine":"auto","method":"GET","url":url,
                "headers":[],"bodyBase64":""}}).to_string()).unwrap();
        server.join().unwrap();
        assert_eq!(sent["status"], 200);
        assert!(sent["error"].is_null());
        let page = offline_command(dir.path(), r#"{"method":"recent","urlContains":"ios-offline-send"}"#).unwrap();
        assert_eq!(page["requests"][0]["id"], sent["id"]);
        assert_eq!(page["requests"][0]["status"], 200);
    }

    #[test]
    fn offline_rules_persist_and_reject_stale_rewrite_versions() {
        let dir = tempfile::tempdir().unwrap();
        capture_core::store::Store::open(&dir.path().join("sessions.db")).unwrap();
        let blocking = serde_json::json!({"enabled":true,"scope":"all","rules":[{
            "id":"ios-offline-block","enabled":true,"matchType":"host",
            "pattern":"blocked.example","method":""
        }]});
        let saved = offline_command(dir.path(), &serde_json::json!({
            "method":"configure_blocking","config":blocking}).to_string()).unwrap();
        assert_eq!(saved["rules"][0]["id"], "ios-offline-block");
        let before = offline_command(dir.path(), r#"{"method":"response_rewrite"}"#).unwrap();
        let config = serde_json::json!({"enabled":true,"scope":"all","rules":[{
            "id":"ios-offline-response","name":"offline","enabled":true,
            "matchType":"host","pattern":"example.com","method":"GET",
            "action":"replace","replacement":{"status":201,"headers":[],"bodyBase64":"b2s="}
        }]});
        let update = serde_json::json!({"method":"configure_response_rewrite",
            "expectedVersion":before["version"],"config":config});
        let after = offline_command(dir.path(), &update.to_string()).unwrap();
        assert_eq!(after["config"]["rules"][0]["id"], "ios-offline-response");
        assert!(offline_command(dir.path(), &update.to_string()).is_err());
        let engine = Engine::open(dir.path()).unwrap();
        assert_eq!(engine.blocking.snapshot().rules[0].id, "ios-offline-block");
        assert_eq!(engine.response_rewrite.snapshot().config.rules[0].id, "ios-offline-response");
    }

    #[test]
    fn offline_ca_export_and_upstream_trust_keep_public_material_only() {
        let dir = tempfile::tempdir().unwrap();
        capture_core::store::Store::open(&dir.path().join("sessions.db")).unwrap();
        let ca = offline_command(dir.path(), r#"{"method":"certificate"}"#).unwrap();
        let pem = ca.as_str().unwrap();
        assert!(pem.contains("BEGIN CERTIFICATE"));
        assert!(!pem.contains("PRIVATE KEY"));
        let before = offline_command(dir.path(), r#"{"method":"certificates"}"#).unwrap();
        let imported = offline_command(dir.path(), &serde_json::json!({
            "method":"import_certificate", "input": {
                "name":"iOS offline trust", "kind":"trust", "host":"offline.example",
                "port":443, "certificateBase64":STANDARD.encode(pem.as_bytes()),
                "expectedVersion":before["version"]
            }
        }).to_string()).unwrap();
        let id = imported["entries"][0]["id"].as_str().unwrap();
        let change = serde_json::json!({"method":"change_certificate", "input": {
            "action":"enable", "id":id, "enabled":true,
            "expectedVersion":imported["version"]
        }});
        let enabled = offline_command(dir.path(), &change.to_string()).unwrap();
        assert_eq!(enabled["entries"][0]["enabled"], true);
        assert!(offline_command(dir.path(), &change.to_string()).is_err());
        let engine = Engine::open(dir.path()).unwrap();
        assert_eq!(engine.certificates.snapshot().unwrap().entries[0].id, id);
        assert_eq!(offline_command(dir.path(), r#"{"method":"certificate"}"#).unwrap(), ca);
    }

    #[test]
    fn offline_manual_send_applies_saved_block_and_response_rules() {
        let dir = tempfile::tempdir().unwrap();
        capture_core::store::Store::open(&dir.path().join("sessions.db")).unwrap();
        let initial = offline_command(dir.path(), r#"{"method":"response_rewrite"}"#).unwrap();
        offline_command(dir.path(), &serde_json::json!({
            "method":"configure_response_rewrite", "expectedVersion":initial["version"],
            "config":{"enabled":true,"scope":"replay","rules":[{
                "id":"offline-replay-replace","enabled":true,"matchType":"urlContains",
                "pattern":"/offline-rule","action":"replace",
                "replacement":{"status":201,"headers":[],"bodyBase64":"b2s="}
            }]}
        }).to_string()).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/offline-rule", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
            let mut input = [0; 1024];
            socket.read(&mut input).unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nreal").unwrap();
        });
        let sent = offline_command(dir.path(), &serde_json::json!({"method":"send_request",
            "request":{"engine":"auto","method":"GET","url":url,
                "headers":[],"bodyBase64":""}}).to_string()).unwrap();
        server.join().unwrap();
        assert_eq!(sent["status"], 201);
        let detail = offline_command(dir.path(), &serde_json::json!({"method":"metadata",
            "id":sent["id"]}).to_string()).unwrap();
        let response = offline_command(dir.path(), &serde_json::json!({"method":"body_preview",
            "id":sent["id"],"variant":"response","revision":detail["bodyRevision"],
            "offset":0}).to_string()).unwrap();
        assert_eq!(response["bodyBase64"], "b2s=");

        offline_command(dir.path(), &serde_json::json!({"method":"configure_blocking",
            "config":{"enabled":true,"scope":"replay","rules":[{
                "id":"offline-replay-block","enabled":true,"matchType":"host",
                "pattern":"blocked.example","method":""
            }]}}).to_string()).unwrap();
        let blocked = offline_command(dir.path(), &serde_json::json!({"method":"send_request",
            "request":{"engine":"auto","method":"GET",
                "url":"http://blocked.example/no-connect","headers":[],"bodyBase64":""}})
            .to_string()).unwrap();
        assert_eq!(blocked["status"], 403);
        assert!(blocked["error"].is_null());
    }

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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn packet_device_forwards_tcp_and_returns_ip_packets() {
        packet_device_forwards_tcp(false).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn packet_device_forwards_ipv6_tcp_and_returns_ip_packets() {
        packet_device_forwards_tcp(true).await;
    }

    async fn packet_device_forwards_tcp(ipv6: bool) {
        use etherparse::{PacketBuilder, SlicedPacket, TransportSlice};
        use tokio::net::TcpListener;

        unsafe extern "C" fn output(packet: *const u8, length: usize, context: *mut c_void) {
            let sender = &*(context as *const mpsc::UnboundedSender<Vec<u8>>);
            sender.send(std::slice::from_raw_parts(packet, length).to_vec()).unwrap();
        }

        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (input, incoming) = mpsc::channel(8);
        let (output_sender, mut returned) = mpsc::unbounded_channel::<Vec<u8>>();
        let context = Box::into_raw(Box::new(output_sender)) as usize;
        let device = PacketDevice { incoming, pending: None, callback: output, context };
        let mut args = Args::default();
        args.proxy = ArgProxy::try_from(format!("http://{}", proxy.local_addr().unwrap()).as_str())
            .unwrap();
        args.ipv6_enabled = ipv6;
        let cancel = CancellationToken::new();
        let task = tokio::spawn(tun2proxy::run(device, MTU, args, cancel.clone()));

        let source = [10, 111, 0, 2];
        let destination = [203, 0, 113, 1];
        let source_v6 = "fd00:198:18::2".parse::<std::net::Ipv6Addr>().unwrap().octets();
        let destination_v6 = "2001:db8::1".parse::<std::net::Ipv6Addr>().unwrap().octets();
        let syn = if ipv6 {
            PacketBuilder::ipv6(source_v6, destination_v6, 64)
                .tcp(41003, 443, 42, 65535).syn()
        } else {
            PacketBuilder::ipv4(source, destination, 64)
                .tcp(41003, 443, 42, 65535).syn()
        };
        let mut packet = Vec::with_capacity(syn.size(0));
        syn.write(&mut packet, &[]).unwrap();
        input.send(packet).await.unwrap();

        let server_sequence = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let bytes = returned.recv().await.unwrap();
                let packet = SlicedPacket::from_ip(&bytes).unwrap();
                if let Some(TransportSlice::Tcp(tcp)) = packet.transport {
                    if tcp.syn() { break tcp.sequence_number(); }
                }
            }
        }).await.expect("iOS packet callback did not receive SYN-ACK");
        let ack = if ipv6 {
            PacketBuilder::ipv6(source_v6, destination_v6, 64)
                .tcp(41003, 443, 43, 65535).ack(server_sequence.wrapping_add(1))
        } else {
            PacketBuilder::ipv4(source, destination, 64)
                .tcp(41003, 443, 43, 65535).ack(server_sequence.wrapping_add(1))
        };
        let mut packet = Vec::with_capacity(ack.size(0));
        ack.write(&mut packet, &[]).unwrap();
        input.send(packet).await.unwrap();

        let (mut upstream, _) = tokio::time::timeout(std::time::Duration::from_secs(5), proxy.accept())
            .await.expect("TUN did not connect to proxy").unwrap();
        let mut request = [0_u8; 1024];
        let head = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let size = upstream.read(&mut request).await.unwrap();
                assert!(size > 0 && head.len() + size <= 4096, "invalid CONNECT head");
                head.extend_from_slice(&request[..size]);
            }
            head
        }).await.expect("proxy did not receive CONNECT");
        let expected_connect = if ipv6 {
            b"CONNECT [2001:db8::1]:443 HTTP/1.1\r\n".as_slice()
        } else {
            b"CONNECT 203.0.113.1:443 HTTP/1.1\r\n".as_slice()
        };
        assert!(head.starts_with(expected_connect), "{}", String::from_utf8_lossy(&head));
        upstream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await.unwrap();

        let data = if ipv6 {
            PacketBuilder::ipv6(source_v6, destination_v6, 64)
                .tcp(41003, 443, 43, 65535).psh().ack(server_sequence.wrapping_add(1))
        } else {
            PacketBuilder::ipv4(source, destination, 64)
                .tcp(41003, 443, 43, 65535).psh().ack(server_sequence.wrapping_add(1))
        };
        let mut packet = Vec::with_capacity(data.size(5));
        data.write(&mut packet, b"hello").unwrap();
        input.send(packet).await.unwrap();
        let size = tokio::time::timeout(std::time::Duration::from_secs(5), upstream.read(&mut request))
            .await.expect("proxy did not receive TCP payload").unwrap();
        assert_eq!(&request[..size], b"hello");

        upstream.write_all(b"world").await.unwrap();
        let payload = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let bytes = returned.recv().await.unwrap();
                let packet = SlicedPacket::from_ip(&bytes).unwrap();
                if let Some(TransportSlice::Tcp(tcp)) = packet.transport {
                    if !tcp.payload().is_empty() { break tcp.payload().to_vec(); }
                }
            }
        }).await.expect("iOS packet callback did not receive TCP response");
        assert_eq!(payload, b"world");

        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await.expect("TUN loop did not stop").unwrap().unwrap();
        unsafe { drop(Box::from_raw(context as *mut mpsc::UnboundedSender<Vec<u8>>)); }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn packet_device_forwards_udp_without_http_proxy() {
        use etherparse::{PacketBuilder, SlicedPacket, TransportSlice};
        use tokio::net::{TcpListener, UdpSocket};

        unsafe extern "C" fn output(packet: *const u8, length: usize, context: *mut c_void) {
            let sender = &*(context as *const mpsc::UnboundedSender<Vec<u8>>);
            sender.send(std::slice::from_raw_parts(packet, length).to_vec()).unwrap();
        }

        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (input, incoming) = mpsc::channel(8);
        let (output_sender, mut returned) = mpsc::unbounded_channel::<Vec<u8>>();
        let context = Box::into_raw(Box::new(output_sender)) as usize;
        let device = PacketDevice { incoming, pending: None, callback: output, context };
        let mut args = Args::default();
        args.proxy = ArgProxy::try_from(format!("http://{}", proxy.local_addr().unwrap()).as_str())
            .unwrap();
        args.dns = ArgDns::Virtual;
        args.direct_udp_for_http = true;
        let cancel = CancellationToken::new();
        let task = tokio::spawn(tun2proxy::run(device, MTU, args, cancel.clone()));

        let source = [10, 111, 0, 2];
        let destination = [127, 0, 0, 1];
        let origin_port = origin.local_addr().unwrap().port();
        let request = PacketBuilder::ipv4(source, destination, 64).udp(41004, origin_port);
        let mut packet = Vec::with_capacity(request.size(4));
        request.write(&mut packet, b"ping").unwrap();
        input.send(packet).await.unwrap();

        let mut buffer = [0_u8; 16];
        let (size, sender) = tokio::time::timeout(std::time::Duration::from_secs(5), origin.recv_from(&mut buffer))
            .await.expect("UDP origin did not receive packet").unwrap();
        assert_eq!(&buffer[..size], b"ping");
        origin.send_to(b"pong", sender).await.unwrap();

        let payload = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let bytes = returned.recv().await.unwrap();
                let packet = SlicedPacket::from_ip(&bytes).unwrap();
                if let Some(TransportSlice::Udp(udp)) = packet.transport {
                    break udp.payload().to_vec();
                }
            }
        }).await.expect("iOS packet callback did not receive UDP response");
        assert_eq!(payload, b"pong");
        assert!(tokio::time::timeout(std::time::Duration::from_millis(100), proxy.accept()).await.is_err());

        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await.expect("TUN loop did not stop").unwrap().unwrap();
        unsafe { drop(Box::from_raw(context as *mut mpsc::UnboundedSender<Vec<u8>>)); }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn packet_device_forwards_ipv6_udp() {
        use etherparse::{PacketBuilder, SlicedPacket, TransportSlice};
        use tokio::net::UdpSocket;

        unsafe extern "C" fn output(packet: *const u8, length: usize, context: *mut c_void) {
            let sender = &*(context as *const mpsc::UnboundedSender<Vec<u8>>);
            sender.send(std::slice::from_raw_parts(packet, length).to_vec()).unwrap();
        }

        let origin = UdpSocket::bind("[::1]:0").await.unwrap();
        let (input, incoming) = mpsc::channel(8);
        let (output_sender, mut returned) = mpsc::unbounded_channel::<Vec<u8>>();
        let context = Box::into_raw(Box::new(output_sender)) as usize;
        let device = PacketDevice { incoming, pending: None, callback: output, context };
        let mut args = Args::default();
        args.proxy = ArgProxy::try_from("http://127.0.0.1:1").unwrap();
        args.dns = ArgDns::Virtual;
        args.direct_udp_for_http = true;
        args.ipv6_enabled = true;
        args.virtual_dns_pool_v6 = Some("fd00:198:19::/96".parse().unwrap());
        let cancel = CancellationToken::new();
        let task = tokio::spawn(tun2proxy::run(device, MTU, args, cancel.clone()));

        let mut source = [0_u8; 16];
        source[0] = 0xfd;
        source[2] = 0x01;
        source[3] = 0x98;
        source[5] = 0x18;
        source[15] = 2;
        let mut destination = [0_u8; 16];
        destination[15] = 1;
        let request = PacketBuilder::ipv6(source, destination, 64)
            .udp(41005, origin.local_addr().unwrap().port());
        let mut packet = Vec::with_capacity(request.size(4));
        request.write(&mut packet, b"ping").unwrap();
        input.send(packet).await.unwrap();

        let mut buffer = [0_u8; 16];
        let (size, sender) = tokio::time::timeout(std::time::Duration::from_secs(5), origin.recv_from(&mut buffer))
            .await.expect("IPv6 UDP origin did not receive packet").unwrap();
        assert_eq!(&buffer[..size], b"ping");
        origin.send_to(b"pong", sender).await.unwrap();

        let payload = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let bytes = returned.recv().await.unwrap();
                let packet = SlicedPacket::from_ip(&bytes).unwrap();
                if let Some(TransportSlice::Udp(udp)) = packet.transport {
                    assert_eq!(bytes[0] >> 4, 6);
                    break udp.payload().to_vec();
                }
            }
        }).await.expect("iOS packet callback did not receive IPv6 UDP response");
        assert_eq!(payload, b"pong");

        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await.expect("TUN loop did not stop").unwrap().unwrap();
        unsafe { drop(Box::from_raw(context as *mut mpsc::UnboundedSender<Vec<u8>>)); }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn packet_device_answers_virtual_dns() {
        use etherparse::{PacketBuilder, SlicedPacket, TransportSlice};
        use tokio::net::TcpListener;

        unsafe extern "C" fn output(packet: *const u8, length: usize, context: *mut c_void) {
            let sender = &*(context as *const mpsc::UnboundedSender<Vec<u8>>);
            sender.send(std::slice::from_raw_parts(packet, length).to_vec()).unwrap();
        }

        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (input, incoming) = mpsc::channel(8);
        let (output_sender, mut returned) = mpsc::unbounded_channel::<Vec<u8>>();
        let context = Box::into_raw(Box::new(output_sender)) as usize;
        let device = PacketDevice { incoming, pending: None, callback: output, context };
        let mut args = Args::default();
        args.proxy = ArgProxy::try_from(format!("http://{}", proxy.local_addr().unwrap()).as_str())
            .unwrap();
        args.dns = ArgDns::Virtual;
        let cancel = CancellationToken::new();
        let task = tokio::spawn(tun2proxy::run(device, MTU, args, cancel.clone()));

        let query = [
            0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x07, b'e', b'x', b'a', b'm', b'p', b'l', b'e',
            0x07, b'i', b'n', b'v', b'a', b'l', b'i', b'd', 0x00,
            0x00, 0x01, 0x00, 0x01,
        ];
        let request = PacketBuilder::ipv4([10, 111, 0, 2], [10, 111, 0, 1], 64)
            .udp(41006, 53);
        let mut packet = Vec::with_capacity(request.size(query.len()));
        request.write(&mut packet, &query).unwrap();
        input.send(packet).await.unwrap();

        let response = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let bytes = returned.recv().await.unwrap();
                let packet = SlicedPacket::from_ip(&bytes).unwrap();
                if let Some(TransportSlice::Udp(udp)) = packet.transport {
                    if udp.source_port() == 53 { break udp.payload().to_vec(); }
                }
            }
        }).await.expect("virtual DNS did not return an IP packet");
        assert_eq!(&response[..2], &[0x12, 0x34]);
        assert_ne!(response[2] & 0x80, 0, "DNS response bit missing");
        assert_eq!(&response[6..8], &[0x00, 0x01], "expected one virtual A answer");
        let virtual_ip: [u8; 4] = response[response.len() - 4..].try_into().unwrap();

        let syn = PacketBuilder::ipv4([10, 111, 0, 2], virtual_ip, 64)
            .tcp(41007, 443, 42, 65535).syn();
        let mut packet = Vec::with_capacity(syn.size(0));
        syn.write(&mut packet, &[]).unwrap();
        input.send(packet).await.unwrap();
        let server_sequence = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let bytes = returned.recv().await.unwrap();
                let packet = SlicedPacket::from_ip(&bytes).unwrap();
                if let Some(TransportSlice::Tcp(tcp)) = packet.transport {
                    if tcp.syn() { break tcp.sequence_number(); }
                }
            }
        }).await.expect("virtual IP did not receive SYN-ACK");
        let ack = PacketBuilder::ipv4([10, 111, 0, 2], virtual_ip, 64)
            .tcp(41007, 443, 43, 65535).ack(server_sequence.wrapping_add(1));
        let mut packet = Vec::with_capacity(ack.size(0));
        ack.write(&mut packet, &[]).unwrap();
        input.send(packet).await.unwrap();
        let data = PacketBuilder::ipv4([10, 111, 0, 2], virtual_ip, 64)
            .tcp(41007, 443, 43, 65535).psh().ack(server_sequence.wrapping_add(1));
        let mut packet = Vec::with_capacity(data.size(5));
        data.write(&mut packet, b"hello").unwrap();
        input.send(packet).await.unwrap();

        let (mut upstream, _) = tokio::time::timeout(std::time::Duration::from_secs(5), proxy.accept())
            .await.expect("virtual IP did not connect to proxy").unwrap();
        let mut buffer = [0_u8; 1024];
        let head = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let size = upstream.read(&mut buffer).await.unwrap();
                assert!(size > 0 && head.len() + size <= 4096, "invalid CONNECT head");
                head.extend_from_slice(&buffer[..size]);
            }
            head
        }).await.expect("proxy did not receive virtual DNS CONNECT");
        assert!(head.starts_with(b"CONNECT example.invalid:443 HTTP/1.1\r\n"), "{head:?}");

        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await.expect("TUN loop did not stop").unwrap().unwrap();
        unsafe { drop(Box::from_raw(context as *mut mpsc::UnboundedSender<Vec<u8>>)); }
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
        let certificates = command(r#"{"method":"certificates"}"#).unwrap();
        let public_ca = command(r#"{"method":"certificate"}"#).unwrap();
        let imported = command(&serde_json::json!({"method":"import_certificate","input":{
            "name":"iOS smoke CA","kind":"trust","host":"localhost","port":443,
            "certificateBase64":STANDARD.encode(public_ca.as_str().unwrap()),
            "expectedVersion":certificates["version"]
        }}).to_string()).unwrap();
        let trusted = imported["entries"].as_array().unwrap().iter()
            .find(|entry| entry["name"] == "iOS smoke CA").unwrap();
        assert_eq!(trusted["enabled"], false);
        let id = trusted["id"].as_str().unwrap().to_owned();
        let enabled = command(&serde_json::json!({"method":"change_certificate","input":{
            "action":"enable","id":id,"enabled":true,
            "expectedVersion":imported["version"]
        }}).to_string()).unwrap();
        assert!(enabled["entries"].as_array().unwrap().iter()
            .any(|entry| entry["id"] == id && entry["enabled"] == true));
        command(&serde_json::json!({"method":"change_certificate","input":{
            "action":"delete","id":id,"expectedVersion":enabled["version"]
        }}).to_string()).unwrap();
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
        let exported = command(r#"{"method":"begin_session_export"}"#).unwrap();
        let token = exported["token"].as_u64().unwrap();
        let total = exported["totalBytes"].as_u64().unwrap();
        let mut session_bytes = Vec::new();
        let mut offset = 0;
        while offset < total {
            let chunk = command(&serde_json::json!({"method":"read_session_export",
                "token":token,"offset":offset}).to_string()).unwrap();
            assert_eq!(chunk["offset"], offset);
            let bytes = STANDARD.decode(chunk["chunkBase64"].as_str().unwrap()).unwrap();
            assert!(!bytes.is_empty() && bytes.len() <= SESSION_EXPORT_CHUNK);
            offset += bytes.len() as u64;
            session_bytes.extend_from_slice(&bytes);
        }
        assert_eq!(offset, total);
        let session: serde_json::Value = serde_json::from_slice(&session_bytes).unwrap();
        assert_eq!(session["format"], "http-capture-session");
        assert!(session["flows"].as_array().unwrap().iter().any(|flow|
            flow["id"] == id && flow["response"]["status"] == 201));
        command(&serde_json::json!({"method":"finish_session_export","token":token}).to_string()).unwrap();
        assert!(command(&serde_json::json!({"method":"read_session_export",
            "token":token,"offset":0}).to_string()).is_err());
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

        // Packet Tunnel dispatches app messages on a concurrent queue. A
        // pending replay must not hold the global capture lock and stall a
        // status message until the upstream server replies.
        let slow_origin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let slow_port = slow_origin.local_addr().unwrap().port();
        let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let slow_server = std::thread::spawn(move || {
            let (mut stream, _) = slow_origin.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut request = [0_u8; 1024];
            let count = stream.read(&mut request).unwrap();
            assert!(request[..count].starts_with(b"GET /slow HTTP/1.1"));
            accepted_tx.send(()).unwrap();
            release_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
        });
        let slow_replay = std::thread::spawn(move || command(&serde_json::json!({
            "method":"send_request","request":{
                "engine":"native","method":"GET",
                "url":format!("http://127.0.0.1:{slow_port}/slow"),
                "headers":[],"bodyBase64":""
            }
        }).to_string()));
        accepted_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let status_started = std::time::Instant::now();
        let status_result = command(r#"{"method":"status"}"#);
        let status_elapsed = status_started.elapsed();
        release_tx.send(()).unwrap();
        assert!(status_result.is_ok(), "{status_result:?}");
        assert!(status_elapsed < std::time::Duration::from_secs(4),
            "status waited for replay upstream: {status_elapsed:?}");
        assert_eq!(slow_replay.join().unwrap().unwrap()["status"], 200);
        slow_server.join().unwrap();

        // The packet adapter always sends CONNECT, including for plain HTTP.
        // Verify that the transparent listener returns exact replacement bytes.
        let rewrite = command(r#"{"method":"response_rewrite"}"#).unwrap();
        let replacement = serde_json::json!({"method":"configure_response_rewrite",
            "expectedVersion":rewrite["version"],
            "config":{"enabled":true,"scope":"capture","rules":[{
                "id":"ios-binary-smoke","enabled":true,"matchType":"urlContains",
                "pattern":"/bridge","method":"GET","action":"replace",
                "replacement":{"status":203,"headers":[{"name":"Content-Type",
                    "value":"application/octet-stream"}],"bodyBase64":"AP9B"}
            }]}});
        let rewritten = command(&replacement.to_string()).unwrap();
        assert_eq!(rewritten["config"]["rules"][0]["replacement"]["bodyBase64"], "AP9B");
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
        assert!(head.starts_with("HTTP/1.1 203"), "{head}");
        let mut body = [0_u8; 3];
        bridge.read_exact(&mut body).unwrap();
        assert_eq!(&body, &[0, 255, 65]);
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

        // The extension's transparent listener must also decrypt a real TLS
        // request and apply its HTTP rule after the client trusts its CA.
        let ca = command(r#"{"method":"certificate"}"#).unwrap();
        let ca = ca.as_str().unwrap();
        assert!(ca.contains("BEGIN CERTIFICATE") && !ca.contains("PRIVATE KEY"));
        let der = STANDARD.decode(ca.lines().filter(|line| !line.starts_with("-----"))
            .collect::<String>()).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(der.into()).unwrap();
        let blocked = command(r#"{"method":"configure_blocking","config":{"enabled":true,"scope":"capture","rules":[{"id":"ios-tls-smoke","enabled":true,"matchType":"urlContains","pattern":"/ios-tls-smoke","method":"GET"}]}}"#).unwrap();
        assert_eq!(blocked["rules"][0]["pattern"], "/ios-tls-smoke");
        let unavailable = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let unavailable_address = unavailable.local_addr().unwrap();
        drop(unavailable);
        let mut tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots).with_no_client_auth();
        tls_config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let client = rustls::ClientConnection::new(Arc::new(tls_config),
            rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap()).unwrap();
        let tunnel = tunnel_to(bridge_address, unavailable_address).into_inner();
        tunnel.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
        let mut tls = rustls::StreamOwned::new(client, tunnel);
        tls.write_all(format!("GET /ios-tls-smoke HTTP/1.1\r\nHost: {unavailable_address}\r\nConnection: close\r\n\r\n").as_bytes()).unwrap();
        let mut tls_response = String::new();
        tls.read_to_string(&mut tls_response).unwrap();
        assert!(tls_response.starts_with("HTTP/1.1 403"), "{tls_response}");
        assert!(tls_response.contains("X-HTTP-Capture-Blocked: ios-tls-smoke"));
        let recorded = command(r#"{"method":"recent","urlContains":"/ios-tls-smoke"}"#).unwrap();
        assert_eq!(recorded["requests"][0]["state"], "blocked");
        assert_eq!(recorded["requests"][0]["status"], 403);

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

        let saved_draft = serde_json::json!({"engine":"auto", "method":"POST",
            "url":"https://example.com/ios-saved", "headers":[
                {"name":"X-Order","value":"first"},
                {"name":"x-order","value":"second"},
                {"name":"X-Disabled","value":"omit","disabled":true}],
            "bodyBase64":""});
        let saved_body = vec![0xA5; MAX_UPLOAD_BODY];
        let begun = command(&serde_json::json!({"method":"begin_saved_request_upload",
            "name":"iOS saved smoke","request":saved_draft,
            "totalBytes":saved_body.len(),
            "newCollectionName":"iOS segmented collection"}).to_string()).unwrap();
        let saved_token = begun["token"].as_u64().unwrap();
        assert!(command(&serde_json::json!({"method":"commit_request_upload",
            "token":saved_token}).to_string()).is_err());
        for (index, chunk) in saved_body.chunks(MAX_UPLOAD_CHUNK).enumerate() {
            command(&serde_json::json!({"method":"append_request_upload",
                "token":saved_token,"offset":index*MAX_UPLOAD_CHUNK,
                "bodyBase64":STANDARD.encode(chunk)}).to_string()).unwrap();
        }
        let saved = command(&serde_json::json!({"method":"commit_saved_request_upload",
            "token":saved_token}).to_string()).unwrap();
        let saved_id = saved["id"].as_str().unwrap();
        let saved_collections = command(r#"{"method":"list_saved_collections"}"#).unwrap();
        let saved_collection_id = saved_collections["collections"].as_array().unwrap().iter()
            .find(|entry| entry["name"] == "iOS segmented collection")
            .unwrap()["id"].as_str().unwrap();
        let saved_in_collection = command(&serde_json::json!({"method":"list_saved_requests",
            "offset":0,"collectionId":saved_collection_id}).to_string()).unwrap();
        assert_eq!(saved_in_collection["total"], 1);
        assert_eq!(saved_in_collection["requests"][0]["id"], saved_id);
        let list = command(r#"{"method":"list_saved_requests","offset":0}"#).unwrap();
        assert_eq!(list["requests"][0]["id"], saved_id);
        let filtered = command(r#"{"method":"list_saved_requests","offset":0,"query":"IOS SAVED SMOKE"}"#).unwrap();
        assert_eq!(filtered["total"], 1);
        assert_eq!(filtered["requests"][0]["id"], saved_id);
        let unmatched = command(r#"{"method":"list_saved_requests","offset":0,"query":"no such saved request"}"#).unwrap();
        assert_eq!(unmatched["total"], 0);
        let opened = command(&serde_json::json!({"method":"open_saved_request",
            "id":saved_id}).to_string()).unwrap();
        assert_eq!(opened["request"]["collectionId"], saved_collection_id);
        assert_eq!(opened["totalBytes"], MAX_UPLOAD_BODY);
        assert_eq!(opened["request"]["draft"]["bodyBase64"], "");
        assert_eq!(opened["request"]["draft"]["headers"][1]["value"], "second");
        assert_eq!(opened["request"]["draft"]["headers"][2]["disabled"], true);
        let read_token = opened["readToken"].as_u64().unwrap();
        let mut copied = Vec::new();
        while copied.len() < saved_body.len() {
            let part = command(&serde_json::json!({"method":"saved_request_body",
                "readToken":read_token,"offset":copied.len()}).to_string()).unwrap();
            copied.extend(STANDARD.decode(part["bodyBase64"].as_str().unwrap()).unwrap());
            assert_eq!(part["nextOffset"].as_u64().map(|v| v as usize),
                (copied.len() < saved_body.len()).then_some(copied.len()));
        }
        assert_eq!(copied, saved_body);
        assert!(command(&serde_json::json!({"method":"saved_request_body",
            "readToken":read_token,"offset":1}).to_string()).is_err());
        let updated_draft = serde_json::json!({"engine":"auto", "method":"GET",
            "url":"https://example.com/ios-saved-updated", "headers":[], "bodyBase64":""});
        let updated = command(&serde_json::json!({"method":"save_request",
            "id":saved_id,"name":"iOS saved updated","request":updated_draft}).to_string()).unwrap();
        assert_eq!(updated["id"], saved_id);
        assert_eq!(command(&serde_json::json!({"method":"open_saved_request",
            "id":saved_id}).to_string()).unwrap()["request"]["draft"]["url"],
            "https://example.com/ios-saved-updated");
        assert_eq!(command(&serde_json::json!({"method":"open_saved_request",
            "id":saved_id}).to_string()).unwrap()["request"]["collectionId"], saved_collection_id);
        command(&serde_json::json!({"method":"delete_saved_request","id":saved_id})
            .to_string()).unwrap();
        assert!(command(&serde_json::json!({"method":"open_saved_request",
            "id":saved_id}).to_string()).is_err());
        let collection_draft = serde_json::json!({"engine":"auto", "method":"GET",
            "url":"https://example.com/ios-collection", "headers":[], "bodyBase64":""});
        let collection_saved = command(&serde_json::json!({"method":"save_request",
            "name":"collection request", "request":collection_draft,
            "newCollectionName":"iOS collection smoke"}).to_string()).unwrap();
        let collection_request_id = collection_saved["id"].as_str().unwrap();
        let collections = command(r#"{"method":"list_saved_collections"}"#).unwrap();
        let entries = collections["collections"].as_array().unwrap();
        let collection_id = entries.iter().find(|entry| entry["name"] == "iOS collection smoke")
            .unwrap()["id"].as_str().unwrap();
        let in_collection = command(&serde_json::json!({"method":"list_saved_requests",
            "offset":0,"collectionId":collection_id}).to_string()).unwrap();
        assert_eq!(in_collection["total"], 1);
        assert_eq!(in_collection["requests"][0]["id"], collection_request_id);
        assert!(command(&serde_json::json!({"method":"change_saved_collection",
            "action":"delete","id":collection_id}).to_string()).is_err());
        command(&serde_json::json!({"method":"save_request","id":collection_request_id,
            "name":"collection request","request":collection_draft,
            "collectionId":saved_collection_id}).to_string()).unwrap();
        command(&serde_json::json!({"method":"change_saved_collection",
            "action":"rename","id":collection_id,"name":"iOS collection renamed"}).to_string()).unwrap();
        command(&serde_json::json!({"method":"change_saved_collection",
            "action":"delete","id":collection_id}).to_string()).unwrap();
        assert!(command(&serde_json::json!({"method":"list_saved_requests",
            "collectionId":collection_id}).to_string()).is_err());
        command(&serde_json::json!({"method":"delete_saved_request","id":collection_request_id})
            .to_string()).unwrap();
        let cached = command(&serde_json::json!({"method":"save_request",
            "name":"cached body", "request":{"engine":"auto", "method":"POST",
            "url":"https://example.com/ios-cached", "headers":[], "bodyBase64":"AQ=="}})
            .to_string()).unwrap();
        let cached_id = cached["id"].as_str().unwrap();
        let opened_cached = command(&serde_json::json!({"method":"open_saved_request",
            "id":cached_id}).to_string()).unwrap();
        let cached_token = opened_cached["readToken"].as_u64().unwrap();
        command(&serde_json::json!({"method":"delete_saved_request","id":cached_id})
            .to_string()).unwrap();
        assert!(command(&serde_json::json!({"method":"saved_request_body",
            "readToken":cached_token,"offset":0}).to_string()).is_err());

        // A manual replay may still be waiting on an upstream server when
        // Network Extension asks the tunnel to stop. Teardown must release
        // the listener and executor even with that replay in flight.
        let pending_origin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let pending_port = pending_origin.local_addr().unwrap().port();
        let (pending_tx, pending_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let pending_server = std::thread::spawn(move || {
            let (mut stream, _) = pending_origin.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut request = [0_u8; 1024];
            let count = stream.read(&mut request).unwrap();
            assert!(request[..count].starts_with(b"GET /stop-during-replay HTTP/1.1"));
            pending_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(std::time::Duration::from_secs(8));
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        });
        let pending_replay = std::thread::spawn(move || command(&serde_json::json!({
            "method":"send_request","request":{
                "engine":"native","method":"GET",
                "url":format!("http://127.0.0.1:{pending_port}/stop-during-replay"),
                "headers":[],"bodyBase64":""
            }
        }).to_string()));
        pending_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let stop_started = std::time::Instant::now();
        let stopped = stop();
        let stop_elapsed = stop_started.elapsed();
        release_tx.send(()).unwrap();
        assert!(stopped.is_ok(), "{stopped:?}");
        assert!(stop_elapsed < std::time::Duration::from_secs(7),
            "tunnel stop waited too long for replay: {stop_elapsed:?}");
        let _ = pending_replay.join().unwrap();
        pending_server.join().unwrap();
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
