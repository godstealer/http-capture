use crate::{http1::prepare, model::*, transport::{EngineContract,SendFuture}};
use std::path::PathBuf;
pub struct HttpcloakEngine(pub transport_httpcloak::Helper);
impl HttpcloakEngine {
 pub fn discover()->anyhow::Result<Self>{
  let path=std::env::var_os("HTTP_CAPTURE_HTTPCLOAK").map(PathBuf::from).unwrap_or_else(||{
   let name=if cfg!(windows){"http-capture-httpcloak.exe"}else{"http-capture-httpcloak"};
   let packaged=std::env::current_exe().ok().and_then(|p|p.parent().map(|p|p.join(name)));
   if let Some(p)=packaged.filter(|p|p.is_file()){p}else{PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.local/httpcloak").join(name)}
  });
  Ok(Self(transport_httpcloak::Helper::load(path)?))
 }
}
impl EngineContract<RequestDraft,crate::upstream::UpstreamProxy> for HttpcloakEngine {
 fn id(&self)->&'static str{"httpcloak"}
 fn profiles(&self)->Vec<String>{self.0.capabilities.browser_versions.keys().cloned().collect()}
 fn browser_versions(&self)->std::collections::BTreeMap<String,Vec<u16>>{self.0.capabilities.browser_versions.clone()}
 fn send<'a>(&'a self,draft:&'a RequestDraft)->SendFuture<'a>{self.send_via(draft,None)}
 fn send_via<'a>(&'a self,draft:&'a RequestDraft,proxy:Option<&'a crate::upstream::UpstreamProxy>)->SendFuture<'a>{Box::pin(async move{
  anyhow::ensure!(draft.tls.version.as_deref().unwrap_or("auto")=="auto"&&draft.tls.cipher_list.is_none()&&draft.tls.sigalgs_list.is_none()&&draft.tls.curves_list.is_none()&&draft.tls.grease.is_none()&&draft.tls.permute_extensions.is_none(),"httpcloak custom TLS overrides are not yet supported; clear overrides to use the selected preset");
  anyhow::ensure!(draft.pseudo_headers.is_empty(),"httpcloak uses preset pseudo-header order; explicit captured pseudo headers are not yet supported");
  let versions=self.0.capabilities.browser_versions.get(&draft.tls.preset).ok_or_else(||anyhow::anyhow!("Unsupported httpcloak browser"))?;
  let version=crate::browser_profiles::requested(draft,versions)?;
  let (_,headers,_,mut notes)=prepare(draft)?;
  let mut outgoing=draft.clone();outgoing.headers=headers;
  let preset=format!("{}-{}-windows",draft.tls.preset,version);
  let proxy_config=match proxy{Some(p)=>Some(p.for_helper().await?),None=>None};
  let (response,extra)=self.0.send(&outgoing,&preset,proxy_config.as_ref()).await?;notes.extend(extra);Ok((response,notes))
 })}
}
