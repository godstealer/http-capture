//! An explicitly configured, out-of-process send engine using the helper JSON protocol.
use crate::{http1::prepare_validated_fields, model::RequestDraft, transport::{EngineContract, SendFuture}};
use anyhow::{ensure, Context, Result};
use std::path::PathBuf;

pub struct CustomEngine(transport_httpcloak::Helper);

impl CustomEngine {
    pub fn discover() -> Result<Self> {
        let path = PathBuf::from(std::env::var_os("HTTP_CAPTURE_CUSTOM_ENGINE")
            .context("Set HTTP_CAPTURE_CUSTOM_ENGINE to an absolute executable path")?);
        ensure!(path.is_absolute(), "HTTP_CAPTURE_CUSTOM_ENGINE must be an absolute path");
        let helper = transport_httpcloak::Helper::load(path)?;
        let profiles = &helper.capabilities.profiles;
        ensure!(!profiles.is_empty() && profiles.len() <= 3, "Custom engine must declare 1–3 TLS profiles");
        ensure!(profiles.iter().all(|p| matches!(p.as_str(), "native" | "chrome" | "firefox")),
            "Custom engine has an unsupported TLS profile");
        ensure!(profiles.iter().collect::<std::collections::HashSet<_>>().len() == profiles.len(),
            "Custom engine TLS profiles must be unique");
        ensure!(helper.capabilities.browser_versions.keys().all(|p| profiles.contains(p)),
            "Custom engine browserVersions must match declared profiles");
        Ok(Self(helper))
    }
}

impl EngineContract<RequestDraft, crate::upstream::UpstreamProxy> for CustomEngine {
    fn id(&self) -> &'static str { "custom" }
    fn profiles(&self) -> Vec<String> { self.0.capabilities.profiles.clone() }
    fn browser_versions(&self) -> std::collections::BTreeMap<String, Vec<u16>> {
        self.0.capabilities.browser_versions.clone()
    }
    fn send<'a>(&'a self, draft: &'a RequestDraft) -> SendFuture<'a> { self.send_via(draft, None) }
    fn send_via<'a>(&'a self, draft: &'a RequestDraft,
        proxy: Option<&'a crate::upstream::UpstreamProxy>) -> SendFuture<'a> {
        Box::pin(async move {
            let (_, headers, mut notes) = prepare_validated_fields(draft)?;
            let mut outgoing = draft.clone();
            outgoing.headers = headers;
            let proxy_config = match proxy { Some(p) => Some(p.for_helper().await?), None => None };
            let security = crate::certificates::current().material();
            let (response, extra) = self.0.send_with_security(&outgoing, &draft.tls.preset,
                proxy_config.as_ref(), security.as_ref()).await?;
            ensure!((100..=599).contains(&response.status), "Custom engine returned invalid HTTP status");
            ensure!(!response.version.is_empty() && response.version.len() <= 32,
                "Custom engine returned invalid HTTP version");
            notes.extend(extra);
            Ok((response, notes))
        })
    }
}
