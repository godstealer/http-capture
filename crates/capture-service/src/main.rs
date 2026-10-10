//! Authenticated loopback control API for the Vite development UI.
//! The token stays in .local and the Vite server; it is never sent to the browser.
use anyhow::Result;
use axum::{extract::State, http::{HeaderMap, StatusCode}, routing::{get, post}, Json, Router};
use capture_core::{model::{Flow, ProxyStatus, RequestDraft}, proxy::{self, ProxyHandle}, Engine};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

struct Service { engine: Arc<Engine>, proxy: Mutex<Option<ProxyHandle>>, token: String }
type Shared = Arc<Service>;
type ApiError = (StatusCode, String);

fn authorize(headers: &HeaderMap, state: &Service) -> Result<(), ApiError> {
    if headers.get("authorization").and_then(|h| h.to_str().ok()) != Some(&format!("Bearer {}", state.token)) {
        return Err((StatusCode::UNAUTHORIZED, "Unauthorized".into()));
    }
    Ok(())
}
fn error(e: impl std::fmt::Display) -> ApiError { (StatusCode::BAD_REQUEST, e.to_string()) }
async fn tun_status(State(s):State<Shared>,headers:HeaderMap)->Result<Json<capture_core::tun::TunStatus>,ApiError>{authorize(&headers,&s)?;Ok(Json(s.engine.tun.status().await))}
#[derive(Deserialize)] struct TunStart {config:capture_core::tun::TunConfig}
async fn start_tun(State(s):State<Shared>,headers:HeaderMap,Json(input):Json<TunStart>)->Result<Json<()>,ApiError>{authorize(&headers,&s)?;s.engine.tun.start(s.engine.clone(),input.config).await.map(Json).map_err(error)}
async fn stop_tun(State(s):State<Shared>,headers:HeaderMap)->Result<Json<()>,ApiError>{authorize(&headers,&s)?;s.engine.tun.stop().await.map(Json).map_err(error)}
async fn network_interfaces(State(s): State<Shared>, headers: HeaderMap) -> Result<Json<Vec<capture_core::network::InterfaceAddress>>, ApiError> {
    authorize(&headers,&s)?;
    tokio::task::spawn_blocking(capture_core::network::interfaces).await.map_err(error)?.map(Json).map_err(error)
}
async fn status(State(s): State<Shared>, headers: HeaderMap) -> Result<Json<ProxyStatus>, ApiError> {
    authorize(&headers, &s)?;
    let proxy = s.proxy.lock().await;
    Ok(Json(ProxyStatus { upstream: s.engine.upstream.status(), send_engines: s.engine.send_engines.list(), running: proxy.is_some(), address: proxy.as_ref().map(|p| p.address.to_string()),
        ca_path: s.engine.ca.cert_path.to_string_lossy().into_owned(), browser_replay: cfg!(feature = "browser-replay") }))
}
#[derive(Deserialize)] struct ChangesInput {since:Option<String>}
async fn flow_changes(State(s):State<Shared>,headers:HeaderMap,Json(p):Json<ChangesInput>)->Result<Json<capture_core::store::FlowChanges>,ApiError>{authorize(&headers,&s)?;tokio::task::spawn_blocking(move||s.engine.store.changes(p.since.as_deref())).await.map_err(error)?.map(Json).map_err(error)}
async fn flow_revision(State(s): State<Shared>, headers: HeaderMap) -> Result<Json<String>, ApiError> {
    authorize(&headers, &s)?;
    let engine = s.engine.clone();
    tokio::task::spawn_blocking(move || engine.store.revision()).await.map_err(error)?.map(Json).map_err(error)
}
async fn list(State(s): State<Shared>, headers: HeaderMap) -> Result<Json<Vec<Flow>>, ApiError> {
    authorize(&headers, &s)?;
    let engine = s.engine.clone();
    let flows = tokio::task::spawn_blocking(move || engine.store.list()).await.map_err(error)?.map_err(error)?;
    Ok(Json(flows))
}
#[derive(Deserialize)] struct WsConnect{request:RequestDraft}
#[derive(Deserialize)] struct WsSend{id:String,opcode:u8,body:String}
async fn websocket_connect(State(s):State<Shared>,headers:HeaderMap,Json(p):Json<WsConnect>)->Result<Json<String>,ApiError>{authorize(&headers,&s)?;capture_core::websocket_client::connect(s.engine.clone(),p.request).map(Json).map_err(error)}
async fn websocket_send(State(s):State<Shared>,headers:HeaderMap,Json(p):Json<WsSend>)->Result<Json<()>,ApiError>{authorize(&headers,&s)?;capture_core::websocket_client::send(&s.engine,&p.id,p.opcode,&p.body).await.map(Json).map_err(error)}
async fn decryption_hosts(State(s):State<Shared>,headers:HeaderMap)->Result<Json<Vec<String>>,ApiError>{authorize(&headers,&s)?;Ok(Json(s.engine.store.setting("decryption.bypass.v1").map_err(error)?.unwrap_or_default()))}
#[derive(Deserialize)] struct DecryptionInput{hosts:Vec<String>}
async fn save_decryption_hosts(State(s):State<Shared>,headers:HeaderMap,Json(p):Json<DecryptionInput>)->Result<Json<()>,ApiError>{authorize(&headers,&s)?;capture_core::decryption::save(&s.engine.store,p.hosts).map(Json).map_err(error)}
async fn test_upstream(State(s):State<Shared>,headers:HeaderMap,Json(p):Json<ProfileDelete>)->Result<Json<u64>,ApiError>{authorize(&headers,&s)?;Ok(Json(capture_core::upstream::test_profile(&s.engine.upstream,&p.id).await.map_err(error)?))}
async fn library(State(s):State<Shared>,headers:HeaderMap)->Result<Json<capture_core::store::Library>,ApiError>{authorize(&headers,&s)?;s.engine.store.library().map(Json).map_err(error)}
#[derive(Deserialize)] struct LibraryInput {library:capture_core::store::Library}
async fn save_library(State(s):State<Shared>,headers:HeaderMap,Json(p):Json<LibraryInput>)->Result<Json<u64>,ApiError>{authorize(&headers,&s)?;s.engine.store.save_library(p.library).map(Json).map_err(error)}
async fn backup_database(State(s):State<Shared>,headers:HeaderMap)->Result<Json<String>,ApiError>{authorize(&headers,&s)?;tokio::task::spawn_blocking(move||s.engine.store.backup()).await.map_err(error)?.map(Json).map_err(error)}
async fn compact_database(State(s):State<Shared>,headers:HeaderMap)->Result<Json<()>,ApiError>{authorize(&headers,&s)?;tokio::task::spawn_blocking(move||s.engine.store.compact()).await.map_err(error)?.map(Json).map_err(error)}
async fn clear_database(State(s):State<Shared>,headers:HeaderMap)->Result<Json<usize>,ApiError>{authorize(&headers,&s)?;tokio::task::spawn_blocking(move||s.engine.store.clear()).await.map_err(error)?.map(Json).map_err(error)}
async fn begin_import(State(s):State<Shared>,headers:HeaderMap)->Result<Json<String>,ApiError>{authorize(&headers,&s)?;tokio::task::spawn_blocking(move||s.engine.store.begin_import()).await.map_err(error)?.map(Json).map_err(error)}
#[derive(Deserialize)] struct ImportChunk {id:String,offset:usize,flows:Vec<Flow>}
async fn append_import(State(s):State<Shared>,headers:HeaderMap,Json(p):Json<ImportChunk>)->Result<Json<usize>,ApiError>{authorize(&headers,&s)?;tokio::task::spawn_blocking(move||s.engine.store.append_import(&p.id,p.offset,p.flows)).await.map_err(error)?.map(Json).map_err(error)}
#[derive(Deserialize)] struct ImportFinish {id:String,commit:bool}
async fn finish_import(State(s):State<Shared>,headers:HeaderMap,Json(p):Json<ImportFinish>)->Result<Json<usize>,ApiError>{authorize(&headers,&s)?;tokio::task::spawn_blocking(move||s.engine.store.finish_import(&p.id,p.commit)).await.map_err(error)?.map(Json).map_err(error)}
#[derive(Deserialize)] struct DeleteFlows { ids: Vec<String> }
async fn delete_flows(State(s):State<Shared>, headers:HeaderMap, Json(input):Json<DeleteFlows>)->Result<Json<()>,ApiError>{
 authorize(&headers,&s)?; tokio::task::spawn_blocking(move||s.engine.store.delete_flows(input.ids)).await.map_err(error)?.map(Json).map_err(error)
}
#[derive(Deserialize)] struct ImportFlows { flows: Vec<Flow> }
async fn import_flows(State(s):State<Shared>, headers:HeaderMap, Json(input):Json<ImportFlows>)->Result<Json<usize>,ApiError>{
 authorize(&headers,&s)?; tokio::task::spawn_blocking(move||s.engine.store.import_flows(input.flows)).await.map_err(error)?.map(Json).map_err(error)
}
#[derive(Deserialize)] struct Start { port: u16, host: Option<String> }
async fn start(State(s): State<Shared>, headers: HeaderMap, Json(input): Json<Start>) -> Result<Json<()>, ApiError> {
    authorize(&headers, &s)?;
    let mut running = s.proxy.lock().await;
    if running.is_some() { return Err(error("Proxy is already running")); }
    let address = capture_core::network::listener_address(input.host.as_deref().unwrap_or("127.0.0.1"),input.port).map_err(|e|error(format!("{e:#}")))?;
    *running = Some(proxy::start_at(s.engine.clone(), address).await.map_err(|e|error(format!("{e:#}")))?);
    Ok(Json(()))
}
async fn stop(State(s): State<Shared>, headers: HeaderMap) -> Result<Json<()>, ApiError> {
    authorize(&headers, &s)?;
    let mut running = s.proxy.lock().await;
    if let Some(proxy) = running.take() { proxy.stop().await; }
    Ok(Json(()))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Replay { request: RequestDraft, parent_id: Option<String>, execution_id: Option<String> }
async fn replay(State(s): State<Shared>, headers: HeaderMap, Json(input): Json<Replay>) -> Result<Json<Flow>, ApiError> {
    authorize(&headers, &s)?;
    s.engine.replay(input.request, input.parent_id, input.execution_id).await.map(Json).map_err(error)
}
async fn prepare_replay(State(s): State<Shared>, headers: HeaderMap) -> Result<Json<String>, ApiError> {
    authorize(&headers, &s)?;
    s.engine.executions.prepare().map(Json).map_err(error)
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase")]
struct CancelReplay { execution_id: String }
async fn cancel_replay(State(s): State<Shared>, headers: HeaderMap, Json(input): Json<CancelReplay>) -> Result<Json<bool>, ApiError> {
    authorize(&headers, &s)?;
    Ok(Json(s.engine.executions.cancel(&input.execution_id)))
}
async fn certificate(State(s): State<Shared>, headers: HeaderMap) -> Result<String, ApiError> {
    authorize(&headers, &s)?;
    std::fs::read_to_string(&s.engine.ca.cert_path).map_err(error)
}

#[derive(Deserialize)] struct UpstreamUpdate { config: capture_core::upstream::UpstreamInput }
async fn set_upstream(State(s): State<Shared>, headers: HeaderMap, Json(input): Json<UpstreamUpdate>) -> Result<Json<()>, ApiError> {
    authorize(&headers, &s)?; s.engine.upstream.update(input.config).map_err(error)?; Ok(Json(()))
}

async fn interception(State(s): State<Shared>, headers: HeaderMap) -> Result<Json<capture_core::intercept::Snapshot>, ApiError> {
 authorize(&headers,&s)?; Ok(Json(s.engine.intercept.snapshot()))
}
#[derive(Deserialize)] struct InterceptConfig { config: capture_core::intercept::Config }
async fn configure_interception(State(s): State<Shared>, headers: HeaderMap, Json(input): Json<InterceptConfig>) -> Result<Json<()>, ApiError> {
 authorize(&headers,&s)?; s.engine.intercept.configure(input.config).map_err(error)?; Ok(Json(()))
}
#[derive(Deserialize)] struct InterceptDecision { decision: capture_core::intercept::Decision }
async fn resolve_interception(State(s): State<Shared>, headers: HeaderMap, Json(input): Json<InterceptDecision>) -> Result<Json<()>, ApiError> {
 authorize(&headers,&s)?; s.engine.intercept.resolve(input.decision).map_err(error)?; Ok(Json(()))
}
async fn capture_scripts(State(s): State<Shared>, headers: HeaderMap) -> Result<Json<capture_core::scripts::Scripts>, ApiError> {
 authorize(&headers,&s)?; Ok(Json(s.engine.scripts.snapshot().0))
}
#[derive(Deserialize)] struct ScriptConfig { config: capture_core::scripts::Scripts }
async fn set_capture_scripts(State(s): State<Shared>, headers: HeaderMap, Json(input): Json<ScriptConfig>) -> Result<Json<()>, ApiError> {
 authorize(&headers,&s)?; s.engine.scripts.configure(input.config).map_err(error)?; Ok(Json(()))
}
async fn upstream_profiles(State(s): State<Shared>, headers: HeaderMap)->Result<Json<Vec<capture_core::upstream::ProfileView>>,ApiError>{authorize(&headers,&s)?;Ok(Json(tokio::task::spawn_blocking(move||s.engine.upstream.list_profiles()).await.map_err(error)?))}
#[derive(Deserialize)] struct ProfileSave { input:capture_core::upstream::ProfileInput }
async fn save_upstream_profile(State(s): State<Shared>,headers:HeaderMap,Json(p):Json<ProfileSave>)->Result<Json<()>,ApiError>{authorize(&headers,&s)?;tokio::task::spawn_blocking(move||s.engine.upstream.save_profile(p.input)).await.map_err(error)?.map_err(error)?;Ok(Json(()))}
#[derive(Deserialize)] struct ProfileSelect { id:Option<String> }
async fn select_upstream_profile(State(s): State<Shared>,headers:HeaderMap,Json(p):Json<ProfileSelect>)->Result<Json<()>,ApiError>{authorize(&headers,&s)?;s.engine.upstream.select_profile(p.id).map_err(error)?;Ok(Json(()))}
#[derive(Deserialize)] struct ProfileDelete { id:String }
async fn delete_upstream_profile(State(s): State<Shared>,headers:HeaderMap,Json(p):Json<ProfileDelete>)->Result<Json<()>,ApiError>{authorize(&headers,&s)?;tokio::task::spawn_blocking(move||s.engine.upstream.delete_profile(p.id)).await.map_err(error)?.map_err(error)?;Ok(Json(()))}
async fn workspace(State(s):State<Shared>,headers:HeaderMap)->Result<Json<capture_core::store::Workspace>,ApiError>{
 authorize(&headers,&s)?;tokio::task::spawn_blocking(move||s.engine.store.workspace()).await.map_err(error)?.map(Json).map_err(error)
}
#[derive(Deserialize)] struct SaveWorkspace { workspace:capture_core::store::Workspace }
async fn save_workspace(State(s):State<Shared>,headers:HeaderMap,Json(input):Json<SaveWorkspace>)->Result<Json<u64>,ApiError>{
 authorize(&headers,&s)?;tokio::task::spawn_blocking(move||s.engine.store.save_workspace(input.workspace)).await.map_err(error)?.map(Json).map_err(error)
}
#[tokio::main]
async fn main() -> Result<()> {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".local/capture".into()));
    let engine = Engine::open(&root)?;
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let address = "127.0.0.1:1421";
    let listener = tokio::net::TcpListener::bind(address).await?;
    let token_path = root.join("control.token");
    std::fs::write(&token_path, &token)?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600))?; }
    let state = Arc::new(Service { engine, proxy: Mutex::new(None), token });
    let app = router(state.clone());
    println!("Capture control: http://{address}\nData: {}\nOpen the GUI and click Start capture.", root.display());
    axum::serve(listener, app).with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; }).await?;
    let _ = state.engine.tun.stop().await;
    if let Some(proxy) = state.proxy.lock().await.take() { proxy.stop().await; }
    Ok(())
}

fn router(state:Shared)->Router {
    Router::new().route("/websocket/connect",post(websocket_connect)).route("/websocket/send",post(websocket_send)).route("/decryption",get(decryption_hosts).post(save_decryption_hosts)).route("/upstream/test",post(test_upstream)).route("/library",get(library).post(save_library)).route("/database/backup",post(backup_database)).route("/database/compact",post(compact_database)).route("/database/clear",post(clear_database))
        .route("/imports/begin",post(begin_import)).route("/imports/append",post(append_import).layer(axum::extract::DefaultBodyLimit::max(64*1024*1024))).route("/imports/finish",post(finish_import)).route("/tun",get(tun_status)).route("/tun/start",post(start_tun)).route("/tun/stop",post(stop_tun)).route("/network/interfaces",get(network_interfaces)).route("/status", get(status)).route("/flows", get(list)).route("/flows/changes",post(flow_changes)).route("/flows/revision", get(flow_revision)).route("/flows/delete",post(delete_flows)).route("/flows/import",post(import_flows))
        .route("/workspace",get(workspace).post(save_workspace)).route("/start", post(start)).route("/stop", post(stop)).route("/replay", post(replay)).route("/replay/prepare",post(prepare_replay)).route("/replay/cancel",post(cancel_replay))
        .route("/upstream/profiles",get(upstream_profiles).post(save_upstream_profile)).route("/upstream/select",post(select_upstream_profile)).route("/upstream/delete",post(delete_upstream_profile))
        .route("/scripts", get(capture_scripts).post(set_capture_scripts))
        .route("/interception", get(interception).post(configure_interception)).route("/interception/resolve", post(resolve_interception))
        .route("/certificate", get(certificate)).route("/upstream", post(set_upstream))
        .layer(axum::extract::DefaultBodyLimit::max(12 * 1024 * 1024)).with_state(state)
}

#[cfg(test)] mod contract_tests;
