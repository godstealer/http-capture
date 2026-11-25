//! An explicitly configured, out-of-process send engine using the helper JSON protocol.
use crate::{http1::prepare_validated_fields, model::RequestDraft, transport::{EngineContract, SendFuture}};
use anyhow::{ensure, Context, Result};
use std::path::PathBuf;

pub struct CustomEngine {
    id: &'static str,
    helper: transport_httpcloak::Helper,
}

fn configured_helper(variable: &str) -> Result<transport_httpcloak::Helper> {
    let path = PathBuf::from(std::env::var_os(variable)
        .with_context(|| format!("Set {variable} to an absolute executable path"))?);
    ensure!(path.is_absolute(), "{variable} must be an absolute path");
    transport_httpcloak::Helper::load(path)
}

impl CustomEngine {
    pub fn discover() -> Result<Self> {
        let helper = configured_helper("HTTP_CAPTURE_CUSTOM_ENGINE")?;
        let profiles = &helper.capabilities.profiles;
        ensure!(!profiles.is_empty() && profiles.len() <= 3, "Custom engine must declare 1–3 TLS profiles");
        ensure!(profiles.iter().all(|p| matches!(p.as_str(), "native" | "chrome" | "firefox")),
            "Custom engine has an unsupported TLS profile");
        ensure!(profiles.iter().collect::<std::collections::HashSet<_>>().len() == profiles.len(),
            "Custom engine TLS profiles must be unique");
        ensure!(helper.capabilities.browser_versions.keys().all(|p| profiles.contains(p)),
            "Custom engine browserVersions must match declared profiles");
        Ok(Self { id: "custom", helper })
    }

    pub fn discover_xhttp() -> Result<Self> {
        let helper = configured_helper("HTTP_CAPTURE_XHTTP_ENGINE")?;
        let capabilities = &helper.capabilities;
        ensure!(capabilities.profiles == ["native"] && capabilities.browser_versions.is_empty()
            && !capabilities.custom_client_hello,
            "xhttp requires a native-only helper manifest");
        Ok(Self { id: "xhttp", helper })
    }
}

impl EngineContract<RequestDraft, crate::upstream::UpstreamProxy> for CustomEngine {
    fn id(&self) -> &'static str { self.id }
    fn profiles(&self) -> Vec<String> { self.helper.capabilities.profiles.clone() }
    fn browser_versions(&self) -> std::collections::BTreeMap<String, Vec<u16>> {
        self.helper.capabilities.browser_versions.clone()
    }
    fn send<'a>(&'a self, draft: &'a RequestDraft) -> SendFuture<'a> { self.send_via(draft, None) }
    fn send_via<'a>(&'a self, draft: &'a RequestDraft,
        proxy: Option<&'a crate::upstream::UpstreamProxy>) -> SendFuture<'a> {
        Box::pin(async move {
            let (url, headers, mut notes) = prepare_validated_fields(draft)?;
            let mut outgoing = draft.clone();
            outgoing.headers = headers;
            if self.id == "xhttp" {
                outgoing.url = url.to_string();
            }
            let proxy_config = match proxy { Some(p) => Some(p.for_helper().await?), None => None };
            let security = crate::certificates::current().material();
            let (response, extra) = self.helper.send_with_security(&outgoing, &draft.tls.preset,
                proxy_config.as_ref(), security.as_ref()).await?;
            let label = if self.id == "custom" { "Custom" } else { self.id };
            ensure!((100..=599).contains(&response.status), "{label} engine returned invalid HTTP status");
            ensure!(!response.version.is_empty() && response.version.len() <= 32,
                "{label} engine returned invalid HTTP version");
            notes.extend(extra);
            Ok((response, notes))
        })
    }
}
