//! Android-owned entry point for the shared capture engine. JNI only translates
//! arguments and results; HTTP/TLS, persistence and capture rules stay in core.
use capture_core::store::McpRequestFilter;
use capture_core::store::{Collection, SavedRequest};
use capture_core::config_migration::{self, ExportOptions};
use capture_core::{model::RequestDraft, CaptureRuntime, Engine};
use jni::{
    objects::{GlobalRef, JClass, JString, JValue},
    sys::{jint, jlong, jstring},
    JNIEnv,
    JavaVM,
};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

const MAX_MOBILE_BODY: usize = 1024 * 1024;

fn validate_mobile_body(draft: &RequestDraft) -> Result<(), String> {
    if draft.body_base64.len() > MAX_MOBILE_BODY.div_ceil(3) * 4 {
        return Err("Request body exceeds the 1 MiB mobile limit".into());
    }
    let mut decoder = base64::read::DecoderReader::new(
        draft.body_base64.as_bytes(), &base64::engine::general_purpose::STANDARD);
    let mut buffer = [0_u8; 16 * 1024];
    let mut length = 0;
    loop {
        let count = decoder.read(&mut buffer).map_err(|error| format!("Invalid body base64: {error}"))?;
        if count == 0 { break; }
        length += count;
        if length > MAX_MOBILE_BODY {
            return Err("Request body exceeds the 1 MiB mobile limit".into());
        }
    }
    Ok(())
}

#[cfg(target_os = "android")]
use tun2proxy::{ArgDns, ArgProxy, Args, CancellationToken, TcpProxyObserver};

#[cfg(target_os = "android")]
fn lookup_application(vm: &JavaVM, service_class: &GlobalRef,
    source: std::net::SocketAddr, destination: std::net::SocketAddr)
    -> Option<capture_core::model::ApplicationInfo> {
    let mut env = vm.attach_current_thread().ok()?;
    let source_ip = env.new_string(source.ip().to_string()).ok()?;
    let destination_ip = env.new_string(destination.ip().to_string()).ok()?;
    let result = env.call_static_method(
        service_class, "lookupConnectionOwner",
        "(Ljava/lang/String;ILjava/lang/String;I)Ljava/lang/String;",
        &[
            JValue::Object(source_ip.as_ref()), JValue::Int(source.port().into()),
            JValue::Object(destination_ip.as_ref()), JValue::Int(destination.port().into()),
        ],
    ).ok()?.l().ok()?;
    if result.is_null() { return None; }
    let response = JString::from(result);
    let json: String = env.get_string(&response).ok()?.into();
    serde_json::from_str(&json).ok()
}

#[cfg(target_os = "android")]
struct MobileTunnel {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<Result<usize, String>>,
    bridge: Option<capture_core::proxy::ProxyHandle>,
    quic: Option<capture_core::quic_proxy::QuicHandle>,
}

struct MobileCapture {
    runtime: Arc<CaptureRuntime>,
    executor: tokio::runtime::Runtime,
    port: u16,
    #[cfg(target_os = "android")]
    tunnel: Option<MobileTunnel>,
}

static CAPTURE: OnceLock<Mutex<Option<MobileCapture>>> = OnceLock::new();
static OFFLINE_STORE: OnceLock<Mutex<Option<(PathBuf, Arc<capture_core::store::Store>)>>> = OnceLock::new();
static OFFLINE_CERTIFICATES: OnceLock<Mutex<Option<(PathBuf, Arc<capture_core::certificates::Certificates>)>>> = OnceLock::new();

fn capture() -> &'static Mutex<Option<MobileCapture>> {
    CAPTURE.get_or_init(|| Mutex::new(None))
}

fn start(data_dir: &Path, port: u16) -> Result<serde_json::Value, String> {
    if port == 0 {
        return Err("Port must be between 1 and 65535".into());
    }
    let mut current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    if let Some(active) = current.as_ref() {
        if active.port != port {
            return Err("Capture is already running on a different port".into());
        }
        return serde_json::to_value(active.executor.block_on(active.runtime.status()))
            .map_err(|error| error.to_string());
    }
    let executor = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let engine = Engine::open(data_dir).map_err(|error| format!("{error:#}"))?;
    let runtime = CaptureRuntime::new(engine);
    // The phone's own proxy endpoint is loopback-only. A future VPN adapter
    // routes selected device connections here; no unauthenticated LAN API is opened.
    executor.block_on(runtime.start_proxy("127.0.0.1".into(), port))?;
    let status = executor.block_on(runtime.status());
    *current = Some(MobileCapture {
        runtime,
        executor,
        port,
        #[cfg(target_os = "android")]
        tunnel: None,
    });
    serde_json::to_value(status).map_err(|error| error.to_string())
}

fn stop() -> Result<(), String> {
    let mut current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    if let Some(mut capture) = current.take() {
        #[cfg(target_os = "android")]
        let tunnel_result = stop_tunnel_inner(&mut capture);
        let outcome = capture
            .executor
            .block_on(capture.runtime.shutdown_network());
        capture.executor.shutdown_background();
        let certificate_cleanup = clear_offline_certificates();
        outcome?;
        certificate_cleanup?;
        #[cfg(target_os = "android")]
        tunnel_result?;
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn start_tunnel(fd: i32, capture_quic: bool, remote_port: Option<u16>, remote_udp_port: Option<u16>, vm: JavaVM, service_class: GlobalRef) -> Result<(), String> {
    if fd < 0 {
        return Err("Invalid VPN file descriptor".into());
    }
    let mut current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_mut().ok_or("Capture is not running")?;
    if capture.tunnel.is_some() {
        return Err("VPN tunnel is already running".into());
    }
    // Remote mode uses an authenticated, VPN-protected loopback relay. Do not
    // route it through the local MITM bridge or silently send UDP directly.
    if remote_port.is_some() && capture_quic { return Err("Remote QUIC capture is not available".into()); }
    let bridge = if remote_port.is_none() {
        Some(capture.executor.block_on(capture_core::proxy::start_tun_bridge(
            capture.runtime.engine.clone(),
        )).map_err(|error| format!("{error:#}"))?)
    } else { None };
    let proxy = match remote_port {
        Some(port) if port >= 1024 => format!("http://127.0.0.1:{port}"),
        Some(_) => return Err("Invalid protected relay port".into()),
        None => format!("http://{}", bridge.as_ref().unwrap().address),
    };
    let mut args = Args::default();
    args.proxy = match ArgProxy::try_from(proxy.as_str()) {
        Ok(proxy) => proxy,
        Err(error) => {
            if let Some(bridge) = bridge { capture.executor.block_on(bridge.stop()); }
            return Err(format!("Transparent bridge address is invalid: {error}"));
        }
    };
    args.direct_udp_for_http = remote_port.is_none();
    if let Some(port) = remote_udp_port {
        if remote_port.is_none() || port < 1024 { return Err("Invalid remote UDP gateway configuration".into()); }
        args.udpgw_server = Some((std::net::Ipv4Addr::LOCALHOST, port).into());
    }
    if let Some(bridge) = bridge.as_ref() {
        let origin_registry = bridge.origin_registry.as_ref().expect("TUN bridge has origin registry").clone();
        origin_registry.enable();
        args.tcp_proxy_observer = Some(TcpProxyObserver::new(move |source, destination, proxy_peer| {
            let application = lookup_application(&vm, &service_class, source, destination);
            let process_status = if application.is_some() { "identified" } else { "tun-unavailable" };
            origin_registry.register(proxy_peer, capture_core::model::TrafficOrigin {
                entry: "tun".into(), client_address: Some(source.to_string()),
                process: None, application, process_status: process_status.into(),
            });
        }));
    }
    args.ipv6_enabled = true;
    args.virtual_dns_pool_v6 = Some(
        "fd00:198:19::/96"
            .parse()
            .map_err(|error| format!("{error}"))?,
    );
    args.tun_fd = Some(fd);
    // Java's ParcelFileDescriptor owns the fd and closes it after the task exits.
    args.close_fd_on_drop = Some(false);
    args.dns = ArgDns::Virtual;
    let quic = if capture_quic {
        match capture.executor.block_on(capture_core::quic_proxy::start_tun(
            capture.runtime.engine.clone(),
        )) {
            Ok(quic) => {
                args.udp_443_redirect = Some(quic.address);
                Some(quic)
            }
            Err(error) => {
                if let Some(bridge) = bridge { capture.executor.block_on(bridge.stop()); }
                return Err(format!("HTTP/3 capture could not start: {error:#}"));
            }
        }
    } else {
        None
    };
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = capture.executor.spawn(async move {
        tun2proxy::general_run_async(args, 1500, false, task_cancel)
            .await
            .map_err(|error| error.to_string())
    });
    capture.tunnel = Some(MobileTunnel {
        cancel,
        task,
        bridge,
        quic,
    });
    Ok(())
}

#[cfg(target_os = "android")]
fn stop_tunnel_inner(capture: &mut MobileCapture) -> Result<(), String> {
    let Some(mut tunnel) = capture.tunnel.take() else {
        return Ok(());
    };
    tunnel.cancel.cancel();
    let result = capture.executor.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), &mut tunnel.task).await
    });
    let outcome = match result {
        // The previous network may have already failed. Its forwarding error
        // does not mean cleanup failed; a new network can start a fresh task.
        Ok(Ok(Ok(_))) | Ok(Ok(Err(_))) => Ok(()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => {
            tunnel.task.abort();
            // Wait for the aborted outer task to drop its abort-on-drop guard,
            // which in turn cancels the inner TUN reader before Java closes fd.
            let aborted = capture.executor.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(2), &mut tunnel.task).await
            });
            if aborted.is_err() {
                Err("VPN tunnel did not stop after abort".into())
            } else {
                Err("VPN tunnel did not stop within five seconds".into())
            }
        }
    };
    if let Some(bridge) = tunnel.bridge { capture.executor.block_on(bridge.stop()); }
    if let Some(quic) = tunnel.quic.take() {
        capture.executor.block_on(quic.stop());
    }
    outcome
}

#[cfg(target_os = "android")]
fn stop_tunnel() -> Result<(), String> {
    let mut current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_mut().ok_or("Capture is not running")?;
    stop_tunnel_inner(capture)
}

#[cfg(target_os = "android")]
fn tunnel_status() -> Result<serde_json::Value, String> {
    let mut current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_mut().ok_or("Capture is not running")?;
    let Some(tunnel) = capture.tunnel.as_ref() else {
        return Err("VPN tunnel is not running".into());
    };
    if !tunnel.task.is_finished() {
        return Ok(serde_json::json!({"running": true}));
    }
    let mut tunnel = capture.tunnel.take().unwrap();
    let outcome = match capture.executor.block_on(tunnel.task) {
        Ok(Ok(_)) => Err("VPN tunnel exited unexpectedly".into()),
        Ok(Err(error)) => Err(error),
        Err(error) => Err(error.to_string()),
    };
    if let Some(bridge) = tunnel.bridge { capture.executor.block_on(bridge.stop()); }
    if let Some(quic) = tunnel.quic.take() {
        capture.executor.block_on(quic.stop());
    }
    outcome
}

fn session_store(data_dir: &Path) -> Result<Arc<capture_core::store::Store>, String> {
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    if let Some(active) = current.as_ref() {
        Ok(active.runtime.engine.store.clone())
    } else {
        offline_store(data_dir)
    }
}

fn offline_certificates(data_dir: &Path) -> Result<Arc<capture_core::certificates::Certificates>, String> {
    let mut offline = OFFLINE_CERTIFICATES.get_or_init(|| Mutex::new(None))
        .lock().map_err(|_| "Offline certificates are unavailable")?;
    if let Some((opened_path, certificates)) = offline.as_ref() {
        if opened_path == data_dir { return Ok(certificates.clone()); }
    }
    let certificates = capture_core::certificates::Certificates::open(data_dir)
        .map_err(|error| format!("{error:#}"))?;
    *offline = Some((data_dir.to_path_buf(), certificates.clone()));
    Ok(certificates)
}

fn clear_offline_certificates() -> Result<(), String> {
    *OFFLINE_CERTIFICATES.get_or_init(|| Mutex::new(None))
        .lock().map_err(|_| "Offline certificates are unavailable")? = None;
    Ok(())
}

fn offline_store(data_dir: &Path) -> Result<Arc<capture_core::store::Store>, String> {
    let path = data_dir.join("sessions.db");
    let mut offline = OFFLINE_STORE.get_or_init(|| Mutex::new(None))
        .lock().map_err(|_| "Offline history is unavailable")?;
    if let Some((opened_path, store)) = offline.as_ref() {
        if opened_path == &path { return Ok(store.clone()); }
    }
    let store = Arc::new(capture_core::store::Store::open(&path)
        .map_err(|error| format!("{error:#}"))?);
    *offline = Some((path, store.clone()));
    Ok(store)
}

fn recent(data_dir: &Path, cursor: &str, url_contains: &str) -> Result<serde_json::Value, String> {
    let cursor = if cursor.is_empty() {
        None
    } else {
        Some(cursor.parse::<i64>().map_err(|_| "Invalid page cursor")?)
    };
    let store = session_store(data_dir)?;
    serde_json::to_value(
        store.mcp_list_requests(McpRequestFilter {
                cursor,
                url_contains: if url_contains.is_empty() {
                    None
                } else {
                    Some(url_contains.to_owned())
                },
                limit: 100,
                ..Default::default()
            })
            .map_err(|error| format!("{error:#}"))?,
    )
    .map_err(|error| error.to_string())
}

fn export_session(data_dir: &Path, path: &Path) -> Result<serde_json::Value, String> {
    let store = session_store(data_dir)?;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true)
            .open(path).map_err(|error| error.to_string())?;
        let count = store.export_session_json(&mut file).map_err(|error| format!("{error:#}"))?;
        std::io::Write::flush(&mut file).map_err(|error| error.to_string())?;
        Ok::<_, String>(json!({"count": count}))
    })();
    if result.is_err() { let _ = std::fs::remove_file(path); }
    result
}

fn export_configuration(data_dir: &Path, path: &Path, options_json: &str,
    passphrase: &str) -> Result<serde_json::Value, String> {
    let store = session_store(data_dir)?;
    let options: ExportOptions = serde_json::from_str(options_json)
        .map_err(|error| format!("Invalid export options: {error}"))?;
    let exported = config_migration::export_protected(
        &store, BTreeMap::new(), options, env!("CARGO_PKG_VERSION"),
        (!passphrase.is_empty()).then_some(passphrase),
    ).map_err(|error| format!("{error:#}"))?;
    config_migration::save_atomic(path, &exported.document)
        .map_err(|error| format!("{error:#}"))?;
    serde_json::to_value(exported.summary).map_err(|error| error.to_string())
}

fn export_configuration_recovery(data_dir: &Path, path: &Path,
    passphrase: &str) -> Result<serde_json::Value, String> {
    let store = session_store(data_dir)?;
    let exported = config_migration::recovery_protected(
        &store, (!passphrase.is_empty()).then_some(passphrase),
    ).map_err(|error| format!("{error:#}"))?;
    config_migration::save_atomic(path, &exported.document)
        .map_err(|error| format!("{error:#}"))?;
    serde_json::to_value(exported.summary).map_err(|error| error.to_string())
}

fn read_configuration(path: &Path) -> Result<String, String> {
    let length = std::fs::metadata(path).map_err(|error| error.to_string())?.len();
    if length == 0 || length > config_migration::MAX_ENCRYPTED_DOCUMENT_BYTES as u64 {
        return Err("Configuration file must be 1 byte–24 MiB".into());
    }
    std::fs::read_to_string(path).map_err(|error| error.to_string())
}

fn preview_configuration(data_dir: &Path, path: &Path, passphrase: &str)
    -> Result<serde_json::Value, String> {
    let document = read_configuration(path)?;
    let store = session_store(data_dir)?;
    let preview = config_migration::preview_protected(
        &store, &document, (!passphrase.is_empty()).then_some(passphrase),
    ).map_err(|error| format!("{error:#}"))?;
    serde_json::to_value(preview).map_err(|error| error.to_string())
}

fn import_configuration(data_dir: &Path, path: &Path, selected_json: &str,
    expected_revision: &str, passphrase: &str) -> Result<serde_json::Value, String> {
    // Hold the capture lock through commit so a concurrent VPN start cannot
    // reopen the old configuration between the stopped check and the freeze.
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    if current.is_some() {
        return Err("Stop VPN capture before importing configuration".into());
    }
    let selected: Vec<String> = serde_json::from_str(selected_json)
        .map_err(|error| format!("Invalid selected categories: {error}"))?;
    let document = read_configuration(path)?;
    let store = offline_store(data_dir)?;
    let prepared = config_migration::prepare_import_protected(
        &store, &document, &selected, expected_revision, BTreeMap::new(),
        env!("CARGO_PKG_VERSION"), (!passphrase.is_empty()).then_some(passphrase),
    ).map_err(|error| format!("{error:#}"))?;
    let imported = config_migration::commit_import(&store, prepared)
        .map_err(|error| format!("{error:#}"))?;
    drop(current);
    serde_json::to_value(imported).map_err(|error| error.to_string())
}

fn metadata(data_dir: &Path, id: &str) -> Result<serde_json::Value, String> {
    let record = session_store(data_dir)?
        .metadata(id, None)
        .map_err(|error| format!("{error:#}"))?
        .ok_or("Recorded flow not found")?;
    serde_json::to_value(record).map_err(|error| error.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveMobileRequest {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    collection_id: Option<String>,
    #[serde(default)]
    new_collection_name: Option<String>,
    name: String,
    draft: RequestDraft,
}

fn saved_collections(data_dir: &Path) -> Result<serde_json::Value, String> {
    let library = session_store(data_dir)?.library().map_err(|error| error.to_string())?;
    Ok(json!({"collections": library.collections.iter().map(|collection| {
        let count = library.requests.iter().filter(|request| request.collection_id == collection.id).count();
        json!({"id": collection.id, "name": collection.name, "count": count})
    }).collect::<Vec<_>>() }))
}

fn change_saved_collection(data_dir: &Path, raw: &str) -> Result<serde_json::Value, String> {
    if raw.len() > 2048 { return Err("Collection change exceeds 2 KiB".into()); }
    let input: serde_json::Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let action = input.get("action").and_then(|value| value.as_str()).ok_or("Missing collection action")?;
    let id = input.get("id").and_then(|value| value.as_str()).ok_or("Missing collection ID")?;
    let store = session_store(data_dir)?;
    let mut library = store.library().map_err(|error| error.to_string())?;
    let index = library.collections.iter().position(|collection| collection.id == id)
        .ok_or("Collection not found")?;
    match action {
        "rename" => {
            let name = input.get("name").and_then(|value| value.as_str())
                .ok_or("Missing collection name")?.trim();
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
    Ok(json!({"id": id, "revision": revision}))
}

fn saved_requests(data_dir: &Path, offset: usize, query: &str, collection_id: &str) -> Result<serde_json::Value, String> {
    let query = query.trim().to_lowercase();
    if query.len() > 256 { return Err("Saved request search is limited to 256 bytes".into()); }
    let library = session_store(data_dir)?.library().map_err(|error| error.to_string())?;
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
        json!({"id": request.id, "name": request.name, "collectionName": collection_name,
            "method": request.draft.method, "url": request.draft.url})
    }).collect();
    Ok(json!({"revision": library.revision, "total": total,
        "nextOffset": (offset + requests.len() < total).then_some(offset + requests.len()),
        "requests": requests}))
}

fn saved_request(data_dir: &Path, id: &str) -> Result<serde_json::Value, String> {
    let library = session_store(data_dir)?.library().map_err(|error| error.to_string())?;
    let request = library.requests.iter().find(|request| request.id == id)
        .ok_or("Saved request not found")?;
    validate_mobile_body(&request.draft)?;
    serde_json::to_value(request).map_err(|error| error.to_string())
}

fn delete_saved_request(data_dir: &Path, id: &str) -> Result<serde_json::Value, String> {
    let store = session_store(data_dir)?;
    let mut library = store.library().map_err(|error| error.to_string())?;
    let index = library.requests.iter().position(|request| request.id == id)
        .ok_or("Saved request not found")?;
    library.requests.remove(index);
    let revision = store.save_library(library).map_err(|error| error.to_string())?;
    Ok(json!({"id": id, "revision": revision}))
}

fn save_request(data_dir: &Path, raw: &str) -> Result<serde_json::Value, String> {
    if raw.len() > 2 * 1024 * 1024 { return Err("Saved request exceeds 2 MiB".into()); }
    let input: SaveMobileRequest = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let name = input.name.trim();
    if name.is_empty() || name.len() > 512 { return Err("Request name must be 1–512 bytes".into()); }
    validate_mobile_body(&input.draft)?;
    capture_core::http1::validate_draft(&input.draft).map_err(|error| error.to_string())?;
    let store = session_store(data_dir)?;
    let mut library = store.library().map_err(|error| error.to_string())?;
    if input.collection_id.is_some() && input.new_collection_name.is_some() {
        return Err("Choose an existing collection or create a new one".into());
    }
    let selected_collection = if let Some(new_name) = input.new_collection_name {
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
    } else if let Some(id) = input.collection_id {
        if !library.collections.iter().any(|collection| collection.id == id) {
            return Err("Selected collection no longer exists".into());
        }
        Some(id)
    } else {
        None
    };
    if let Some(id) = input.id {
        let request = library.requests.iter_mut().find(|request| request.id == id)
            .ok_or("Saved request not found")?;
        if request.grpc_draft.is_some() || request.request_mode.as_deref() == Some("grpc") {
            return Err("gRPC requests cannot be edited in the mobile HTTP editor".into());
        }
        request.name = name.into();
        request.draft = input.draft;
        if let Some(collection_id) = selected_collection { request.collection_id = collection_id; }
        let revision = store.save_library(library).map_err(|error| error.to_string())?;
        return Ok(json!({"id": id, "revision": revision}));
    }
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
        draft: input.draft, grpc_draft: None, request_mode: Some("http".into()) });
    let revision = store.save_library(library).map_err(|error| error.to_string())?;
    Ok(json!({"id": id, "revision": revision}))
}

fn certificate(data_dir: &Path) -> Result<String, String> {
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let cert_path = if let Some(capture) = current.as_ref() {
        capture.runtime.engine.ca.cert_path.clone()
    } else {
        Engine::open(data_dir).map_err(|error| format!("{error:#}"))?.ca.cert_path.clone()
    };
    std::fs::read_to_string(cert_path).map_err(|error| error.to_string())
}

fn certificates(data_dir: &Path) -> Result<serde_json::Value, String> {
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    let certificates = if let Some(capture) = current.as_ref() {
        capture.runtime.engine.certificates.clone()
    } else {
        offline_certificates(data_dir)?
    };
    serde_json::to_value(certificates.snapshot().map_err(|error| format!("{error:#}"))?)
        .map_err(|error| error.to_string())
}

fn import_certificate(data_dir: &Path, raw: &str) -> Result<serde_json::Value, String> {
    let input: capture_core::certificates::Import = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    let certificates = if let Some(capture) = current.as_ref() {
        capture.runtime.engine.certificates.clone()
    } else {
        offline_certificates(data_dir)?
    };
    certificates.snapshot().map_err(|error| format!("{error:#}"))?;
    let mut snapshot = certificates.import(input).map_err(|error| format!("{error:#}"))?;
    if current.is_none() {
        clear_offline_certificates()?;
        snapshot = offline_certificates(data_dir)?.snapshot()
            .map_err(|error| format!("{error:#}"))?;
    }
    serde_json::to_value(snapshot)
        .map_err(|error| error.to_string())
}

fn change_certificate(data_dir: &Path, raw: &str) -> Result<serde_json::Value, String> {
    let input: capture_core::certificates::Change = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    let certificates = if let Some(capture) = current.as_ref() {
        capture.runtime.engine.certificates.clone()
    } else {
        offline_certificates(data_dir)?
    };
    certificates.snapshot().map_err(|error| format!("{error:#}"))?;
    let mut snapshot = certificates.change(input).map_err(|error| format!("{error:#}"))?;
    if current.is_none() {
        clear_offline_certificates()?;
        snapshot = offline_certificates(data_dir)?.snapshot()
            .map_err(|error| format!("{error:#}"))?;
    }
    serde_json::to_value(snapshot)
        .map_err(|error| error.to_string())
}

fn body_preview(
    data_dir: &Path,
    id: &str,
    variant: &str,
    revision: &str,
    offset: u64,
) -> Result<serde_json::Value, String> {
    let mut chunk = session_store(data_dir)?
        .read_body_at_revision(id, variant, revision, offset, 16 * 1024)
        .map_err(|error| format!("{error:#}"))?;
    chunk.align_utf8_boundary().map_err(|error| error.to_string())?;
    serde_json::to_value(chunk).map_err(|error| error.to_string())
}

fn blocking(data_dir: &Path) -> Result<serde_json::Value, String> {
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let config = if let Some(capture) = current.as_ref() {
        capture.runtime.engine.blocking.snapshot()
    } else {
        capture_core::blocking::Blocking::open(offline_store(data_dir)?)
            .map_err(|error| format!("{error:#}"))?.snapshot()
    };
    serde_json::to_value(config).map_err(|error| error.to_string())
}

fn status() -> Result<serde_json::Value, String> {
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    serde_json::to_value(capture.executor.block_on(capture.runtime.status()))
        .map_err(|error| error.to_string())
}

fn configure_blocking(data_dir: &Path, raw: &str) -> Result<serde_json::Value, String> {
    let config: capture_core::blocking::Config =
        serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let snapshot = if let Some(capture) = current.as_ref() {
        capture.runtime.engine.blocking.configure(config)
            .map_err(|error| format!("{error:#}"))?;
        capture.runtime.engine.blocking.snapshot()
    } else {
        let blocking = capture_core::blocking::Blocking::open(offline_store(data_dir)?)
            .map_err(|error| format!("{error:#}"))?;
        blocking.configure(config).map_err(|error| format!("{error:#}"))?;
        blocking.snapshot()
    };
    serde_json::to_value(snapshot).map_err(|error| error.to_string())
}

fn response_rewrite(data_dir: &Path) -> Result<serde_json::Value, String> {
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let snapshot = if let Some(capture) = current.as_ref() {
        capture.runtime.engine.response_rewrite.snapshot()
    } else {
        capture_core::response_rewrite::ResponseRewrite::open(offline_store(data_dir)?)
            .map_err(|error| format!("{error:#}"))?.snapshot()
    };
    serde_json::to_value(snapshot).map_err(|error| error.to_string())
}

fn configure_response_rewrite(data_dir: &Path, raw: &str) -> Result<serde_json::Value, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Update {
        config: capture_core::response_rewrite::Config,
        expected_version: String,
    }
    let update: Update = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let snapshot = if let Some(capture) = current.as_ref() {
        capture.runtime.engine.response_rewrite
            .configure(update.config, &update.expected_version)
    } else {
        capture_core::response_rewrite::ResponseRewrite::open(offline_store(data_dir)?)
            .map_err(|error| format!("{error:#}"))?
            .configure(update.config, &update.expected_version)
    }.map_err(|error| format!("{error:#}"))?;
    serde_json::to_value(snapshot).map_err(|error| error.to_string())
}

fn send_request(data_dir: &Path, raw: &str) -> Result<serde_json::Value, String> {
    if raw.len() > 2 * 1024 * 1024 { return Err("Request exceeds 2 MiB".into()); }
    let request: RequestDraft = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    validate_mobile_body(&request)?;
    let active = {
        let current = capture()
            .lock()
            .map_err(|_| "Capture state is unavailable")?;
        current.as_ref().map(|capture|
            (capture.runtime.engine.clone(), capture.executor.handle().clone()))
    };
    let flow = if let Some((engine, handle)) = active {
        handle.block_on(engine.replay(request, None, None))
    } else {
        let executor = tokio::runtime::Builder::new_multi_thread()
            .enable_all().build().map_err(|error| error.to_string())?;
        let engine = Engine::open(data_dir).map_err(|error| format!("{error:#}"))?;
        let result = executor.block_on(engine.replay(request, None, None));
        executor.shutdown_background();
        result
    }.map_err(|error| format!("{error:#}"))?;
    Ok(serde_json::json!({
        "id": flow.id,
        "status": flow.response.as_ref().map(|response| response.status),
        "error": flow.error,
    }))
}

fn jni_reply(env: JNIEnv<'_>, result: Result<serde_json::Value, String>) -> jstring {
    let response = match result {
        Ok(value) => serde_json::json!({"ok":true,"value":value}),
        Err(error) => serde_json::json!({"ok":false,"error":error}),
    };
    match env.new_string(response.to_string()) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_start(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
    port: jint,
) -> jstring {
    let result = (|| {
        let path: String = env
            .get_string(&data_dir)
            .map_err(|error| error.to_string())?
            .into();
        let port = u16::try_from(port).map_err(|_| "Port must be between 1 and 65535")?;
        start(Path::new(&path), port)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_stop(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    jni_reply(env, stop().map(|()| serde_json::Value::Null))
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_recent(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
    cursor: JString<'_>,
    url_contains: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let cursor: String = env
            .get_string(&cursor)
            .map_err(|error| error.to_string())?
            .into();
        let url_contains: String = env
            .get_string(&url_contains)
            .map_err(|error| error.to_string())?
            .into();
        recent(Path::new(&data_dir), &cursor, &url_contains)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_exportSession(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>, path: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let path: String = env.get_string(&path).map_err(|error| error.to_string())?.into();
        export_session(Path::new(&data_dir), Path::new(&path))
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_exportConfiguration(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>, path: JString<'_>,
    options: JString<'_>, passphrase: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let path: String = env.get_string(&path).map_err(|error| error.to_string())?.into();
        let options: String = env.get_string(&options).map_err(|error| error.to_string())?.into();
        let passphrase: String = env.get_string(&passphrase).map_err(|error| error.to_string())?.into();
        export_configuration(Path::new(&data_dir), Path::new(&path), &options, &passphrase)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_exportConfigurationRecovery(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>, path: JString<'_>,
    passphrase: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let path: String = env.get_string(&path).map_err(|error| error.to_string())?.into();
        let passphrase: String = env.get_string(&passphrase).map_err(|error| error.to_string())?.into();
        export_configuration_recovery(Path::new(&data_dir), Path::new(&path), &passphrase)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_previewConfiguration(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>, path: JString<'_>,
    passphrase: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let path: String = env.get_string(&path).map_err(|error| error.to_string())?.into();
        let passphrase: String = env.get_string(&passphrase).map_err(|error| error.to_string())?.into();
        preview_configuration(Path::new(&data_dir), Path::new(&path), &passphrase)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_importConfiguration(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>, path: JString<'_>,
    selected: JString<'_>, expected_revision: JString<'_>, passphrase: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let path: String = env.get_string(&path).map_err(|error| error.to_string())?.into();
        let selected: String = env.get_string(&selected).map_err(|error| error.to_string())?.into();
        let expected_revision: String = env.get_string(&expected_revision).map_err(|error| error.to_string())?.into();
        let passphrase: String = env.get_string(&passphrase).map_err(|error| error.to_string())?.into();
        import_configuration(Path::new(&data_dir), Path::new(&path), &selected, &expected_revision, &passphrase)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_metadata(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
    id: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let id: String = env
            .get_string(&id)
            .map_err(|error| error.to_string())?
            .into();
        metadata(Path::new(&data_dir), &id)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_savedCollections(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        saved_collections(Path::new(&data_dir))
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_changeSavedCollection(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>, raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let raw: String = env.get_string(&raw).map_err(|error| error.to_string())?.into();
        change_saved_collection(Path::new(&data_dir), &raw)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_savedRequests(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>, offset: jint, query: JString<'_>, collection_id: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let query: String = env.get_string(&query).map_err(|error| error.to_string())?.into();
        let collection_id: String = env.get_string(&collection_id).map_err(|error| error.to_string())?.into();
        saved_requests(Path::new(&data_dir), offset.max(0) as usize, &query, &collection_id)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_savedRequest(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>, id: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let id: String = env.get_string(&id).map_err(|error| error.to_string())?.into();
        saved_request(Path::new(&data_dir), &id)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_deleteSavedRequest(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>, id: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let id: String = env.get_string(&id).map_err(|error| error.to_string())?.into();
        delete_saved_request(Path::new(&data_dir), &id)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_saveRequest(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>, raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let raw: String = env.get_string(&raw).map_err(|error| error.to_string())?.into();
        save_request(Path::new(&data_dir), &raw)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_certificate(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        certificate(Path::new(&data_dir)).map(serde_json::Value::String)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_certificates(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        certificates(Path::new(&data_dir))
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_importCertificate(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>, raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let raw: String = env.get_string(&raw).map_err(|error| error.to_string())?.into();
        import_certificate(Path::new(&data_dir), &raw)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_changeCertificate(
    mut env: JNIEnv<'_>, _class: JClass<'_>, data_dir: JString<'_>, raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let raw: String = env.get_string(&raw).map_err(|error| error.to_string())?.into();
        change_certificate(Path::new(&data_dir), &raw)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_bodyPreview(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
    id: JString<'_>,
    variant: JString<'_>,
    revision: JString<'_>,
    offset: jlong,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let id: String = env
            .get_string(&id)
            .map_err(|error| error.to_string())?
            .into();
        let variant: String = env
            .get_string(&variant)
            .map_err(|error| error.to_string())?
            .into();
        let revision: String = env
            .get_string(&revision)
            .map_err(|error| error.to_string())?
            .into();
        let offset = u64::try_from(offset).map_err(|_| "Invalid body offset")?;
        body_preview(Path::new(&data_dir), &id, &variant, &revision, offset)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_blocking(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        blocking(Path::new(&data_dir))
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_responseRewrite(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        response_rewrite(Path::new(&data_dir))
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_configureResponseRewrite(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
    raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let raw: String = env.get_string(&raw).map_err(|error| error.to_string())?.into();
        configure_response_rewrite(Path::new(&data_dir), &raw)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_status(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    jni_reply(env, status())
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_configureBlocking(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
    raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let raw: String = env
            .get_string(&raw)
            .map_err(|error| error.to_string())?
            .into();
        configure_blocking(Path::new(&data_dir), &raw)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_sendRequest(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
    raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let data_dir: String = env.get_string(&data_dir).map_err(|error| error.to_string())?.into();
        let raw: String = env
            .get_string(&raw)
            .map_err(|error| error.to_string())?
            .into();
        send_request(Path::new(&data_dir), &raw)
    })();
    jni_reply(env, result)
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_startTunnel(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    fd: jint,
    capture_quic: jni::sys::jboolean,
) -> jstring {
    let result = (|| {
        let vm = env.get_java_vm().map_err(|error| error.to_string())?;
        let class = env.find_class("dev/httpcapture/mobile/CaptureVpnService")
            .map_err(|error| error.to_string())?;
        let service_class = env.new_global_ref(class).map_err(|error| error.to_string())?;
        start_tunnel(fd, capture_quic != 0, None, None, vm, service_class)
    })();
    jni_reply(env, result.map(|()| serde_json::Value::Null))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_startRemoteTunnel(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    fd: jint,
    relay_port: jint,
    udp_gateway_port: jint,
) -> jstring {
    let result = (|| {
        let port = u16::try_from(relay_port).map_err(|_| "Invalid protected relay port".to_string())?;
        let udp_port = u16::try_from(udp_gateway_port).map_err(|_| "Invalid protected UDP gateway port".to_string())?;
        let vm = env.get_java_vm().map_err(|error| error.to_string())?;
        let class = env.find_class("dev/httpcapture/mobile/CaptureVpnService")
            .map_err(|error| error.to_string())?;
        let service_class = env.new_global_ref(class).map_err(|error| error.to_string())?;
        start_tunnel(fd, false, Some(port), Some(udp_port), vm, service_class)
    })();
    jni_reply(env, result.map(|()| serde_json::Value::Null))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_stopTunnel(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    jni_reply(env, stop_tunnel().map(|()| serde_json::Value::Null))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_tunnelStatus(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    jni_reply(env, tunnel_status())
}

#[cfg(test)]
mod tests {
    use super::*;
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn portable_configuration_imports_after_preview_and_freezes_old_store() {
        let _guard = SERIAL.lock().unwrap();
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        configure_blocking(source.path(), &serde_json::json!({
            "enabled": true, "scope": "capture", "rules": [{
                "id": "portable-block", "enabled": true, "matchType": "host",
                "pattern": "portable.example", "method": ""
            }]
        }).to_string()).unwrap();
        let file = source.path().join("config.hccfg");
        let summary = export_configuration(source.path(), &file, "{}", "").unwrap();
        assert!(summary["sections"].as_array().unwrap().iter()
            .any(|section| section["id"] == "blocking"));
        let preview = preview_configuration(target.path(), &file, "").unwrap();
        let revision = preview["expectedRevision"].as_str().unwrap();
        assert!(import_configuration(target.path(), &file, r#"["blocking"]"#, "wrong", "").is_err());
        let result = import_configuration(target.path(), &file, r#"["blocking"]"#, revision, "").unwrap();
        assert_eq!(result["requiresRestart"], true);
        assert!(offline_store(target.path()).unwrap().configuration_ready().is_err());
        let reopened = capture_core::store::Store::open(&target.path().join("sessions.db")).unwrap();
        let blocking: serde_json::Value = reopened.setting("blocking.v1").unwrap().unwrap();
        assert_eq!(blocking["rules"][0]["id"], "portable-block");
        assert_eq!(blocking["enabled"], false);
        let recovery = target.path().join("recovery.hccfg");
        export_configuration_recovery(target.path(), &recovery, "android-recovery-123").unwrap();
        let restored = config_migration::decrypt_document(
            &std::fs::read_to_string(recovery).unwrap(), "android-recovery-123",
        ).unwrap();
        let restored: serde_json::Value = serde_json::from_str(&restored).unwrap();
        assert_eq!(restored["format"], "http-capture-config");
        assert!(!restored.to_string().contains("portable-block"));

        let encrypted = source.path().join("config-encrypted.hccfg");
        export_configuration(source.path(), &encrypted, "{}", "android-secret").unwrap();
        assert!(preview_configuration(source.path(), &encrypted, "wrong-secret").is_err());
        assert!(preview_configuration(source.path(), &encrypted, "android-secret").is_ok());
    }

    #[test]
    fn rejects_invalid_port_without_opening_state() {
        let _guard = SERIAL.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert!(start(dir.path(), 0).is_err());
        assert!(capture().lock().unwrap().is_none());
    }

    #[test]
    fn response_rewrite_roundtrip_rejects_stale_version() {
        let _guard = SERIAL.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let initial = response_rewrite(dir.path()).unwrap();
        let update = serde_json::json!({
            "expectedVersion": initial["version"],
            "config": {"enabled": true, "scope": "capture", "rules": [{
                "id": "mobile-test", "name": "mobile test", "enabled": true,
                "matchType": "urlContains", "pattern": "/mobile-test", "action": "replace",
                "replacement": {"status": 201, "headers": [], "bodyBase64": "b2s="}
            }]}
        });
        let saved = configure_response_rewrite(dir.path(), &update.to_string()).unwrap();
        assert_ne!(saved["version"], initial["version"]);
        assert!(configure_response_rewrite(dir.path(), &update.to_string()).is_err());
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap().local_addr().unwrap().port();
        start(dir.path(), port).unwrap();
        assert_eq!(response_rewrite(dir.path()).unwrap()["config"]["rules"][0]["replacement"]["status"], 201);
        let running_update = serde_json::json!({
            "expectedVersion": saved["version"], "config": {"enabled": false, "rules": []}
        });
        configure_response_rewrite(dir.path(), &running_update.to_string()).unwrap();
        stop().unwrap();
        assert_eq!(response_rewrite(dir.path()).unwrap()["config"]["enabled"], false);
    }

    #[test]
    fn blocking_rules_can_be_edited_offline_and_loaded_on_start() {
        let _guard = SERIAL.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let config = serde_json::json!({
            "enabled": true, "scope": "capture", "rules": [{
                "id": "offline-block", "enabled": true, "matchType": "host",
                "pattern": "blocked.example", "method": ""
            }]
        });
        configure_blocking(dir.path(), &config.to_string()).unwrap();
        assert_eq!(blocking(dir.path()).unwrap()["rules"][0]["id"], "offline-block");
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap().local_addr().unwrap().port();
        start(dir.path(), port).unwrap();
        assert_eq!(blocking(dir.path()).unwrap()["rules"][0]["id"], "offline-block");
        stop().unwrap();
    }

    #[test]
    fn certificate_preparation_works_before_capture_starts() {
        let _guard = SERIAL.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ca = certificate(dir.path()).unwrap();
        assert!(ca.contains("BEGIN CERTIFICATE"));
        assert!(!ca.contains("PRIVATE KEY"));
        let before = certificates(dir.path()).unwrap();
        let imported = import_certificate(dir.path(), &serde_json::json!({
            "name": "offline trust", "kind": "trust", "host": "offline.example",
            "port": 443, "certificateBase64": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD, ca.as_bytes()),
            "expectedVersion": before["version"]
        }).to_string()).unwrap();
        let id = imported["entries"][0]["id"].as_str().unwrap().to_owned();
        let enabled = change_certificate(dir.path(), &serde_json::json!({
            "action": "enable", "id": id, "enabled": true,
            "expectedVersion": imported["version"]
        }).to_string()).unwrap();
        assert_eq!(enabled["entries"][0]["enabled"], true);
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap().local_addr().unwrap().port();
        start(dir.path(), port).unwrap();
        assert_eq!(certificate(dir.path()).unwrap(), ca);
        assert_eq!(certificates(dir.path()).unwrap()["entries"][0]["id"], id);
        stop().unwrap();
        assert_eq!(certificate(dir.path()).unwrap(), ca);
        assert_eq!(certificates(dir.path()).unwrap()["entries"][0]["enabled"], true);
    }

    #[test]
    fn sends_manual_request_and_saves_response_without_capture() {
        use std::io::{Read, Write};
        let _guard = SERIAL.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/offline-send", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
            let mut input = [0; 1024];
            let length = socket.read(&mut input).unwrap();
            assert!(String::from_utf8_lossy(&input[..length]).contains("GET /offline-send"));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
        });
        let request = serde_json::json!({
            "engine": "native", "method": "GET", "url": url,
            "headers": [], "bodyBase64": ""
        });
        let result = send_request(dir.path(), &request.to_string()).unwrap();
        server.join().unwrap();
        assert_eq!(result["status"], 200);
        assert!(result["error"].is_null());
        let page = recent(dir.path(), "", "offline-send").unwrap();
        assert_eq!(page["requests"][0]["id"], result["id"]);
        assert_eq!(page["requests"][0]["status"], 200);
        assert!(capture().lock().unwrap().is_none());
    }

    #[test]
    fn saved_one_megabyte_request_replays_without_changing_body() {
        use std::io::{BufRead, BufReader, Read, Write};
        let _guard = SERIAL.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/large-post", listener.local_addr().unwrap());
        let body = vec![0xA5; 1024 * 1024];
        let expected = body.clone();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            assert!(line.starts_with("POST /large-post HTTP/1.1"));
            let mut length = None;
            loop {
                line.clear();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" { break; }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
            }
            assert_eq!(length, Some(expected.len()));
            let mut received = vec![0; expected.len()];
            reader.read_exact(&mut received).unwrap();
            assert_eq!(received, expected);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
        });
        let raw = json!({"name":"large body", "draft": {
            "engine":"native", "method":"POST", "url":url, "headers":[],
            "bodyBase64":base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &body)
        }}).to_string();
        let id = save_request(dir.path(), &raw).unwrap()["id"].as_str().unwrap().to_owned();
        let saved = saved_request(dir.path(), &id).unwrap();
        assert_eq!(saved["draft"]["bodyBase64"].as_str().unwrap().len(), body.len().div_ceil(3) * 4);
        let result = send_request(dir.path(), &saved["draft"].to_string()).unwrap();
        server.join().unwrap();
        assert_eq!(result["status"], 200);
        assert!(result["error"].is_null());
        let mut oversized_draft = saved["draft"].clone();
        oversized_draft["bodyBase64"] = json!(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD, vec![0_u8; MAX_MOBILE_BODY + 1]));
        assert!(send_request(dir.path(), &oversized_draft.to_string())
            .unwrap_err().contains("1 MiB"));
        assert!(save_request(dir.path(), &json!({"name":"oversized",
            "draft":oversized_draft}).to_string()).unwrap_err().contains("1 MiB"));
        let store = session_store(dir.path()).unwrap();
        let mut library = store.library().unwrap();
        library.requests.iter_mut().find(|request| request.id == id).unwrap()
            .draft.body_base64 = oversized_draft["bodyBase64"].as_str().unwrap().to_owned().into();
        store.save_library(library).unwrap();
        assert!(saved_request(dir.path(), &id).unwrap_err().contains("1 MiB"));
    }

    #[test]
    fn exports_saved_session_after_capture_stops() {
        let _guard = SERIAL.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = capture_core::store::Store::open(&dir.path().join("sessions.db")).unwrap();
        let flow = serde_json::from_value(serde_json::json!({
            "id": "offline-flow", "parentId": null, "startedAt": 1,
            "durationMs": 2, "source": "capture", "request": {
                "method": "GET", "url": "https://example.test/offline",
                "headers": [], "bodyBase64": "eHl6"
            },
            "response": null, "error": null, "rawRequestHeadBase64": null, "notes": []
        })).unwrap();
        store.insert(&flow).unwrap();
        drop(store);

        let page = recent(dir.path(), "", "offline").unwrap();
        assert_eq!(page["requests"][0]["id"], "offline-flow");
        let detail = metadata(dir.path(), "offline-flow").unwrap();
        assert_eq!(detail["id"], "offline-flow");
        let chunk = body_preview(dir.path(), "offline-flow", "request",
            detail["bodyRevision"].as_str().unwrap(), 0).unwrap();
        assert_eq!(chunk["bodyBase64"], "eHl6");

        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap().local_addr().unwrap().port();
        start(dir.path(), port).unwrap();
        let running_detail = metadata(dir.path(), "offline-flow").unwrap();
        assert_eq!(body_preview(dir.path(), "offline-flow", "request",
            running_detail["bodyRevision"].as_str().unwrap(), 0).unwrap()["bodyBase64"], "eHl6");
        stop().unwrap();
        let stopped_detail = metadata(dir.path(), "offline-flow").unwrap();
        assert_eq!(body_preview(dir.path(), "offline-flow", "request",
            stopped_detail["bodyRevision"].as_str().unwrap(), 0).unwrap()["bodyBase64"], "eHl6");

        let destination = dir.path().join("saved-session.json");
        let result = export_session(dir.path(), &destination).unwrap();
        assert_eq!(result["count"], 1);
        let session: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&destination).unwrap()).unwrap();
        assert_eq!(session["format"], "http-capture-session");
        assert_eq!(session["flows"][0]["id"], "offline-flow");
    }

    #[test]
    fn saved_request_library_works_without_capture() {
        let _guard = SERIAL.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let raw = json!({"name": "offline request", "draft": {
            "method": "GET", "url": "https://example.test/offline",
            "headers": [], "bodyBase64": ""
        }}).to_string();
        let created = save_request(dir.path(), &raw).unwrap();
        let id = created["id"].as_str().unwrap();
        assert_eq!(saved_requests(dir.path(), 0, "OFFLINE", "").unwrap()["total"], 1);
        assert_eq!(saved_request(dir.path(), id).unwrap()["draft"]["url"],
            "https://example.test/offline");
        let collections = saved_collections(dir.path()).unwrap();
        assert_eq!(collections["collections"][0]["count"], 1);
        let collection_id = collections["collections"][0]["id"].as_str().unwrap().to_owned();

        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap().local_addr().unwrap().port();
        start(dir.path(), port).unwrap();
        assert_eq!(saved_requests(dir.path(), 0, "offline", "").unwrap()["total"], 1);
        stop().unwrap();
        assert_eq!(delete_saved_request(dir.path(), id).unwrap()["id"], id);
        assert_eq!(saved_requests(dir.path(), 0, "offline", "").unwrap()["total"], 0);
        change_saved_collection(dir.path(), &json!({
            "action": "rename", "id": collection_id, "name": "offline collection"
        }).to_string()).unwrap();
        assert_eq!(saved_collections(dir.path()).unwrap()["collections"][0]["name"],
            "offline collection");
        change_saved_collection(dir.path(), &json!({
            "action": "delete", "id": collection_id
        }).to_string()).unwrap();
        assert!(saved_collections(dir.path()).unwrap()["collections"].as_array().unwrap().is_empty());
    }
}
