#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use capture_core::{model::{Flow, ProxyStatus, RequestDraft}, proxy::{self, ProxyHandle}, Engine};
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use tokio::sync::Mutex;

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
async fn save_session_file(app: tauri::AppHandle, content: String) -> Result<String,String> {
    let directory = app.path().download_dir().map_err(|e|e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        use std::io::Write;
        let timestamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_nanos();
        let path=directory.join(format!("capture-{timestamp}.json"));
        let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e|e.to_string())?;
        file.write_all(content.as_bytes()).map_err(|e|e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }).await.map_err(|e|e.to_string())?
}
#[tauri::command]
async fn delete_flows(state: State<'_, AppState>, ids: Vec<String>) -> Result<(),String> { state.engine.store.delete_flows(ids).map_err(|e|e.to_string()) }
#[tauri::command]
async fn import_flows(state: State<'_, AppState>, flows: Vec<Flow>) -> Result<usize,String> { state.engine.store.import_flows(flows).map_err(|e|e.to_string()) }
struct AppState { engine: Arc<Engine>, proxy: Mutex<Option<ProxyHandle>> }

#[tauri::command]
async fn tun_status(state:State<'_,AppState>)->Result<capture_core::tun::TunStatus,String> {Ok(state.engine.tun.status().await)}
#[tauri::command]
async fn start_tun(config:capture_core::tun::TunConfig,state:State<'_,AppState>)->Result<(),String> {state.engine.tun.start(state.engine.clone(),config).await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
async fn stop_tun(state:State<'_,AppState>)->Result<(),String> {state.engine.tun.stop().await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
async fn network_interfaces() -> Result<Vec<capture_core::network::InterfaceAddress>, String> {
    tauri::async_runtime::spawn_blocking(||capture_core::network::interfaces().map_err(|e|format!("{e:#}"))).await.map_err(|e|e.to_string())?
}
#[tauri::command]
async fn proxy_status(state: State<'_, AppState>) -> Result<ProxyStatus, String> {
    let proxy = state.proxy.lock().await;
    Ok(ProxyStatus { upstream: state.engine.upstream.status(), send_engines: state.engine.send_engines.list(), running: proxy.is_some(), address: proxy.as_ref().map(|p| p.address.to_string()),
        ca_path: state.engine.ca.cert_path.to_string_lossy().into_owned(), browser_replay: cfg!(feature = "browser-replay") })
}
#[tauri::command]
async fn start_proxy(port: u16, host: Option<String>, state: State<'_, AppState>) -> Result<(), String> {
    let mut handle = state.proxy.lock().await;
    if handle.is_some() { return Err("代理已经启动".into()); }
    let address = capture_core::network::listener_address(host.as_deref().unwrap_or("127.0.0.1"),port).map_err(|e|format!("{e:#}"))?;
    *handle = Some(proxy::start_at(state.engine.clone(), address).await.map_err(|e| format!("{e:#}"))?);
    Ok(())
}
#[tauri::command]
async fn stop_proxy(state: State<'_, AppState>) -> Result<(), String> {
    let mut handle = state.proxy.lock().await;
    if let Some(proxy) = handle.take() { proxy.stop().await; }
    Ok(())
}
#[tauri::command]
async fn list_flows(state: State<'_, AppState>) -> Result<Vec<Flow>, String> {
    let engine = state.engine.clone();
    tauri::async_runtime::spawn_blocking(move || engine.store.list().map_err(|e| e.to_string()))
        .await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn replay_request(request: RequestDraft, parent_id: Option<String>, execution_id: Option<String>, state: State<'_, AppState>) -> Result<Flow, String> {
    state.engine.replay(request, parent_id, execution_id).await.map_err(|e| e.to_string())
}
#[tauri::command]
fn prepare_replay(state: State<'_, AppState>) -> Result<String, String> {
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
    tauri::Builder::default()
        .setup(|app| {
            // Resources have different locations in .app, Debian and Windows bundles.
            // Resolve before constructing the engine so helper discovery works off-repo.
            let helper = app.path().resource_dir()?.join("helpers").join(if cfg!(windows) { "http-capture-httpcloak.exe" } else { "http-capture-httpcloak" });
            if std::env::var_os("HTTP_CAPTURE_HTTPCLOAK").is_none() && helper.is_file() {
                std::env::set_var("HTTP_CAPTURE_HTTPCLOAK", helper);
            }
            let engine = Engine::open(&app.path().app_data_dir()?)?;
            let mut events = engine.events.subscribe();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(flow) => { let _ = handle.emit("flow-recorded", flow); }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => { let _ = handle.emit("flows-refresh", ()); }
                        Err(_) => break,
                    }
                }
            });
            app.manage(AppState { engine, proxy: Mutex::new(None) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![save_response_file, save_session_file, delete_flows, import_flows, tun_status, start_tun, stop_tun, network_interfaces, request_workspace, save_request_workspace, proxy_status, start_proxy, stop_proxy, list_flows, replay_request, prepare_replay, cancel_replay, export_certificate, set_upstream, interception, configure_interception, resolve_interception, capture_scripts, set_capture_scripts, upstream_profiles, save_upstream_profile, select_upstream_profile, delete_upstream_profile])
        .build(tauri::generate_context!())
        .expect("Failed to start HTTP Capture")
        .run(|app,event| { if let tauri::RunEvent::Exit = event {
            let state=app.state::<AppState>();
            let _=tauri::async_runtime::block_on(state.engine.tun.stop());
        }});
}
