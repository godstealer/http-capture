use serde::{Deserialize, Serialize};

pub use transport_api::{Header, TlsProfile, CapturedResponse, TlsDetails, CertificateDetails};
pub type RequestDraft = transport_api::RequestDraft<crate::scripts::Scripts>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flow {
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
