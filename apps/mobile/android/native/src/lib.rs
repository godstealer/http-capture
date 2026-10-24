//! Android-owned entry point for the shared capture engine. JNI only translates
//! arguments and results; HTTP/TLS, persistence and capture rules stay in core.
use capture_core::store::McpRequestFilter;
use capture_core::store::{Collection, SavedRequest};
use capture_core::{model::RequestDraft, CaptureRuntime, Engine};
use jni::{
    objects::{GlobalRef, JClass, JString, JValue},
    sys::{jint, jlong, jstring},
    JNIEnv,
    JavaVM,
};
use std::{
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

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
    bridge: capture_core::proxy::ProxyHandle,
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
        outcome?;
        #[cfg(target_os = "android")]
        tunnel_result?;
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn start_tunnel(fd: i32, capture_quic: bool, vm: JavaVM, service_class: GlobalRef) -> Result<(), String> {
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
    // TUN TCP streams enter the proxy through CONNECT even for plain HTTP.
    // The regular listener expects TLS after CONNECT; its transparent bridge
    // also accepts an HTTP request on the established tunnel.
    let bridge = capture
        .executor
        .block_on(capture_core::proxy::start_tun_bridge(
            capture.runtime.engine.clone(),
        ))
        .map_err(|error| format!("{error:#}"))?;
    let proxy = format!("http://{}", bridge.address);
    let origin_registry = bridge.origin_registry.as_ref().expect("TUN bridge has origin registry").clone();
    origin_registry.enable();
    let mut args = Args::default();
    args.proxy = match ArgProxy::try_from(proxy.as_str()) {
        Ok(proxy) => proxy,
        Err(error) => {
            capture.executor.block_on(bridge.stop());
            return Err(format!("Transparent bridge address is invalid: {error}"));
        }
    };
    args.direct_udp_for_http = true;
    args.tcp_proxy_observer = Some(TcpProxyObserver::new(move |source, destination, proxy_peer| {
        let application = lookup_application(&vm, &service_class, source, destination);
        let process_status = if application.is_some() { "identified" } else { "tun-unavailable" };
        origin_registry.register(proxy_peer, capture_core::model::TrafficOrigin {
            entry: "tun".into(), client_address: Some(source.to_string()),
            process: None, application, process_status: process_status.into(),
        });
    }));
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
                capture.executor.block_on(bridge.stop());
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
        Ok(Ok(Ok(_))) => Ok(()),
        Ok(Ok(Err(error))) => Err(error),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => {
            tunnel.task.abort();
            Err("VPN tunnel did not stop within five seconds".into())
        }
    };
    capture.executor.block_on(tunnel.bridge.stop());
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
    let tunnel = capture.tunnel.take().unwrap();
    let outcome = match capture.executor.block_on(tunnel.task) {
        Ok(Ok(_)) => Err("VPN tunnel exited unexpectedly".into()),
        Ok(Err(error)) => Err(error),
        Err(error) => Err(error.to_string()),
    };
    capture.executor.block_on(tunnel.bridge.stop());
    outcome
}

fn recent(cursor: &str, url_contains: &str) -> Result<serde_json::Value, String> {
    let cursor = if cursor.is_empty() {
        None
    } else {
        Some(cursor.parse::<i64>().map_err(|_| "Invalid page cursor")?)
    };
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    serde_json::to_value(
        capture
            .runtime
            .engine
            .store
            .mcp_list_requests(McpRequestFilter {
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

fn metadata(id: &str) -> Result<serde_json::Value, String> {
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    let record = capture
        .runtime
        .engine
        .store
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

fn saved_collections() -> Result<serde_json::Value, String> {
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    let library = capture.runtime.engine.store.library().map_err(|error| error.to_string())?;
    Ok(json!({"collections": library.collections.iter().map(|collection| {
        let count = library.requests.iter().filter(|request| request.collection_id == collection.id).count();
        json!({"id": collection.id, "name": collection.name, "count": count})
    }).collect::<Vec<_>>() }))
}

fn change_saved_collection(raw: &str) -> Result<serde_json::Value, String> {
    if raw.len() > 2048 { return Err("Collection change exceeds 2 KiB".into()); }
    let input: serde_json::Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let action = input.get("action").and_then(|value| value.as_str()).ok_or("Missing collection action")?;
    let id = input.get("id").and_then(|value| value.as_str()).ok_or("Missing collection ID")?;
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    let store = &capture.runtime.engine.store;
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

fn saved_requests(offset: usize, query: &str, collection_id: &str) -> Result<serde_json::Value, String> {
    let query = query.trim().to_lowercase();
    if query.len() > 256 { return Err("Saved request search is limited to 256 bytes".into()); }
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    let library = capture.runtime.engine.store.library().map_err(|error| error.to_string())?;
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

fn saved_request(id: &str) -> Result<serde_json::Value, String> {
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    let library = capture.runtime.engine.store.library().map_err(|error| error.to_string())?;
    let request = library.requests.iter().find(|request| request.id == id)
        .ok_or("Saved request not found")?;
    if request.draft.body_base64.len() > (1024 * 1024 + 2) / 3 * 4 {
        return Err("Saved request body exceeds the mobile editor limit".into());
    }
    serde_json::to_value(request).map_err(|error| error.to_string())
}

fn delete_saved_request(id: &str) -> Result<serde_json::Value, String> {
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    let store = &capture.runtime.engine.store;
    let mut library = store.library().map_err(|error| error.to_string())?;
    let index = library.requests.iter().position(|request| request.id == id)
        .ok_or("Saved request not found")?;
    library.requests.remove(index);
    let revision = store.save_library(library).map_err(|error| error.to_string())?;
    Ok(json!({"id": id, "revision": revision}))
}

fn save_request(raw: &str) -> Result<serde_json::Value, String> {
    if raw.len() > 2 * 1024 * 1024 { return Err("Saved request exceeds 2 MiB".into()); }
    let input: SaveMobileRequest = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let name = input.name.trim();
    if name.is_empty() || name.len() > 512 { return Err("Request name must be 1–512 bytes".into()); }
    if input.draft.body_base64.len() > (1024 * 1024 + 2) / 3 * 4 {
        return Err("Request body exceeds the 1 MiB mobile limit".into());
    }
    capture_core::http1::validate_draft(&input.draft).map_err(|error| error.to_string())?;
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    let store = &capture.runtime.engine.store;
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

fn certificate() -> Result<String, String> {
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    std::fs::read_to_string(&capture.runtime.engine.ca.cert_path).map_err(|error| error.to_string())
}

fn certificates() -> Result<serde_json::Value, String> {
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    serde_json::to_value(capture.runtime.engine.certificates.snapshot().map_err(|error| format!("{error:#}"))?)
        .map_err(|error| error.to_string())
}

fn import_certificate(raw: &str) -> Result<serde_json::Value, String> {
    let input: capture_core::certificates::Import = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    serde_json::to_value(capture.runtime.engine.certificates.import(input).map_err(|error| format!("{error:#}"))?)
        .map_err(|error| error.to_string())
}

fn change_certificate(raw: &str) -> Result<serde_json::Value, String> {
    let input: capture_core::certificates::Change = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let current = capture().lock().map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    serde_json::to_value(capture.runtime.engine.certificates.change(input).map_err(|error| format!("{error:#}"))?)
        .map_err(|error| error.to_string())
}

fn body_preview(
    id: &str,
    variant: &str,
    revision: &str,
    offset: u64,
) -> Result<serde_json::Value, String> {
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    let mut chunk = capture
        .runtime
        .engine
        .store
        .read_body_at_revision(id, variant, revision, offset, 16 * 1024)
        .map_err(|error| format!("{error:#}"))?;
    chunk.align_utf8_boundary().map_err(|error| error.to_string())?;
    serde_json::to_value(chunk).map_err(|error| error.to_string())
}

fn blocking() -> Result<serde_json::Value, String> {
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    serde_json::to_value(capture.runtime.engine.blocking.snapshot())
        .map_err(|error| error.to_string())
}

fn status() -> Result<serde_json::Value, String> {
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    serde_json::to_value(capture.executor.block_on(capture.runtime.status()))
        .map_err(|error| error.to_string())
}

fn configure_blocking(raw: &str) -> Result<serde_json::Value, String> {
    let config: capture_core::blocking::Config =
        serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    capture
        .runtime
        .engine
        .blocking
        .configure(config)
        .map_err(|error| format!("{error:#}"))?;
    serde_json::to_value(capture.runtime.engine.blocking.snapshot())
        .map_err(|error| error.to_string())
}

fn response_rewrite() -> Result<serde_json::Value, String> {
    let current = capture()
        .lock()
        .map_err(|_| "Capture state is unavailable")?;
    let capture = current.as_ref().ok_or("Capture is not running")?;
    serde_json::to_value(capture.runtime.engine.response_rewrite.snapshot())
        .map_err(|error| error.to_string())
}

fn configure_response_rewrite(raw: &str) -> Result<serde_json::Value, String> {
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
    let capture = current.as_ref().ok_or("Capture is not running")?;
    serde_json::to_value(
        capture.runtime.engine.response_rewrite
            .configure(update.config, &update.expected_version)
            .map_err(|error| format!("{error:#}"))?,
    )
    .map_err(|error| error.to_string())
}

fn send_request(raw: &str) -> Result<serde_json::Value, String> {
    let request: RequestDraft = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let (runtime, handle) = {
        let current = capture()
            .lock()
            .map_err(|_| "Capture state is unavailable")?;
        let capture = current.as_ref().ok_or("Capture is not running")?;
        (capture.runtime.clone(), capture.executor.handle().clone())
    };
    let flow = handle
        .block_on(runtime.engine.replay(request, None, None))
        .map_err(|error| format!("{error:#}"))?;
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
    cursor: JString<'_>,
    url_contains: JString<'_>,
) -> jstring {
    let result = (|| {
        let cursor: String = env
            .get_string(&cursor)
            .map_err(|error| error.to_string())?
            .into();
        let url_contains: String = env
            .get_string(&url_contains)
            .map_err(|error| error.to_string())?
            .into();
        recent(&cursor, &url_contains)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_metadata(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    id: JString<'_>,
) -> jstring {
    let result = (|| {
        let id: String = env
            .get_string(&id)
            .map_err(|error| error.to_string())?
            .into();
        metadata(&id)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_savedCollections(
    env: JNIEnv<'_>, _class: JClass<'_>,
) -> jstring {
    jni_reply(env, saved_collections())
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_changeSavedCollection(
    mut env: JNIEnv<'_>, _class: JClass<'_>, raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let raw: String = env.get_string(&raw).map_err(|error| error.to_string())?.into();
        change_saved_collection(&raw)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_savedRequests(
    mut env: JNIEnv<'_>, _class: JClass<'_>, offset: jint, query: JString<'_>, collection_id: JString<'_>,
) -> jstring {
    let result = (|| {
        let query: String = env.get_string(&query).map_err(|error| error.to_string())?.into();
        let collection_id: String = env.get_string(&collection_id).map_err(|error| error.to_string())?.into();
        saved_requests(offset.max(0) as usize, &query, &collection_id)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_savedRequest(
    mut env: JNIEnv<'_>, _class: JClass<'_>, id: JString<'_>,
) -> jstring {
    let result = (|| {
        let id: String = env.get_string(&id).map_err(|error| error.to_string())?.into();
        saved_request(&id)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_deleteSavedRequest(
    mut env: JNIEnv<'_>, _class: JClass<'_>, id: JString<'_>,
) -> jstring {
    let result = (|| {
        let id: String = env.get_string(&id).map_err(|error| error.to_string())?.into();
        delete_saved_request(&id)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_saveRequest(
    mut env: JNIEnv<'_>, _class: JClass<'_>, raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let raw: String = env.get_string(&raw).map_err(|error| error.to_string())?.into();
        save_request(&raw)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_certificate(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    jni_reply(env, certificate().map(serde_json::Value::String))
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_certificates(
    env: JNIEnv<'_>, _class: JClass<'_>,
) -> jstring {
    jni_reply(env, certificates())
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_importCertificate(
    mut env: JNIEnv<'_>, _class: JClass<'_>, raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let raw: String = env.get_string(&raw).map_err(|error| error.to_string())?.into();
        import_certificate(&raw)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_changeCertificate(
    mut env: JNIEnv<'_>, _class: JClass<'_>, raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let raw: String = env.get_string(&raw).map_err(|error| error.to_string())?.into();
        change_certificate(&raw)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_bodyPreview(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    id: JString<'_>,
    variant: JString<'_>,
    revision: JString<'_>,
    offset: jlong,
) -> jstring {
    let result = (|| {
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
        body_preview(&id, &variant, &revision, offset)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_blocking(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    jni_reply(env, blocking())
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_responseRewrite(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    jni_reply(env, response_rewrite())
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_configureResponseRewrite(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let raw: String = env.get_string(&raw).map_err(|error| error.to_string())?.into();
        configure_response_rewrite(&raw)
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
    raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let raw: String = env
            .get_string(&raw)
            .map_err(|error| error.to_string())?
            .into();
        configure_blocking(&raw)
    })();
    jni_reply(env, result)
}

#[no_mangle]
pub extern "system" fn Java_dev_httpcapture_mobile_NativeCapture_sendRequest(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    raw: JString<'_>,
) -> jstring {
    let result = (|| {
        let raw: String = env
            .get_string(&raw)
            .map_err(|error| error.to_string())?
            .into();
        send_request(&raw)
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
        start_tunnel(fd, capture_quic != 0, vm, service_class)
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
    #[test]
    fn rejects_invalid_port_without_opening_state() {
        let dir = tempfile::tempdir().unwrap();
        assert!(start(dir.path(), 0).is_err());
        assert!(capture().lock().unwrap().is_none());
    }

    #[test]
    fn response_rewrite_roundtrip_rejects_stale_version() {
        let dir = tempfile::tempdir().unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap().local_addr().unwrap().port();
        start(dir.path(), port).unwrap();
        let initial = response_rewrite().unwrap();
        let update = serde_json::json!({
            "expectedVersion": initial["version"],
            "config": {"enabled": true, "scope": "capture", "rules": [{
                "id": "mobile-test", "name": "mobile test", "enabled": true,
                "matchType": "urlContains", "pattern": "/mobile-test", "action": "replace",
                "replacement": {"status": 201, "headers": [], "bodyBase64": "b2s="}
            }]}
        });
        let saved = configure_response_rewrite(&update.to_string()).unwrap();
        assert_ne!(saved["version"], initial["version"]);
        assert_eq!(response_rewrite().unwrap()["config"]["rules"][0]["replacement"]["status"], 201);
        assert!(configure_response_rewrite(&update.to_string()).is_err());
        stop().unwrap();
    }
}
