#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app_update;
mod mcp_commands;
mod config_migration_commands;
mod response_rewrite_commands;
mod body_commands;
mod certificate_commands;
mod session_export;
use capture_core::{model::{Flow, ProxyStatus, RequestDraft}, CaptureRuntime, Engine};
use capture_mcp::{server::ServerManager, client::ClientManager};
use std::{net::Ipv4Addr, sync::{Arc, atomic::Ordering}};
use tauri::{Emitter, Manager, State};

#[tauri::command]
fn grpc_poll(state:State<'_,AppState>,id:String)->Result<Option<Flow>,String>{state.engine.store.get(&id).map_err(|e|e.to_string())}
#[tauri::command]
async fn grpc_start(state:State<'_,AppState>,request:RequestDraft,half_close:bool)->Result<String,String>{let _lifecycle=state.network_lifecycle.lock().await;state.ensure_network_ready()?;capture_core::grpc_client::start(state.engine.clone(),request,half_close).await.map_err(|e|e.to_string())}
#[tauri::command]
async fn grpc_send(state:State<'_,AppState>,id:String,body:String,end:bool)->Result<(),String>{capture_core::grpc_client::send(&state.engine,&id,&body,end).await.map_err(|e|e.to_string())}
#[tauri::command]
fn system_proxy_status(state:State<'_,AppState>)->capture_core::system_proxy::Status {state.engine.system_proxy.status()}
#[tauri::command]
async fn set_system_proxy(state:State<'_,AppState>,enabled:bool)->Result<(),String>{
    let _lifecycle=state.network_lifecycle.lock().await;
    if enabled { state.ensure_network_ready()?; }
    let proxy=state.proxy.lock().await;
    let address=if enabled{Some(proxy.as_ref().ok_or("Start capture before enabling system proxy")?.address)}else{None};
    let engine=state.engine.clone();
    tauri::async_runtime::spawn_blocking(move||match address{Some(a)=>engine.system_proxy.enable(a),None=>engine.system_proxy.restore()}).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())
}
#[tauri::command]
async fn ca_status(state:State<'_,AppState>)->Result<capture_core::ca_management::Status,String>{let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||capture_core::ca_management::status(&engine.ca.cert_path)).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())}
#[tauri::command]
async fn set_ca_trust(state:State<'_,AppState>,install:bool,sha256:String)->Result<capture_core::ca_management::Status,String>{let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||capture_core::ca_management::change(&engine.ca.cert_path,install,&sha256)).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())}

#[tauri::command]
async fn websocket_connect(state:State<'_,AppState>,request:RequestDraft)->Result<String,String>{let _lifecycle=state.network_lifecycle.lock().await;state.ensure_network_ready()?;capture_core::websocket_client::connect(state.engine.clone(),request).map_err(|e|e.to_string())}
#[tauri::command]
async fn websocket_send(state:State<'_,AppState>,id:String,opcode:u8,body:String)->Result<(),String>{capture_core::websocket_client::send(&state.engine,&id,opcode,&body).await.map_err(|e|e.to_string())}
#[tauri::command]
fn decryption_hosts(state:State<'_,AppState>)->Result<Vec<String>,String>{Ok(state.engine.store.setting("decryption.bypass.v1").map_err(|e|e.to_string())?.unwrap_or_default())}
#[tauri::command]
fn save_decryption_hosts(state:State<'_,AppState>,hosts:Vec<String>)->Result<(),String>{capture_core::decryption::save(&state.engine.store,hosts).map_err(|e|e.to_string())}
#[tauri::command]
async fn test_upstream(state:State<'_,AppState>,id:String)->Result<u64,String>{capture_core::upstream::test_profile(&state.engine.upstream,&id).await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
fn read_clipboard(app:tauri::AppHandle)->Result<String,String>{use tauri_plugin_clipboard_manager::ClipboardExt;app.clipboard().read_text().map_err(|e|e.to_string())}
#[tauri::command]
fn write_clipboard(app:tauri::AppHandle,text:String)->Result<(),String>{use tauri_plugin_clipboard_manager::ClipboardExt;app.clipboard().write_text(text).map_err(|e|e.to_string())}
#[tauri::command]
async fn flow_changes(state:State<'_,AppState>,since:Option<String>)->Result<capture_core::store::FlowChanges,String>{let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||engine.store.changes(since.as_deref())).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())}
#[tauri::command]
async fn request_library(state:State<'_,AppState>)->Result<capture_core::store::Library,String>{state.engine.store.library().map_err(|e|e.to_string())}
#[tauri::command]
async fn save_request_library(state:State<'_,AppState>,library:capture_core::store::Library)->Result<u64,String>{state.engine.store.save_library(library).map_err(|e|e.to_string())}
#[tauri::command]
async fn backup_database(state:State<'_,AppState>)->Result<String,String>{let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||engine.store.backup()).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())}
#[tauri::command]
async fn compact_database(state:State<'_,AppState>)->Result<(),String>{let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||engine.store.compact()).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())}
#[tauri::command]
async fn clear_database(state:State<'_,AppState>)->Result<usize,String>{let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||engine.store.clear()).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())}
#[tauri::command]
async fn begin_import(state:State<'_,AppState>)->Result<String,String>{state.engine.store.begin_import().map_err(|e|e.to_string())}
#[tauri::command]
async fn append_import(state:State<'_,AppState>,id:String,offset:usize,flows:Vec<Flow>)->Result<usize,String>{let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||engine.store.append_import(&id,offset,flows)).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())}
#[tauri::command]
async fn finish_import(state:State<'_,AppState>,id:String,commit:bool)->Result<usize,String>{let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||engine.store.finish_import(&id,commit)).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())}
#[tauri::command]
async fn save_response_base64(app:tauri::AppHandle,body:String,original:bool)->Result<String,String>{
    let directory=app.path().download_dir().map_err(|e|e.to_string())?;
    tauri::async_runtime::spawn_blocking(move||{
        let timestamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_nanos();
        let kind=if original{"original"}else{"decoded"};
        let path=directory.join(format!("response-{kind}-{timestamp}.bin"));
        capture_core::body_export::save_base64(&path,&body).map_err(|e|e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }).await.map_err(|e|e.to_string())?
}
#[tauri::command]
async fn save_response_file(app: tauri::AppHandle, bytes: Vec<u8>, original: bool) -> Result<String,String> {
    if bytes.len() > 8 * 1024 * 1024 { return Err("Response exceeds 8 MiB".into()); }
    let directory = app.path().download_dir().map_err(|e|e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        use std::io::Write;
        let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_nanos();
        let kind = if original { "original" } else { "decoded" };
        let path = directory.join(format!("response-{kind}-{timestamp}.bin"));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e|e.to_string())?;
        file.write_all(&bytes).map_err(|e|e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }).await.map_err(|e|e.to_string())?
}
#[tauri::command]
async fn save_session_file(content: String, format: Option<String>) -> Result<Option<String>,String> {
    let extension = match format.as_deref().unwrap_or("json") { "json" => "json", "har" => "har", "pem" => "pem", _ => return Err("Unsupported export format".into()) };
    let name=if extension=="pem" {"http-capture-ca.pem".to_string()} else {format!("capture.{extension}")};
    let Some(file)=rfd::AsyncFileDialog::new().set_file_name(&name).add_filter(extension,&[extension]).save_file().await else {return Ok(None)};
    let path=file.path().to_path_buf();
    tokio::fs::write(&path,content.as_bytes()).await.map_err(|e|e.to_string())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn save_code_file(content:String,filename:String)->Result<Option<String>,String> {
    // Suggested names are code-generator assets, never paths supplied to a write.
    if filename.is_empty() || filename.len()>80 || !filename.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b,b'.'|b'-'|b'_')) {
        return Err("Invalid code filename".into());
    }
    let extension=std::path::Path::new(&filename).extension().and_then(|v|v.to_str()).ok_or("Missing code extension")?;
    if !["py","js","mjs","go","c","cs","java","kt","dart","php","sh","txt"].contains(&extension) {return Err("Unsupported code extension".into());}
    let Some(file)=rfd::AsyncFileDialog::new().set_file_name(&filename).add_filter(extension,&[extension]).save_file().await else {return Ok(None)};
    let path=file.path().to_path_buf();
    tokio::fs::write(&path,content.as_bytes()).await.map_err(|e|e.to_string())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn delete_flows(state: State<'_, AppState>, ids: Vec<String>) -> Result<(),String> { state.engine.store.delete_flows(ids).map_err(|e|e.to_string()) }
#[tauri::command]
async fn import_flows(state: State<'_, AppState>, flows: Vec<Flow>) -> Result<usize,String> { state.engine.store.import_flows(flows).map_err(|e|e.to_string()) }
struct AppState {
    runtime: Arc<CaptureRuntime>,
    mcp_server: Arc<ServerManager>,
    mcp_clients: Arc<ClientManager>,
    mobile_gateway: tokio::sync::Mutex<Option<capture_core::mobile_gateway::MobileGateway>>,
}
impl std::ops::Deref for AppState {
    type Target = CaptureRuntime;
    fn deref(&self) -> &Self::Target { &self.runtime }
}
impl AppState {
    async fn resume_network_commands(&self) -> Result<(), String> {
        self.mcp_clients.resume().await.map_err(|error|error.to_string())?;
        self.runtime.resume_network_commands();
        Ok(())
    }
    async fn shutdown_network(&self) -> Result<(), String> {
        self.shutting_down.store(true, Ordering::Release);
        if let Some(gateway) = self.mobile_gateway.lock().await.take() { gateway.stop(); }
        self.engine.executions.cancel_all();
        let mut errors = Vec::new();
        if let Err(error) = self.mcp_server.shutdown().await { errors.push(format!("MCP: {error}")); }
        if let Err(error) = self.mcp_clients.shutdown().await { errors.push(format!("MCP client: {error}")); }
        if let Err(error) = self.runtime.shutdown_network().await { errors.push(error); }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("；")) }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MobileGatewayStatus {
    running: bool,
    address: Option<String>,
    paired: bool,
    fingerprint: Option<String>,
}

#[tauri::command]
async fn mobile_gateway_status(state: State<'_, AppState>) -> Result<MobileGatewayStatus, String> {
    let gateway = state.mobile_gateway.lock().await;
    Ok(MobileGatewayStatus {
        running: gateway.is_some(),
        address: gateway.as_ref().map(|gateway| gateway.address().to_string()),
        paired: gateway.as_ref().is_some_and(|gateway| gateway.is_paired()),
        fingerprint: gateway.as_ref().map(|gateway| gateway.fingerprint().to_string()),
    })
}

#[tauri::command]
async fn mobile_gateway_pair(host: String, state: State<'_, AppState>) -> Result<String, String> {
    let host: Ipv4Addr = host.parse().map_err(|_| "请选择可用的局域网 IPv4 地址".to_string())?;
    if !host.is_private() || host.is_loopback() { return Err("请选择私有局域网 IPv4 地址".into()); }
    let mut current = state.mobile_gateway.lock().await;
    if current.as_ref().is_some_and(|gateway| gateway.address().ip() != std::net::IpAddr::V4(host)) {
        current.take().unwrap().stop();
    }
    if current.is_none() {
        *current = Some(capture_core::mobile_gateway::MobileGateway::start(host, 0, state.engine.clone())
            .await.map_err(|error| format!("无法启动 Android 安全接收端：{error:#}"))?);
    }
    current.as_ref().unwrap().open_pairing().map(|code| code.to_uri())
        .map_err(|error| format!("无法生成配对二维码：{error:#}"))
}

#[tauri::command]
async fn mobile_gateway_close_pairing(uri: String, state: State<'_, AppState>) -> Result<(), String> {
    let token = capture_core::mobile_pairing::token_from_uri(&uri)
        .map_err(|_| "配对码无效".to_string())?;
    if let Some(gateway) = state.mobile_gateway.lock().await.as_ref() { gateway.close_pairing_if(&token); }
    Ok(())
}

#[tauri::command]
async fn mobile_gateway_disconnect(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(gateway) = state.mobile_gateway.lock().await.as_ref() { gateway.disconnect(); }
    Ok(())
}

#[tauri::command]
async fn mobile_gateway_stop(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(gateway) = state.mobile_gateway.lock().await.take() { gateway.stop(); }
    Ok(())
}

#[tauri::command]
async fn tun_status(state:State<'_,AppState>)->Result<capture_core::tun::TunStatus,String> {Ok(state.engine.tun.status().await)}
#[tauri::command]
async fn start_tun(config:capture_core::tun::TunConfig,state:State<'_,AppState>)->Result<(),String> {let _lifecycle=state.network_lifecycle.lock().await;state.ensure_network_ready()?;state.engine.tun.start(state.engine.clone(),config).await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
async fn stop_tun(state:State<'_,AppState>)->Result<(),String> {state.engine.tun.stop().await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
async fn reverse_quic_status(state:State<'_,AppState>)->Result<capture_core::runtime::ReverseQuicStatus,String> {Ok(state.runtime.reverse_quic_status().await)}
#[tauri::command]
async fn start_reverse_quic(target:String,port:u16,state:State<'_,AppState>)->Result<capture_core::runtime::ReverseQuicStatus,String> {state.runtime.start_reverse_quic(target,port).await}
#[tauri::command]
async fn stop_reverse_quic(state:State<'_,AppState>)->Result<capture_core::runtime::ReverseQuicStatus,String> {state.runtime.stop_reverse_quic().await}
#[tauri::command]
async fn network_interfaces() -> Result<Vec<capture_core::network::InterfaceAddress>, String> {
    tauri::async_runtime::spawn_blocking(||capture_core::network::interfaces().map_err(|e|format!("{e:#}"))).await.map_err(|e|e.to_string())?
}
#[tauri::command]
async fn check_listener_port(host: String, port: u16) -> Result<(), String> {
    capture_core::network::check_listener_port(&host, port).await.map_err(|e|format!("{e:#}"))
}
#[tauri::command]
async fn proxy_status(state: State<'_, AppState>) -> Result<ProxyStatus, String> {
    Ok(state.runtime.status().await)
}
#[tauri::command]
async fn start_proxy(port: u16, host: Option<String>, state: State<'_, AppState>) -> Result<(), String> {
    state.runtime.start_proxy(host.unwrap_or_else(||"127.0.0.1".into()),port).await
}
#[tauri::command]
async fn stop_proxy(state: State<'_, AppState>) -> Result<(), String> {
    state.runtime.stop_proxy().await
}
#[tauri::command]
async fn flow_revision(state: State<'_, AppState>) -> Result<String, String> {
    let engine = state.engine.clone();
    tauri::async_runtime::spawn_blocking(move || engine.store.revision().map_err(|e| e.to_string()))
        .await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn list_flows(state: State<'_, AppState>) -> Result<Vec<Flow>, String> {
    let engine = state.engine.clone();
    tauri::async_runtime::spawn_blocking(move || engine.store.list().map_err(|e| e.to_string()))
        .await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn replay_request(request: RequestDraft, parent_id: Option<String>, execution_id: Option<String>, state: State<'_, AppState>) -> Result<Flow, String> {
    state.runtime.replay(request,parent_id,execution_id).await
}
#[tauri::command]
fn prepare_replay(state: State<'_, AppState>) -> Result<String, String> {
    let _lifecycle = state.network_lifecycle.try_lock().map_err(|_| "网络设置正在变更，请稍后重试")?;
    state.ensure_network_ready()?;
    state.engine.executions.prepare().map_err(|e|e.to_string())
}
#[tauri::command]
fn cancel_replay(execution_id: String, state: State<'_, AppState>) -> bool {
    state.engine.executions.cancel(&execution_id)
}
#[tauri::command]
fn export_certificate(state: State<'_, AppState>) -> Result<String, String> {
    std::fs::read_to_string(&state.engine.ca.cert_path).map_err(|e| e.to_string())
}
#[tauri::command]
fn set_upstream(config: capture_core::upstream::UpstreamInput, state: State<'_, AppState>) -> Result<(), String> {
    state.engine.upstream.update(config).map_err(|e| e.to_string())
}
#[tauri::command]
fn blocking(state:State<'_,AppState>) -> capture_core::blocking::Config { state.engine.blocking.snapshot() }
#[tauri::command]
fn configure_blocking(config:capture_core::blocking::Config, state:State<'_,AppState>) -> Result<(),String> { state.engine.blocking.configure(config).map_err(|e|e.to_string()) }
#[tauri::command]
fn fault_rules(state:State<'_,AppState>)->capture_core::fault_rules::Snapshot{state.engine.fault_rules.snapshot()}
#[tauri::command]
fn configure_fault_rules(state:State<'_,AppState>,config:capture_core::fault_rules::Config,expected_version:String)->Result<capture_core::fault_rules::Snapshot,String>{state.engine.fault_rules.configure(config,&expected_version).map_err(|e|e.to_string())}
#[tauri::command]
async fn mcp_audit(state:State<'_,AppState>,cursor:Option<String>,limit:Option<usize>)->Result<capture_core::audit::AuditPage,String>{let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||engine.store.mcp_audit_page(capture_core::audit::AuditQuery{cursor,limit})).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())}
#[tauri::command]
fn interception(state: State<'_, AppState>) -> capture_core::intercept::Snapshot { state.engine.intercept.snapshot() }
#[tauri::command]
fn configure_interception(config: capture_core::intercept::Config, state: State<'_, AppState>) -> Result<(),String> { state.engine.intercept.configure(config).map_err(|e|e.to_string()) }
#[tauri::command]
fn resolve_interception(decision: capture_core::intercept::Decision, state: State<'_, AppState>) -> Result<(),String> { state.engine.intercept.resolve(decision).map_err(|e|e.to_string()) }
#[tauri::command]
fn capture_scripts(state: State<'_, AppState>) -> capture_core::scripts::Scripts { state.engine.scripts.snapshot().0 }
#[tauri::command]
fn set_capture_scripts(config: capture_core::scripts::Scripts, state: State<'_, AppState>) -> Result<(),String> { state.engine.scripts.configure(config).map_err(|e|e.to_string()) }
#[tauri::command]
async fn upstream_profiles(state:State<'_,AppState>)->Result<Vec<capture_core::upstream::ProfileView>,String>{let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||engine.upstream.list_profiles()).await.map_err(|e|e.to_string())}
#[tauri::command]
async fn save_upstream_profile(input:capture_core::upstream::ProfileInput,state:State<'_,AppState>)->Result<(),String>{let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||engine.upstream.save_profile(input)).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())}
#[tauri::command]
fn select_upstream_profile(id:Option<String>,state:State<'_,AppState>)->Result<(),String>{state.engine.upstream.select_profile(id).map_err(|e|e.to_string())}
#[tauri::command]
async fn delete_upstream_profile(id:String,state:State<'_,AppState>)->Result<(),String>{let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||engine.upstream.delete_profile(id)).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())}
#[tauri::command]
async fn request_workspace(state:State<'_,AppState>)->Result<capture_core::store::Workspace,String>{
 let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||engine.store.workspace()).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())
}
#[tauri::command]
async fn save_request_workspace(workspace:capture_core::store::Workspace,state:State<'_,AppState>)->Result<u64,String>{
 let engine=state.engine.clone();tauri::async_runtime::spawn_blocking(move||engine.store.save_workspace(workspace)).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())
}
fn main() {
    // Branch before GTK/WebKit/Tauri and before Engine::open. A configured AI
    // client launches this executable as a stdio adapter to the running app.
    match capture_mcp::bridge::stdio_options(std::env::args_os().skip(1)) {
        Ok(Some(options))=>{
            let outcome=tokio::runtime::Builder::new_multi_thread().enable_all().build()
                .map_err(|error|error.to_string()).and_then(|runtime|{
                    let outcome=runtime.block_on(capture_mcp::bridge::run_stdio(options));
                    // Tokio's stdin owns an uncancellable blocking read. AI
                    // clients can keep that pipe open after the app stops;
                    // never wait for its worker during bridge process exit.
                    runtime.shutdown_background();
                    outcome.map_err(|error|error.to_string())
                });
            if let Err(error)=outcome {eprintln!("MCP stdio: {error}");std::process::exit(1);}
            return;
        }
        Ok(None)=>{}
        Err(error)=>{eprintln!("MCP stdio: {error}");std::process::exit(1);}
    }
    // Must run before GTK/WebKit initialization and before worker threads exist.
    #[cfg(target_os = "linux")]
    {
        // Reported GBM/DRM buffer allocation failure: ship the compatible renderer
        // by default. Advanced users can explicitly select the normal path with 0.
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        // Optional desktop event sounds are not required by the application.
        // Preserve all other modules, particularly accessibility integrations.
        for key in ["GTK_MODULES", "GTK3_MODULES"] {
            if let Ok(value) = std::env::var(key) {
                let modules: Vec<_> = value.split(':').filter(|name| !matches!(*name,
                    "canberra-gtk-module" | "libcanberra-gtk-module.so")).collect();
                if modules.len() != value.split(':').count() {
                    if modules.iter().all(|v|v.is_empty()) { std::env::remove_var(key); }
                    else { std::env::set_var(key, modules.join(":")); }
                }
            }
        }
    }
    tauri::Builder::default().plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(app_update::AppUpdateState::default())
        .setup(|app| {
            // Resources have different locations in .app, Debian and Windows bundles.
            // Resolve before constructing the engine so helper discovery works off-repo.
            let helper = app.path().resource_dir()?.join("helpers").join(if cfg!(windows) { "http-capture-httpcloak.exe" } else { "http-capture-httpcloak" });
            if std::env::var_os("HTTP_CAPTURE_HTTPCLOAK").is_none() && helper.is_file() {
                std::env::set_var("HTTP_CAPTURE_HTTPCLOAK", helper);
            }
            let engine = Engine::open(&app.path().app_data_dir()?)?;
            if let Err(error)=engine.system_proxy.restore(){eprintln!("System proxy recovery: {error}");}
            // The UI uses revision/delta polling. Do not also serialize every
            // complete body into an unconsumed WebView event on each update.
            let runtime = CaptureRuntime::new(engine);
            let mcp_server = ServerManager::new(runtime.clone())?;
            let mcp_clients = ClientManager::new(runtime.engine.store.clone())?;
            let startup = mcp_server.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = startup.restore().await { eprintln!("MCP startup: {error}"); }
            });
            app.manage(AppState { runtime, mcp_server, mcp_clients,
                mobile_gateway: tokio::sync::Mutex::new(None) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![mobile_gateway_status,mobile_gateway_pair,mobile_gateway_close_pairing,mobile_gateway_disconnect,mobile_gateway_stop,session_export::begin_session_export,session_export::append_session_export,session_export::finish_session_export,certificate_commands::certificates,certificate_commands::import_certificate,certificate_commands::change_certificate,body_commands::flow_metadata_changes,body_commands::flow_metadata,body_commands::get_flow,body_commands::read_flow_body,body_commands::save_flow_body,response_rewrite_commands::response_rewrite,response_rewrite_commands::configure_response_rewrite,config_migration_commands::config_status,config_migration_commands::config_export,config_migration_commands::config_preview,config_migration_commands::config_import,config_migration_commands::config_recovery,config_migration_commands::config_restart,config_migration_commands::config_save_file,mcp_commands::workspace_revisions,mcp_commands::mcp_status,mcp_commands::mcp_configure,mcp_commands::mcp_connection_info,mcp_commands::mcp_clients,mcp_commands::mcp_save_client,mcp_commands::mcp_remove_client,mcp_commands::mcp_connect_client,mcp_commands::mcp_disconnect_client,mcp_commands::mcp_client_tools,mcp_commands::mcp_call_tool,mcp_commands::plugins_validate,app_update::open_update_download,app_update::app_update_status,app_update::check_app_update,app_update::download_app_update,app_update::install_app_update,check_listener_port,plugins_list,plugins_save,plugin_logs,grpc_poll,grpc_start,grpc_send,save_response_base64,system_proxy_status,set_system_proxy,ca_status,set_ca_trust,websocket_connect,websocket_send,decryption_hosts,save_decryption_hosts,test_upstream,read_clipboard, write_clipboard, flow_changes, request_library, save_request_library, backup_database, compact_database, clear_database, begin_import, append_import, finish_import, save_response_file, save_session_file, save_code_file, delete_flows, import_flows, tun_status, start_tun, stop_tun, reverse_quic_status, start_reverse_quic, stop_reverse_quic, network_interfaces, request_workspace, save_request_workspace, proxy_status, start_proxy, stop_proxy, list_flows, flow_revision, replay_request, prepare_replay, cancel_replay, export_certificate, set_upstream, blocking, configure_blocking,fault_rules,configure_fault_rules,mcp_audit, interception, configure_interception, resolve_interception, capture_scripts, set_capture_scripts, upstream_profiles, save_upstream_profile, select_upstream_profile, delete_upstream_profile])
        .build(tauri::generate_context!())
        .expect("Failed to start HTTP Capture")
        .run(|app,event| match event {
            // The application menu's Quit action can bypass the WebView's
            // CloseRequested event. Route it through the same save guard as
            // the title-bar close. A destroyed window has already passed that
            // guard, and programmatic exits (including updater restart) keep
            // their existing behavior.
            tauri::RunEvent::ExitRequested { code: None, api, .. }
                if app.get_webview_window("main").is_some() => {
                    api.prevent_exit();
                    if let Err(error) = app.emit_to("main", "desktop-close-requested", ()) {
                        eprintln!("Desktop close request: {error}");
                    }
                }
            tauri::RunEvent::Exit => {
                let state=app.state::<AppState>();
                if let Err(error)=tauri::async_runtime::block_on(state.shutdown_network()){eprintln!("Network shutdown: {error}");}
            }
            _ => {}
        });
}

#[tauri::command]
fn plugins_list(state:State<'_,AppState>)->Vec<capture_core::plugins::Plugin>{state.engine.plugins.list()}
#[tauri::command]
fn plugins_save(plugins:Vec<capture_core::plugins::Plugin>,state:State<'_,AppState>)->Result<(),String>{state.engine.plugins.save(plugins).map_err(|e|e.to_string())}
#[tauri::command]
fn plugin_logs(state:State<'_,AppState>)->Vec<String>{state.engine.plugins.logs()}
