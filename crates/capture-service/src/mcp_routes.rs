use super::{Shared,ApiError,error};
use axum::{extract::State,routing::{get,post},Json,Router};
use capture_mcp::{server::{ServerConfig,ServerStatus},client::{ClientInput,ClientStatus}};
use serde::Deserialize;
use serde_json::Value;

pub fn router()->Router<Shared>{
    Router::new().route("/mcp/status",get(status)).route("/workspace/revisions",get(workspace_revisions))
        .route("/mcp/configure",post(configure)).route("/mcp/connection",post(connection))
        .route("/mcp/clients",get(clients)).route("/mcp/clients/save",post(save))
        .route("/mcp/clients/remove",post(remove)).route("/mcp/clients/connect",post(connect))
        .route("/mcp/clients/disconnect",post(disconnect)).route("/mcp/clients/tools",post(tools))
        .route("/mcp/clients/call",post(call)).route("/plugins/validate",post(validate_plugin))
}
async fn status(State(s):State<Shared>)->Json<ServerStatus>{Json(s.mcp_server.status().await)}
async fn workspace_revisions(State(s):State<Shared>)->Result<Json<Value>,ApiError>{let mut value=s.engine.store.workspace_revisions().map_err(error)?;value["configurationFrozen"]=serde_json::json!(s.engine.store.configuration_ready().is_err());Ok(Json(value))}
#[derive(Deserialize)] struct Configure {config:ServerConfig}
async fn configure(State(s):State<Shared>,Json(p):Json<Configure>)->Result<Json<ServerStatus>,ApiError>{s.ensure_network_ready().map_err(error)?;s.mcp_server.configure(p.config).await.map(Json).map_err(error)}
async fn connection(State(s):State<Shared>)->Result<Json<Value>,ApiError>{s.mcp_server.connection_info().await.map(Json).map_err(error)}
async fn clients(State(s):State<Shared>)->Json<Vec<ClientStatus>>{Json(s.mcp_clients.list().await)}
#[derive(Deserialize)] struct Save {input:ClientInput}
async fn save(State(s):State<Shared>,Json(p):Json<Save>)->Result<Json<ClientStatus>,ApiError>{s.mcp_clients.save(p.input).await.map(Json).map_err(error)}
#[derive(Deserialize)] struct Id {id:String}
async fn remove(State(s):State<Shared>,Json(p):Json<Id>)->Result<Json<()>,ApiError>{s.mcp_clients.remove(p.id).await.map(Json).map_err(error)}
async fn connect(State(s):State<Shared>,Json(p):Json<Id>)->Result<Json<ClientStatus>,ApiError>{s.ensure_network_ready().map_err(error)?;s.mcp_clients.connect(p.id).await.map(Json).map_err(error)}
async fn disconnect(State(s):State<Shared>,Json(p):Json<Id>)->Result<Json<ClientStatus>,ApiError>{s.mcp_clients.disconnect(p.id).await.map(Json).map_err(error)}
async fn tools(State(s):State<Shared>,Json(p):Json<Id>)->Result<Json<Vec<Value>>,ApiError>{s.mcp_clients.tools(p.id).await.map(Json).map_err(error)}
#[derive(Deserialize)] struct Call {id:String,name:String,arguments:Value}
async fn call(State(s):State<Shared>,Json(p):Json<Call>)->Result<Json<Value>,ApiError>{s.ensure_network_ready().map_err(error)?;s.mcp_clients.call(p.id,p.name,p.arguments).await.map(Json).map_err(error)}
#[derive(Deserialize)] struct Validate {plugin:capture_core::plugins::Plugin}
async fn validate_plugin(Json(p):Json<Validate>)->Result<Json<()>,ApiError>{capture_core::plugins::validate_plugin(&p.plugin).map_err(error)?;Ok(Json(()))}
