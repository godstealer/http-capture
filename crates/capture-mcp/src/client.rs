//! Explicit MCP client connections. Configuration is durable; credentials and
//! connections are session-only and never restored or invoked automatically.
use anyhow::{ensure, Result};
use capture_core::store::Store;
use rmcp::{
    model::CallToolRequestParams,
    service::{Peer, RunningService},
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig,
        StreamableHttpClientTransport, TokioChildProcess,
    },
    RoleClient, ServiceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, process::Stdio, sync::Arc, time::Duration};
use tokio::{process::Command, sync::Mutex};
use tokio_util::sync::CancellationToken;

const KEY: &str = "mcp.clients.v1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(6);
const CREDENTIAL_NOTICE: &str = "需重新填写 Bearer Token（仅保留本次会话，重启后不会保存）";

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientInput {
    #[serde(default)] pub id: Option<String>,
    pub name: String,
    pub transport: String,
    #[serde(default)] pub url: String,
    #[serde(default)] pub command: String,
    #[serde(default)] pub args: Vec<String>,
    /// None keeps the existing session credential; an empty string clears it.
    #[serde(default)] pub bearer_token: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientStatus {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub url: String,
    pub command: String,
    pub args: Vec<String>,
    pub has_token: bool,
    pub connected: bool,
    pub server_info: Option<Value>,
    pub last_error: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredClient {
    id: String,
    name: String,
    transport: String,
    #[serde(default)] url: String,
    #[serde(default)] command: String,
    #[serde(default)] args: Vec<String>,
    #[serde(default)] token_required: bool,
}

struct Session {
    service: RunningService<RoleClient, ()>,
    cancel: CancellationToken,
    generation: String,
}
impl Session {
    fn connected(&self) -> bool {
        !self.cancel.is_cancelled() && !self.service.is_closed() && !self.service.peer().is_transport_closed()
    }
}
struct PendingConnection { generation: String, cancel: CancellationToken }
#[derive(Default)]
struct State {
    entries: Vec<StoredClient>,
    tokens: HashMap<String, String>,
    sessions: HashMap<String, Session>,
    connecting: HashMap<String, PendingConnection>,
    errors: HashMap<String, String>,
    stopping: bool,
}

pub struct ClientManager {
    store: Arc<Store>, state: Mutex<State>, lifecycle: Mutex<()>,
    connection_tasks: tokio::sync::RwLock<()>,
}

impl ClientManager {
    pub fn new(store: Arc<Store>) -> Result<Arc<Self>> {
        // MCP clients can be created without CaptureRuntime/Engine. Reqwest's
        // rustls-no-provider backend requires a process default even for HTTP
        // URLs because ClientBuilder prepares its HTTPS connector eagerly.
        // Keep a previously selected provider; otherwise use core's ring choice.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let entries: Vec<StoredClient> = store.setting(KEY)?.unwrap_or_default();
        ensure!(entries.len() <= 32, "最多保存 32 个外部 MCP 服务");
        let mut ids = std::collections::HashSet::new();
        for entry in &entries {
            validate(entry, None)?;
            ensure!(ids.insert(entry.id.clone()), "MCP 服务 ID 重复");
        }
        Ok(Arc::new(Self { store, state: Mutex::new(State { entries, ..Default::default() }), lifecycle:Mutex::new(()), connection_tasks:tokio::sync::RwLock::new(()) }))
    }

    pub async fn list(&self) -> Vec<ClientStatus> {
        let state = self.state.lock().await;
        state.entries.iter().map(|entry| status(&state, entry)).collect()
    }

    pub async fn save(&self, input: ClientInput) -> Result<ClientStatus> {
        let mut state = self.state.lock().await;
        ensure!(!state.stopping, "MCP 客户端正在关闭");
        let existing = input.id.as_ref().and_then(|id| state.entries.iter().find(|item| &item.id == id));
        ensure!(input.id.is_none() || existing.is_some(), "MCP 服务不存在");
        ensure!(existing.is_some() || state.entries.len() < 32, "最多保存 32 个外部 MCP 服务");
        let id = input.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let keep_token = input.bearer_token.is_none();
        let token = match input.bearer_token {
            Some(value) if value.is_empty() => None,
            Some(value) => Some(value),
            None => state.tokens.get(&id).cloned(),
        };
        let token_required = if input.transport == "http" {
            token.is_some() || (existing.is_some_and(|entry| entry.token_required) && keep_token)
        } else { false };
        let entry = StoredClient {
            id: id.clone(), name: input.name.trim().to_owned(), transport: input.transport,
            url: input.url.trim().to_owned(), command: input.command.trim().to_owned(), args: input.args,
            token_required,
        };
        validate(&entry, token.as_deref())?;
        ensure!(!state.entries.iter().any(|other| other.id != id && other.name == entry.name), "MCP 服务名称已存在");
        let mut next = state.entries.clone();
        if let Some(position) = next.iter().position(|item| item.id == id) { next[position] = entry.clone(); }
        else { next.push(entry.clone()); }
        // No secret is included in StoredClient, even if persistence fails.
        self.store.save_setting(KEY, &next)?;
        state.entries = next;
        state.errors.remove(&id);
        if entry.transport == "http" {
            if let Some(token) = token { state.tokens.insert(id.clone(), token); }
            else { state.tokens.remove(&id); }
        } else { state.tokens.remove(&id); }
        let session = detach(&mut state, &id);
        let output = status(&state, &entry);
        drop(state);
        close(session).await;
        Ok(output)
    }

    pub async fn remove(&self, id: String) -> Result<()> {
        let mut state = self.state.lock().await;
        ensure!(!state.stopping, "MCP 客户端正在关闭");
        ensure!(state.entries.iter().any(|item| item.id == id), "MCP 服务不存在");
        let next: Vec<_> = state.entries.iter().filter(|item| item.id != id).cloned().collect();
        self.store.save_setting(KEY, &next)?;
        state.entries = next;
        state.tokens.remove(&id);
        state.errors.remove(&id);
        let session = detach(&mut state, &id);
        drop(state);
        close(session).await;
        Ok(())
    }

    pub async fn connect(&self, id: String) -> Result<ClientStatus> {
        let _connecting = self.connection_tasks.read().await;
        let (entry, token, generation, cancel, old) = {
            let mut state = self.state.lock().await;
            ensure!(!state.stopping, "MCP 客户端正在关闭");
            let entry = state.entries.iter().find(|item| item.id == id).cloned().ok_or_else(|| anyhow::anyhow!("MCP 服务不存在"))?;
            if state.sessions.get(&id).is_some_and(Session::connected) { return Ok(status(&state, &entry)); }
            if state.connecting.get(&id).is_some_and(|pending| pending.cancel.is_cancelled()) {
                state.connecting.remove(&id);
            }
            ensure!(!state.connecting.contains_key(&id), "MCP 服务正在连接");
            let token = state.tokens.get(&id).cloned();
            ensure!(!entry.token_required || token.is_some(), CREDENTIAL_NOTICE);
            let old = state.sessions.remove(&id);
            let generation = uuid::Uuid::new_v4().to_string();
            let cancel = CancellationToken::new();
            state.connecting.insert(id.clone(), PendingConnection { generation: generation.clone(), cancel: cancel.clone() });
            state.errors.remove(&id);
            (entry, token, generation, cancel, old)
        };
        // If the caller itself is dropped (for example a cancelled HTTP
        // control request), the attempt must not permanently lock this entry.
        let attempt = cancel.clone().drop_guard();
        close(old).await;
        let connection = establish(&entry, token.as_deref(), cancel.clone()).await;
        let mut state = self.state.lock().await;
        let current = state.connecting.get(&id).is_some_and(|pending| pending.generation == generation);
        if !current || cancel.is_cancelled() || state.stopping {
            drop(state);
            if let Ok(service) = connection { close(Some(Session { service, cancel, generation })).await; }
            anyhow::bail!("MCP 连接已取消");
        }
        state.connecting.remove(&id);
        match connection {
            Ok(service) => {
                state.sessions.insert(id.clone(), Session { service, cancel, generation });
                state.errors.remove(&id);
                let _ = attempt.disarm();
                Ok(status(&state, &entry))
            }
            Err(error) => {
                // Errors are deliberately local messages: a remote response or
                // SDK error may reflect an Authorization header or URL secret.
                state.errors.insert(id, error.to_string());
                Err(error)
            }
        }
    }

    pub async fn disconnect(&self, id: String) -> Result<ClientStatus> {
        let mut state = self.state.lock().await;
        let entry = state.entries.iter().find(|item| item.id == id).cloned().ok_or_else(|| anyhow::anyhow!("MCP 服务不存在"))?;
        let session = detach(&mut state, &id);
        state.errors.remove(&id);
        let output = status(&state, &entry);
        drop(state);
        close(session).await;
        Ok(output)
    }

    pub async fn tools(&self, id: String) -> Result<Vec<Value>> {
        let (peer, cancel, generation) = self.peer(&id).await?;
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(anyhow::anyhow!("MCP 连接已断开")),
            result = tokio::time::timeout(CONNECT_TIMEOUT, peer.list_all_tools()) => match result {
                Ok(Ok(tools)) => tools.into_iter().map(|tool| serde_json::to_value(tool).map_err(|_| anyhow::anyhow!("MCP 工具信息无法解析"))).collect(),
                Ok(Err(_)) => Err(anyhow::anyhow!("读取 MCP 工具目录失败，请检查服务状态")),
                Err(_) => Err(anyhow::anyhow!("读取 MCP 工具目录超时（15 秒）")),
            }
        };
        self.record_error(&id, &generation, result.as_ref().err().map(ToString::to_string)).await;
        result
    }

    pub async fn call(&self, id: String, name: String, arguments: Value) -> Result<Value> {
        ensure!(!name.is_empty() && name.len() <= 512 && !name.chars().any(char::is_control), "MCP 工具名称无效");
        let arguments = arguments.as_object().cloned().ok_or_else(|| anyhow::anyhow!("工具参数必须为 JSON 对象"))?;
        ensure!(serde_json::to_vec(&arguments)?.len() <= 1024 * 1024, "MCP 工具参数最多 1 MiB");
        let (peer, cancel, generation) = self.peer(&id).await?;
        let params = CallToolRequestParams::new(name).with_arguments(arguments);
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(anyhow::anyhow!("MCP 连接已断开，已取消等待工具结果；远端操作可能已经执行")),
            result = tokio::time::timeout(CALL_TIMEOUT, peer.call_tool(params)) => match result {
                Ok(Ok(result)) => serde_json::to_value(result).map_err(|_| anyhow::anyhow!("MCP 工具结果无法解析")),
                Ok(Err(_)) => Err(anyhow::anyhow!("MCP 工具调用失败，请检查服务状态；远端操作可能已经执行")),
                Err(_) => Err(anyhow::anyhow!("MCP 工具调用超时（60 秒）；远端操作可能已经执行，请勿盲目重复调用")),
            }
        };
        self.record_error(&id, &generation, result.as_ref().err().map(ToString::to_string)).await;
        result
    }

    pub async fn shutdown(&self) -> Result<()> {
        let _lifecycle = self.lifecycle.lock().await;
        let sessions = {
            let mut state = self.state.lock().await;
            state.stopping = true;
            for (_, pending) in state.connecting.drain() { pending.cancel.cancel(); }
            state.tokens.clear();
            state.sessions.drain().map(|(_, session)| { session.cancel.cancel(); session }).collect::<Vec<_>>()
        };
        // Signal all connections before waiting for any individual cleanup.
        let mut closing = tokio::task::JoinSet::new();
        for session in sessions { closing.spawn(close(Some(session))); }
        while closing.join_next().await.is_some() {}
        // Cancelled initialization/catalog futures also own SDK transports.
        // Wait for those owners to finish cleanup before allowing resume.
        let _connections = self.connection_tasks.write().await;
        Ok(())
    }

    /// Resume after a cancelled/failed application update. Configuration stays
    /// intact, credentials remain cleared, and no connection is restarted.
    pub async fn resume(&self) -> Result<()> {
        let _lifecycle = self.lifecycle.lock().await;
        let _connections = self.connection_tasks.write().await;
        let mut state = self.state.lock().await;
        ensure!(state.sessions.is_empty() && state.connecting.is_empty(), "旧 MCP 会话尚未关闭");
        state.stopping = false;
        Ok(())
    }

    async fn peer(&self, id: &str) -> Result<(Peer<RoleClient>, CancellationToken, String)> {
        let state = self.state.lock().await;
        let session = state.sessions.get(id).filter(|session| session.connected()).ok_or_else(|| anyhow::anyhow!("请先连接 MCP 服务"))?;
        Ok((session.service.peer().clone(), session.cancel.clone(), session.generation.clone()))
    }

    async fn record_error(&self, id: &str, generation: &str, error: Option<String>) {
        let mut state = self.state.lock().await;
        if state.sessions.get(id).is_some_and(|session| session.generation == generation) {
            if let Some(error) = error { state.errors.insert(id.to_owned(), error); }
            else { state.errors.remove(id); }
        }
    }
}

fn validate(entry: &StoredClient, token: Option<&str>) -> Result<()> {
    ensure!(!entry.id.is_empty() && entry.id.len() <= 128 && entry.id.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)), "MCP 服务 ID 无效");
    ensure!(!entry.name.is_empty() && entry.name.len() <= 160 && !entry.name.chars().any(char::is_control), "MCP 名称需为 1–160 字节");
    if let Some(token) = token {
        ensure!(token.len() <= 8192 && token.bytes().all(|c| matches!(c, 0x21..=0x7e)), "Bearer Token 格式无效");
    }
    match entry.transport.as_str() {
        "http" => {
            let url = url::Url::parse(&entry.url).map_err(|_| anyhow::anyhow!("请输入有效的 MCP HTTP URL"))?;
            ensure!(entry.url.len() <= 4096 && !entry.url.chars().any(char::is_control), "MCP URL 无效或过长");
            ensure!(matches!(url.scheme(), "http" | "https") && url.host_str().is_some(), "MCP 地址必须使用 http:// 或 https://");
            ensure!(url.username().is_empty() && url.password().is_none() && url.fragment().is_none(), "MCP URL 不能包含用户名、密码或 fragment");
            ensure!(entry.command.is_empty() && entry.args.is_empty(), "HTTP 服务不能同时设置本地命令");
        }
        "stdio" => {
            ensure!(entry.url.is_empty() && token.is_none() && !entry.token_required, "stdio 服务不使用 HTTP URL 或 Bearer Token");
            ensure!(!entry.command.is_empty() && entry.command.len() <= 4096 && !entry.command.chars().any(char::is_control), "请输入有效的本地可执行程序");
            ensure!(entry.args.len() <= 128 && entry.args.iter().all(|arg| arg.len() <= 16384 && !arg.contains('\0')) && entry.args.iter().map(String::len).sum::<usize>() <= 65536, "MCP 命令参数过多、过长或包含 NUL");
        }
        _ => anyhow::bail!("MCP 传输类型必须为 http 或 stdio"),
    }
    Ok(())
}

fn status(state: &State, entry: &StoredClient) -> ClientStatus {
    let session = state.sessions.get(&entry.id).filter(|session| session.connected());
    let has_token = state.tokens.contains_key(&entry.id);
    let last_error = state.errors.get(&entry.id).cloned().or_else(|| {
        if entry.token_required && !has_token { Some(CREDENTIAL_NOTICE.to_owned()) }
        else if state.sessions.contains_key(&entry.id) && session.is_none() { Some("MCP 服务连接已关闭".to_owned()) }
        else { None }
    });
    ClientStatus {
        id: entry.id.clone(), name: entry.name.clone(), transport: entry.transport.clone(),
        url: entry.url.clone(), command: entry.command.clone(), args: entry.args.clone(), has_token,
        connected: session.is_some(),
        server_info: session.and_then(|session| session.service.peer_info()).and_then(|info| serde_json::to_value(info.as_ref()).ok()),
        last_error,
    }
}

fn detach(state: &mut State, id: &str) -> Option<Session> {
    if let Some(pending) = state.connecting.remove(id) { pending.cancel.cancel(); }
    let session = state.sessions.remove(id);
    if let Some(session) = &session { session.cancel.cancel(); }
    session
}

async fn close(session: Option<Session>) {
    if let Some(mut session) = session {
        session.cancel.cancel();
        let _ = session.service.close_with_timeout(CLOSE_TIMEOUT).await;
    }
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .build().map_err(|_| anyhow::anyhow!("无法创建 MCP HTTP 客户端"))
}

async fn establish(entry: &StoredClient, token: Option<&str>, cancel: CancellationToken) -> Result<RunningService<RoleClient, ()>> {
    validate(entry, token)?;
    let connect = async {
        if entry.transport == "http" {
            let client = http_client()?;
            let mut config = StreamableHttpClientTransportConfig::with_uri(entry.url.clone())
                // A retried tool may have side effects. Session expiry is shown
                // to the user rather than transparently replaying a call.
                .reinit_on_expired_session(false);
            if let Some(token) = token { config = config.auth_header(token.to_owned()); }
            let transport = StreamableHttpClientTransport::with_client(client, config);
            ().serve(transport).await.map_err(|_| anyhow::anyhow!("MCP 初始化失败，请检查地址、认证信息和服务状态"))
        } else {
            let mut command = Command::new(&entry.command);
            command.args(&entry.args).kill_on_drop(true);
            #[cfg(windows)] command.creation_flags(0x08000000);
            let (transport, _) = TokioChildProcess::builder(command).stderr(Stdio::null()).spawn()
                .map_err(|_| anyhow::anyhow!("无法启动 MCP 本地程序，请检查可执行路径和参数；不会通过 shell 执行命令"))?;
            ().serve(transport).await.map_err(|_| anyhow::anyhow!("MCP 本地程序初始化失败，请检查程序是否支持 stdio MCP"))
        }
    };
    let mut service = tokio::select! {
        biased;
        _ = cancel.cancelled() => anyhow::bail!("MCP 连接已取消"),
        result = tokio::time::timeout(CONNECT_TIMEOUT, connect) => result.map_err(|_| anyhow::anyhow!("MCP 初始化超时（15 秒）"))??,
    };
    // Fetch only the directory; connecting never executes a remote tool.
    let catalog = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(anyhow::anyhow!("MCP 连接已取消")),
        result = tokio::time::timeout(CONNECT_TIMEOUT, service.peer().list_all_tools()) => match result {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(_)) => Err(anyhow::anyhow!("MCP 已初始化，但读取工具目录失败")),
            Err(_) => Err(anyhow::anyhow!("读取 MCP 工具目录超时（15 秒）")),
        },
    };
    if let Err(error) = catalog {
        let _ = service.close_with_timeout(CLOSE_TIMEOUT).await;
        return Err(error);
    }
    Ok(service)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Request, middleware::{self, Next}, response::IntoResponse, routing::any, Router};
    use rmcp::{
        model::{CallToolResponse, CallToolResult, ContentBlock, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool},
        service::RequestContext,
        transport::streamable_http_server::{session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService},
        ErrorData, RoleServer, ServerHandler,
    };
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn http_input(url: String) -> ClientInput {
        ClientInput {id:None,name:"fixture".into(),transport:"http".into(),url,command:String::new(),args:vec![],bearer_token:None}
    }
    fn store() -> (tempfile::TempDir, Arc<Store>) {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("sessions.db")).unwrap());
        (directory, store)
    }

    #[test]
    fn isolated_http_client_initializes_tls_provider() {
        // A fresh filtered process is essential: core's parallel tests also
        // install a provider and used to hide the standalone client panic.
        let output=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","client::tests::http_provider_child_fixture","--nocapture"])
            .env("HTTP_CAPTURE_MCP_TLS_TEST_CHILD","1").output().unwrap();
        assert!(output.status.success(),"isolated MCP TLS regression failed:\n{}\n{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
    }

    #[test]
    fn http_provider_child_fixture() {
        if std::env::var("HTTP_CAPTURE_MCP_TLS_TEST_CHILD").as_deref()!=Ok("1") { return; }
        assert!(rustls::crypto::CryptoProvider::get_default().is_none(),"fixture must start without an Engine-installed provider");
        let runtime=tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (_directory,store)=store();
            let _manager=ClientManager::new(store).unwrap();
            assert!(rustls::crypto::CryptoProvider::get_default().is_some());
            http_client().expect("standalone MCP ClientManager must prepare its own TLS provider");
        });
    }

    #[tokio::test]
    async fn durable_configuration_never_serializes_credentials_or_auto_connects() {
        let (_directory, store) = store();
        let manager = ClientManager::new(store.clone()).unwrap();
        let mut input = http_input("http://127.0.0.1:1/mcp".into());
        input.bearer_token = Some("private-fixture-token-007".into());
        let saved = manager.save(input).await.unwrap();
        assert!(!saved.connected);
        assert!(saved.has_token);
        assert!(!serde_json::to_string(&saved).unwrap().contains("private-fixture-token-007"));
        let raw: Value = store.setting(KEY).unwrap().unwrap();
        assert!(!raw.to_string().contains("private-fixture-token-007"));
        assert_eq!(raw[0]["tokenRequired"], true);
        let reopened = ClientManager::new(store).unwrap();
        let after_restart = reopened.list().await;
        assert!(!after_restart[0].connected && !after_restart[0].has_token);
        assert_eq!(after_restart[0].last_error.as_deref(), Some(CREDENTIAL_NOTICE));
        assert!(reopened.connect(saved.id.clone()).await.unwrap_err().to_string().contains("Bearer Token"));
        let mut update = http_input(saved.url.clone());
        update.id = Some(saved.id.clone());
        assert!(manager.save(update.clone()).await.unwrap().has_token);
        update.bearer_token = Some(String::new());
        assert!(!manager.save(update).await.unwrap().has_token);
        manager.remove(saved.id).await.unwrap();
        assert!(manager.list().await.is_empty());
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn invalid_configuration_and_non_object_arguments_have_no_side_effects() {
        let (_directory, store) = store();
        let manager = ClientManager::new(store.clone()).unwrap();
        for url in ["file:///etc/passwd", "https://user:pass@example.invalid/mcp", "https://example.invalid/mcp#fragment", "ws://example.invalid/mcp"] {
            assert!(manager.save(http_input(url.into())).await.is_err());
        }
        let mut input = http_input("https://example.invalid/mcp".into());
        input.bearer_token = Some("line1\r\nInjected: header".into());
        assert!(manager.save(input).await.is_err());
        assert!(manager.list().await.is_empty());
        assert!(store.setting::<Value>(KEY).unwrap().is_none());
        assert!(manager.call("missing".into(), "tool".into(), json!([])).await.unwrap_err().to_string().contains("JSON 对象"));
    }

    #[derive(Clone)]
    struct EchoServer { calls: Arc<AtomicUsize>, started: Arc<tokio::sync::Notify> }
    impl ServerHandler for EchoServer {
        fn get_info(&self) -> ServerConfig { ServerConfig::new(ServerCapabilities::builder().enable_tools().build()) }
        async fn list_tools(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListToolsResult, ErrorData> {
            let tool: Tool = serde_json::from_value(json!({"name":"echo","description":"Local test only","inputSchema":{"type":"object"}})).unwrap();
            Ok(ListToolsResult::with_all_items(vec![tool]))
        }
        async fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            if request.arguments.as_ref().is_some_and(|args|args.get("wait")==Some(&Value::Bool(true))) { context.ct.cancelled().await; }
            Ok(CallToolResult::success(vec![ContentBlock::text(serde_json::to_string(&request.arguments).unwrap())]).into())
        }
    }

    #[tokio::test]
    async fn official_http_sdk_roundtrip_and_disconnect_cancel_inflight_work() {
        let (_directory, store) = store();
        let manager = ClientManager::new(store).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let server = EchoServer {calls:calls.clone(), started:started.clone()};
        let transport = StreamableHttpService::new(move||Ok(server.clone()), Arc::new(LocalSessionManager::default()), StreamableHttpServerConfig::default());
        let auth_requests = Arc::new(AtomicUsize::new(0));
        let observed_auth = auth_requests.clone();
        let app = Router::new().nest_service("/mcp",transport).layer(middleware::from_fn(move|request:Request,next:Next| {
            let observed_auth=observed_auth.clone();
            async move {
                if request.headers().get("authorization").and_then(|v|v.to_str().ok())!=Some("Bearer fixture-session-secret") {
                    return axum::http::StatusCode::UNAUTHORIZED.into_response();
                }
                observed_auth.fetch_add(1,Ordering::SeqCst);
                next.run(request).await
            }
        }));
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address=listener.local_addr().unwrap();
        let http=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
        let mut input=http_input(format!("http://{address}/mcp"));
        input.bearer_token=Some("fixture-session-secret".into());
        let saved=manager.save(input).await.unwrap();
        assert_eq!(auth_requests.load(Ordering::SeqCst),0,"saving must not connect");
        let connected=manager.connect(saved.id.clone()).await.unwrap();
        assert!(connected.connected&&connected.server_info.is_some());
        let tools=manager.tools(saved.id.clone()).await.unwrap();
        assert_eq!(tools[0]["name"],"echo");
        assert_eq!(calls.load(Ordering::SeqCst),0,"connect/list must not execute a tool");
        let result=manager.call(saved.id.clone(),"echo".into(),json!({"text":"中文 🛰️","number":7})).await.unwrap();
        let echoed:Value=serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(echoed,json!({"text":"中文 🛰️","number":7}));
        assert_eq!(calls.load(Ordering::SeqCst),1);
        // Consume the notification from the first completed call before waiting
        // for the second request to reach the real SDK server.
        started.notified().await;
        let running_manager=manager.clone();
        let id=saved.id.clone();
        let running=tokio::spawn(async move {running_manager.call(id,"echo".into(),json!({"wait":true})).await});
        tokio::time::timeout(Duration::from_secs(5),started.notified()).await.unwrap();
        assert!(!manager.disconnect(saved.id.clone()).await.unwrap().connected);
        assert!(tokio::time::timeout(Duration::from_secs(7),running).await.unwrap().unwrap().is_err());
        assert!(manager.tools(saved.id).await.is_err());
        assert!(auth_requests.load(Ordering::SeqCst)>=3);
        manager.shutdown().await.unwrap();
        http.abort();
    }

    #[tokio::test]
    async fn redirects_never_forward_the_bearer_token() {
        let (_directory, store) = store();
        let manager=ClientManager::new(store).unwrap();
        let hits=Arc::new(AtomicUsize::new(0));
        let hits2=hits.clone();
        let destination=Router::new().route("/stolen",any(move||{let hits=hits2.clone();async move{hits.fetch_add(1,Ordering::SeqCst);axum::http::StatusCode::OK}}));
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stolen=listener.local_addr().unwrap();
        let target=tokio::spawn(async move{axum::serve(listener,destination).await.unwrap()});
        let location=format!("http://{stolen}/stolen");
        let redirect=Router::new().route("/mcp",any(move||{let location=location.clone();async move{(axum::http::StatusCode::TEMPORARY_REDIRECT,[("location",location)]).into_response()}}));
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address=listener.local_addr().unwrap();
        let source=tokio::spawn(async move{axum::serve(listener,redirect).await.unwrap()});
        let mut input=http_input(format!("http://{address}/mcp"));
        input.bearer_token=Some("redirect-secret-must-not-leak".into());
        let saved=manager.save(input).await.unwrap();
        let error=manager.connect(saved.id).await.unwrap_err().to_string();
        assert!(!error.contains("redirect-secret-must-not-leak"));
        assert_eq!(hits.load(Ordering::SeqCst),0);
        manager.shutdown().await.unwrap();source.abort();target.abort();
    }

    // A tiny stdio peer exercises real argv and pipe ownership. The Rust CI
    // job must install its Node dependency explicitly, just as the frontend
    // job does. A separate test-only file diagnoses startup vs pipe stalls;
    // protocol stdout remains JSON-RPC only, and production stderr stays private.
    const STDIO_FIXTURE:&str=r#"
const fs = require('node:fs');
const readline = require('node:readline');
const diagnostic = process.argv[1];
const args = process.argv.slice(2);
let traceCount = 0;
const trace = (event, fields = {}) => {
  if(traceCount++ < 64) fs.appendFileSync(diagnostic, JSON.stringify({event,pid:process.pid,...fields})+'\n');
};
trace('startup', {node:process.version,executable:process.execPath});
process.stdin.on('error', error => trace('stdin-error', {code:error.code || ''}));
process.stdout.on('error', error => trace('stdout-error', {code:error.code || ''}));
process.on('uncaughtException', error => {trace('uncaught', {name:error.name});process.exit(1);});
process.on('exit', code => trace('exit', {code}));
const rl = readline.createInterface({input:process.stdin});
rl.on('line', line => {
  const m = JSON.parse(line);
  trace('received', {method:m.method,id:m.id ?? null});
  if(m.id === undefined) return;
  let result;
  if(m.method === 'initialize') result={protocolVersion:m.params.protocolVersion,capabilities:{tools:{}},serverInfo:{name:'stdio-fixture',version:'1'}};
  else if(m.method === 'tools/list') result={tools:[{name:'echo',inputSchema:{type:'object'}}]};
  else if(m.method === 'tools/call') {
    if(m.params.arguments.wait) return;
    result={content:[{type:'text',text:JSON.stringify({args,input:m.params.arguments})}]};
  } else if(m.method === 'ping') result={};
  else { process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:m.id,error:{code:-32601,message:'Unknown method'}})+'\n'); return; }
  trace('reply-enqueued', {method:m.method,id:m.id});
  process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:m.id,result})+'\n', () => trace('reply-flushed', {method:m.method,id:m.id}));
});
process.stdin.on('end',()=>{trace('stdin-end');process.exit(0);});
trace('ready');
"#;

    async fn connect_stdio_fixture(manager:&ClientManager,id:&str,diagnostic:&std::path::Path,phase:&str)->ClientStatus {
        let started=std::time::Instant::now();
        manager.connect(id.to_owned()).await.unwrap_or_else(|error| {
            let trace=std::fs::read_to_string(diagnostic).map(|trace|trace.chars().take(32768).collect::<String>()).unwrap_or_else(|read_error|format!("No fixture trace: {read_error}"));
            panic!("stdio {phase} failed after {:?}: {error}\nTest-only startup/pipe trace:\n{trace}",started.elapsed());
        })
    }

    #[tokio::test]
    async fn stdio_argv_is_literal_and_disconnect_cancels_calls() {
        let (directory, store) = store();
        let manager=ClientManager::new(store).unwrap();
        let diagnostic=directory.path().join("stdio fixture 中文.jsonl");
        let literal="spaces 中文 ; $(no_shell) & | > file";
        let saved=manager.save(ClientInput{id:None,name:"stdio fixture".into(),transport:"stdio".into(),url:String::new(),command:"node".into(),args:vec!["-e".into(),STDIO_FIXTURE.into(),diagnostic.to_str().unwrap().into(),literal.into()],bearer_token:None}).await.unwrap();
        assert!(!saved.connected);
        assert!(connect_stdio_fixture(&manager,&saved.id,&diagnostic,"initial connect").await.connected);
        assert_eq!(manager.tools(saved.id.clone()).await.unwrap()[0]["name"],"echo");
        let result=manager.call(saved.id.clone(),"echo".into(),json!({"message":"hello"})).await.unwrap();
        let echoed:Value=serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(echoed["args"],json!([literal]));
        assert_eq!(echoed["input"]["message"],"hello");
        let running_manager=manager.clone();let id=saved.id.clone();
        let running=tokio::spawn(async move{running_manager.call(id,"echo".into(),json!({"wait":true})).await});
        // A yield gives the call a chance to enter the SDK; either cancellation
        // before enqueue or cancellation while awaiting is a valid outcome.
        tokio::task::yield_now().await;
        manager.disconnect(saved.id.clone()).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(7),running).await.unwrap().unwrap().is_err());
        assert!(!manager.list().await[0].connected);
        assert!(connect_stdio_fixture(&manager,&saved.id,&diagnostic,"reconnect after disconnect").await.connected);
        manager.shutdown().await.unwrap();
        assert!(!manager.list().await[0].connected);
        assert!(manager.connect(saved.id.clone()).await.is_err());
        manager.resume().await.unwrap();
        assert!(!manager.list().await[0].connected,"resume must not reconnect automatically");
        assert!(connect_stdio_fixture(&manager,&saved.id,&diagnostic,"connect after resume").await.connected);
        manager.shutdown().await.unwrap();
        let trace=std::fs::read_to_string(&diagnostic).unwrap();
        let events=trace.lines().map(|line|serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
        assert_eq!(events.iter().filter(|event|event["event"]=="startup").count(),3,"each explicit reconnect must launch its own real stdio child");
        assert_eq!(events.iter().filter(|event|event["event"]=="reply-flushed"&&event["method"]=="initialize").count(),3,"every child must receive and flush the real initialization response");
    }
}
