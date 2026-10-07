//! Transport data only: no storage, scripts runtime, UI or capture dependencies.
use serde::{Serialize, Deserialize};
/// An ordered sequence, including duplicate names and their original casing.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    /// ISO-8859-1 byte mapping; never decode HTTP/1 values with lossy UTF-8.
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDraft<S = serde_json::Value> {
    /// Explicit per-request upstream profile; None means direct for manual requests.
    #[serde(default)]
    pub upstream_profile_id: Option<String>,
    #[serde(default)]
    pub scripts: S,
    /// Ordered HTTP/2 pseudo headers captured before regular fields.
    #[serde(default)]
    pub pseudo_headers: Vec<Header>,
    #[serde(default = "auto")]
    pub engine: String,
    pub method: String,
    pub url: String,
    pub headers: Vec<Header>,
    pub body_base64: String,
    #[serde(default)]
    pub tls: TlsProfile,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TlsProfile {
    #[serde(default)]
    pub browser_version: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// Browser family, resolved by the selected transport.
    #[serde(default = "native")]
    pub preset: String,
    pub cipher_list: Option<String>,
    pub sigalgs_list: Option<String>,
    pub curves_list: Option<String>,
    pub grease: Option<bool>,
    pub permute_extensions: Option<bool>,
}
fn native() -> String { "native".into() }
fn auto() -> String { "auto".into() }
impl Default for TlsProfile {
    fn default() -> Self {
        Self { browser_version: None, version: None, preset: native(), cipher_list: None, sigalgs_list: None, curves_list: None,
            grease: None, permute_extensions: None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedResponse {
    #[serde(default)]
    pub upstream_tls: Option<TlsDetails>,
    /// Actual upstream TLS version; absent for plaintext and older records.
    #[serde(default)]
    pub tls_version: Option<String>,
    /// HTTP/1 or HTTP/2 fields serialized upstream, including generated/pseudo fields.
    #[serde(default)]
    pub sent_request_headers: Option<Vec<Header>>,
    pub status: u16,
    pub version: String,
    pub headers: Vec<Header>,
    pub body_base64: String,
    pub raw_head_base64: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsDetails {
    pub version: Option<String>,
    pub cipher_suite: Option<String>,
    pub alpn: Option<String>,
    pub server_name: Option<String>,
    pub handshake_kind: Option<String>,
    pub offered_cipher_suites: Vec<String>,
    pub offered_alpn: Vec<String>,
    pub signature_schemes: Vec<String>,
    pub supported_groups: Vec<String>,
    pub certificates: Vec<CertificateDetails>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateDetails {
    pub subject: String,
    pub issuer: String,
    pub serial: String,
    pub not_before: String,
    pub not_after: String,
    pub sha256: String,
    pub dns_names: Vec<String>,
    pub der_base64: String,
    pub parse_error: Option<String>,
}


use anyhow::Result;
use std::{future::Future,pin::Pin};
pub type SendResult = Result<(CapturedResponse, Vec<String>)>;
pub type SendFuture<'a> = Pin<Box<dyn Future<Output = SendResult> + Send + 'a>>;

pub trait SendEngine<R, P>: Send + Sync {
    fn id(&self) -> &'static str;
    fn profiles(&self) -> Vec<String>;
    fn browser_versions(&self) -> std::collections::BTreeMap<String,Vec<u16>> { Default::default() }
    /// Implementations must not mutate the captured draft, follow redirects,
    /// bypass certificate validation, or silently discard unsupported options.
    /// Dropping this future must cancel outstanding work (including child I/O).
    fn send<'a>(&'a self, draft: &'a R) -> SendFuture<'a>;
    fn send_via<'a>(&'a self, draft: &'a R, proxy: Option<&'a P>) -> SendFuture<'a> {
        if proxy.is_none() { return self.send(draft); }
        Box::pin(async { anyhow::bail!("此发送引擎暂不支持上游代理；未回退直连") })
    }
}

/// Ephemeral proxy credentials. Never attach to a flow, log, or command line.
#[derive(Serialize, Deserialize)]
pub struct UpstreamProxyConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}
