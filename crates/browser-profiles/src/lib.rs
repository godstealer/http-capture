use transport_api::RequestDraft;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;

#[cfg(feature="browser-replay")]
fn profiles(family: &str) -> Vec<(u16, wreq_util::Profile)> {
    let prefix = match family { "chrome" => "Chrome", "firefox" => "Firefox", _ => return vec![] };
    let mut result: Vec<_> = wreq_util::Profile::VARIANTS.iter().filter_map(|p| {
        let name = format!("{p:?}");
        name.strip_prefix(prefix)?.parse::<u16>().ok().map(|version| (version, *p))
    }).collect();
    result.sort_by_key(|(version,_)| *version);
    result
}
pub fn versions() -> BTreeMap<String,Vec<u16>> {
    #[cfg(feature="browser-replay")]
    { ["chrome","firefox"].into_iter().map(|family| (family.to_string(),profiles(family).into_iter().rev().map(|(v,_)|v).collect())).collect() }
    #[cfg(not(feature="browser-replay"))]
    { BTreeMap::new() }
}
pub fn requested<S>(draft:&RequestDraft<S>, supported:&[u16]) -> Result<u16> {
    let setting=draft.tls.browser_version.as_deref().unwrap_or("auto");
    let latest=|| supported.iter().max().copied().context("No browser TLS profiles available");
    let version=match setting {
        "latest"=>return latest(),
        "auto"=>{
            let token=match draft.tls.preset.as_str(){"chrome"=>"Chrome/","firefox"=>"Firefox/",_=>bail!("Unknown browser family")};
            let ua:Vec<_>=draft.headers.iter().filter(|h|h.name.eq_ignore_ascii_case("user-agent")).collect();
            anyhow::ensure!(ua.len()<=1,"Multiple User-Agent headers; select a TLS browser version explicitly");
            match ua.first().and_then(|h|h.value.split_whitespace().find_map(|part|part.strip_prefix(token))) {
                Some(value)=>value.split('.').next().unwrap_or("").parse::<u16>().context("Invalid browser version in User-Agent")?,
                None=>return latest(),
            }
        },
        value=>value.parse::<u16>().context("Invalid TLS browser version")?,
    };
    anyhow::ensure!(supported.contains(&version),"{} {} TLS profile is not available in this build; choose a supported version or latest explicitly (latest: {})",draft.tls.preset,version,latest()?);
    Ok(version)
}
#[cfg(feature="browser-replay")]
pub fn resolve<S>(draft:&RequestDraft<S>)->Result<(u16,wreq_util::Profile)> {
    let available=profiles(&draft.tls.preset);
    let version=requested(draft,&available.iter().map(|(v,_)|*v).collect::<Vec<_>>())?;
    available.into_iter().find(|(v,_)|*v==version).context("Browser profile unavailable")
}
#[cfg(test)]
mod tests {
 use super::*;
 fn draft()->RequestDraft{serde_json::from_value(serde_json::json!({"method":"GET","url":"https://example.com","headers":[{"name":"User-Agent","value":"Mozilla/5.0 Chrome/152.0.0.0 Safari/537.36"}],"bodyBase64":"","tls":{"preset":"chrome"}})).unwrap()}
 #[test] fn ua_override_and_fallback(){
  let mut d=draft();let supported=[147,149,150,152];
  assert_eq!(requested(&d,&supported).unwrap(),152);
  assert!(requested(&d,&[147,149]).is_err());
  d.tls.browser_version=Some("150".into());assert_eq!(requested(&d,&supported).unwrap(),150);
  assert!(d.headers[0].value.contains("152"));
  d.tls.browser_version=Some("latest".into());assert_eq!(requested(&d,&supported).unwrap(),152);
  d.tls.browser_version=None;d.headers.clear();assert_eq!(requested(&d,&supported).unwrap(),152);
  d.tls.browser_version=Some("banana".into());assert!(requested(&d,&supported).is_err());
 }
 #[cfg(feature="browser-replay")]
 #[test] fn catalog_resolves_every_advertised_profile(){for(family,versions)in versions(){for version in versions{let mut d=draft();d.tls.preset=family.clone();d.tls.browser_version=Some(version.to_string());assert_eq!(resolve(&d).unwrap().0,version);}}}
}
