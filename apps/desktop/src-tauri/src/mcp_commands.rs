use super::AppState;
use capture_mcp::{server::{ServerConfig,ServerStatus}, client::{ClientInput,ClientStatus}};
use tauri::State;
use serde_json::Value;

#[tauri::command]
pub async fn workspace_revisions(state:State<'_,AppState>)->Result<Value,String>{let mut value=state.engine.store.workspace_revisions().map_err(|e|format!("{e:#}"))?;value["configurationFrozen"]=serde_json::json!(state.engine.store.configuration_ready().is_err());Ok(value)}

#[tauri::command]
pub async fn mcp_status(state:State<'_,AppState>)->Result<ServerStatus,String> {Ok(state.mcp_server.status().await)}
#[tauri::command]
pub async fn mcp_configure(state:State<'_,AppState>,config:ServerConfig)->Result<ServerStatus,String>{state.ensure_network_ready()?;state.mcp_server.configure(config).await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
pub async fn mcp_connection_info(state:State<'_,AppState>)->Result<Value,String>{state.mcp_server.connection_info().await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
pub async fn mcp_clients(state:State<'_,AppState>)->Result<Vec<ClientStatus>,String>{Ok(state.mcp_clients.list().await)}
#[tauri::command]
pub async fn mcp_save_client(state:State<'_,AppState>,input:ClientInput)->Result<ClientStatus,String>{state.mcp_clients.save(input).await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
pub async fn mcp_remove_client(state:State<'_,AppState>,id:String)->Result<(),String>{state.mcp_clients.remove(id).await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
pub async fn mcp_connect_client(state:State<'_,AppState>,id:String)->Result<ClientStatus,String>{state.ensure_network_ready()?;state.mcp_clients.connect(id).await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
pub async fn mcp_disconnect_client(state:State<'_,AppState>,id:String)->Result<ClientStatus,String>{state.mcp_clients.disconnect(id).await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
pub async fn mcp_client_tools(state:State<'_,AppState>,id:String)->Result<Vec<Value>,String>{state.mcp_clients.tools(id).await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
pub async fn mcp_call_tool(state:State<'_,AppState>,id:String,name:String,arguments:Value)->Result<Value,String>{state.ensure_network_ready()?;state.mcp_clients.call(id,name,arguments).await.map_err(|e|format!("{e:#}"))}
#[tauri::command]
pub fn plugins_validate(plugin:capture_core::plugins::Plugin)->Result<(),String>{capture_core::plugins::validate_plugin(&plugin).map_err(|e|format!("{e:#}"))}
