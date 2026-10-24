use serde::{Deserialize, Serialize};

pub use transport_api::{Header, TlsProfile, CapturedResponse, TlsDetails, CertificateDetails, SharedBody};
pub type RequestDraft = transport_api::RequestDraft<crate::scripts::Scripts>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationInfo {
    pub uid: u32,
    pub packages: Vec<String>,
    pub label: Option<String>,
}

/// Immediate capture ingress, observed locally; never inferred from HTTP headers.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrafficOrigin {
    pub entry: String,
    pub client_address: Option<String>,
    pub process: Option<ProcessInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<ApplicationInfo>,
    pub process_status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic_origin: Option<TrafficOrigin>,
    /// Explicit lifecycle; old files deserialize to unknown rather than appearing pending.
    #[serde(default = "unknown_flow_state")]
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket: Option<crate::websocket::Session>,
    #[serde(default)]
    pub original_request: Option<RequestDraft>,
    #[serde(default)]
    pub original_response: Option<CapturedResponse>,
    #[serde(default)]
    pub client_tls: Option<TlsDetails>,
    #[serde(default)]
    pub client_protocol: Option<String>,
    pub id: String,
    pub parent_id: Option<String>,
    pub started_at: u64,
    pub duration_ms: u64,
    pub source: String,
    pub request: RequestDraft,
    pub response: Option<CapturedResponse>,
    pub error: Option<String>,
    pub raw_request_head_base64: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyStatus {
    #[serde(default)]
    pub upstream: crate::upstream::UpstreamStatus,
    pub send_engines: Vec<crate::transport::EngineInfo>,
    pub running: bool,
    pub address: Option<String>,
    pub ca_path: String,
    pub browser_replay: bool,
}

fn unknown_flow_state() -> String { "unknown".into() }
