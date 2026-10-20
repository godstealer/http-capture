//! A bounded local ledger for authenticated MCP write attempts.
//!
//! This is deliberately not a request logger: no argument/response payload,
//! arbitrary error message, URL, header, script, token or client display name
//! can be supplied to an entry. A start must persist before the operation runs;
//! completion is a separate transaction, so a pending entry means unknown,
//! never evidence that an operation failed or is safe to retry.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const RETENTION_LIMIT: usize = 10_000;
pub const MAX_PAGE_SIZE: usize = 100;

pub fn initialize(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS mcp_operation_audit (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        started_at INTEGER NOT NULL,
        finished_at INTEGER,
        actor TEXT NOT NULL CHECK(actor='mcpTokenHolder'),
        entry_point TEXT NOT NULL CHECK(entry_point='mcpHttp'),
        client_family TEXT NOT NULL,
        client_ref TEXT,
        operation TEXT NOT NULL,
        target_kind TEXT NOT NULL,
        target_id TEXT,
        outcome TEXT NOT NULL CHECK(outcome IN ('pending','succeeded','failed','rejected','cancelled')),
        reason TEXT
    );")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AuditClientFamily { Unknown, Codex, Cursor, VsCode, Claude, Other }
impl AuditClientFamily {
    pub(crate) fn as_str(self) -> &'static str {
        match self { Self::Unknown=>"unknown",Self::Codex=>"codex",Self::Cursor=>"cursor",Self::VsCode=>"vsCode",Self::Claude=>"claude",Self::Other=>"other" }
    }
}

#[derive(Clone, Debug)]
pub struct AuditClient {
    pub(crate) family: AuditClientFamily,
    pub(crate) reference: Option<String>,
}
impl AuditClient {
    pub fn unknown() -> Self { Self { family: AuditClientFamily::Unknown, reference: None } }
    /// Self-reported metadata is a label, not authentication. Retain only a
    /// fixed family and a domain-separated digest, never the original strings.
    pub fn from_reported(name: &str, version: &str) -> Self {
        if name.trim().is_empty() { return Self::unknown(); }
        let bounded_name = &name.as_bytes()[..name.len().min(256)];
        let lower = String::from_utf8_lossy(bounded_name).to_ascii_lowercase();
        let family = if lower.contains("codex") { AuditClientFamily::Codex }
            else if lower.contains("cursor") { AuditClientFamily::Cursor }
            else if lower.contains("visual studio code") || lower.contains("vscode") || lower.contains("vs code") { AuditClientFamily::VsCode }
            else if lower.contains("claude") { AuditClientFamily::Claude }
            else { AuditClientFamily::Other };
        let mut hash = Sha256::new();
        hash.update(b"http-capture/mcp-audit/client/v1\0");
        hash.update((bounded_name.len() as u64).to_le_bytes());
        hash.update(bounded_name);
        let bounded_version = &version.as_bytes()[..version.len().min(128)];
        hash.update((bounded_version.len() as u64).to_le_bytes());
        hash.update(bounded_version);
        Self { family, reference: Some(format!("{:x}", hash.finalize())) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOperation {
    SendRequest, CancelRequest, StartCapture, StopCapture,
    CreateCollection, CreateRequest, UpdateRequest, SaveRequestToCollection,
    ConfigureInterception, ResolveInterception, SetGlobalScripts, SetGlobalScriptsEnabled,
}
impl AuditOperation {
    pub fn from_tool_name(name: &str) -> Option<Self> {
        Some(match name {
            "send_request"=>Self::SendRequest,"cancel_request"=>Self::CancelRequest,
            "start_capture"=>Self::StartCapture,"stop_capture"=>Self::StopCapture,
            "create_collection"=>Self::CreateCollection,"create_request"=>Self::CreateRequest,
            "update_request"=>Self::UpdateRequest,"save_request_to_collection"=>Self::SaveRequestToCollection,
            "configure_interception"=>Self::ConfigureInterception,"resolve_interception"=>Self::ResolveInterception,
            "set_global_scripts"=>Self::SetGlobalScripts,"set_global_scripts_enabled"=>Self::SetGlobalScriptsEnabled,
            _=>return None,
        })
    }
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SendRequest=>"send_request",Self::CancelRequest=>"cancel_request",Self::StartCapture=>"start_capture",Self::StopCapture=>"stop_capture",
            Self::CreateCollection=>"create_collection",Self::CreateRequest=>"create_request",Self::UpdateRequest=>"update_request",Self::SaveRequestToCollection=>"save_request_to_collection",
            Self::ConfigureInterception=>"configure_interception",Self::ResolveInterception=>"resolve_interception",Self::SetGlobalScripts=>"set_global_scripts",Self::SetGlobalScriptsEnabled=>"set_global_scripts_enabled",
        }
    }
    pub(crate) fn target_kind(self) -> &'static str {
        match self {
            Self::SendRequest|Self::CancelRequest=>"request",
            Self::StartCapture|Self::StopCapture=>"listener",
            Self::CreateCollection=>"collection",
            Self::CreateRequest|Self::UpdateRequest|Self::SaveRequestToCollection=>"requestEditor",
            Self::ConfigureInterception=>"interceptionConfig",
            Self::ResolveInterception=>"pendingInterception",
            Self::SetGlobalScripts|Self::SetGlobalScriptsEnabled=>"globalScripts",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AuditOutcome { Pending, Succeeded, Failed, Rejected, Cancelled }
impl AuditOutcome {
    pub(crate) fn as_str(self) -> &'static str { match self { Self::Pending=>"pending",Self::Succeeded=>"succeeded",Self::Failed=>"failed",Self::Rejected=>"rejected",Self::Cancelled=>"cancelled" } }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AuditReason { PermissionDenied, InvalidArguments, Conflict, NotFound, Cancelled, OperationFailed, Unavailable, ResponseLimit, Internal }
impl AuditReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self { Self::PermissionDenied=>"permissionDenied",Self::InvalidArguments=>"invalidArguments",Self::Conflict=>"conflict",Self::NotFound=>"notFound",Self::Cancelled=>"cancelled",Self::OperationFailed=>"operationFailed",Self::Unavailable=>"unavailable",Self::ResponseLimit=>"responseLimit",Self::Internal=>"internal" }
    }
}
fn canonical_id(value: Option<&str>) -> Option<String> {
    value.and_then(|id| uuid::Uuid::parse_str(id).ok()).map(|id| id.to_string())
}
#[derive(Clone, Debug)]
pub struct AuditStart {
    pub(crate) operation: AuditOperation,
    pub(crate) client: AuditClient,
    pub(crate) target_id: Option<String>,
}
impl AuditStart {
    pub fn new(operation: AuditOperation, client: AuditClient) -> Self { Self { operation, client, target_id: None } }
    pub fn with_target_id(mut self, value: Option<&str>) -> Self { self.target_id=canonical_id(value);self }
}
#[derive(Clone, Debug)]
pub struct AuditCompletion {
    pub(crate) outcome: AuditOutcome,
    pub(crate) reason: Option<AuditReason>,
    pub(crate) target_id: Option<String>,
}
impl AuditCompletion {
    pub fn new(outcome: AuditOutcome) -> Self { Self { outcome, reason: None, target_id: None } }
    pub fn with_reason(mut self, reason: AuditReason) -> Self { self.reason=Some(reason);self }
    pub fn with_target_id(mut self, value: Option<&str>) -> Self { self.target_id=canonical_id(value);self }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct AuditQuery { pub cursor: Option<String>, pub limit: Option<usize> }
impl AuditQuery {
    pub(crate) fn checked(&self) -> Result<(Option<i64>, usize)> {
        let limit=self.limit.unwrap_or(50);
        ensure!((1..=MAX_PAGE_SIZE).contains(&limit), "Audit page limit must be 1-100");
        let cursor=self.cursor.as_deref().map(|value| {
            ensure!(!value.is_empty() && value.len()<=19 && value.bytes().all(|b|b.is_ascii_digit()), "Invalid audit cursor");
            let id=value.parse::<i64>().map_err(|_|anyhow::anyhow!("Invalid audit cursor"))?;
            ensure!(id>0, "Invalid audit cursor");Ok(id)
        }).transpose()?;
        Ok((cursor,limit))
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all="camelCase")]
pub struct AuditEntry {
    pub id: String,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub actor: String,
    pub entry_point: String,
    pub client_family: String,
    pub client_ref: Option<String>,
    pub operation: String,
    pub target_kind: String,
    pub target_id: Option<String>,
    pub outcome: String,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all="camelCase")]
pub struct AuditPage {
    pub entries: Vec<AuditEntry>,
    pub next_cursor: Option<String>,
    pub retained_count: usize,
    pub retention_limit: usize,
}
