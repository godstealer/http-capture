use anyhow::Result;
use crate::store::Store;
pub fn matches(host:&str,patterns:&[String])->bool {
 let host=host.to_ascii_lowercase();
 patterns.iter().any(|p|p.strip_prefix("*.").map_or(host.eq_ignore_ascii_case(p),|suffix|host.ends_with(&format!(".{suffix}"))))
}
pub fn save(store:&Store,hosts:Vec<String>)->Result<()> {
 anyhow::ensure!(hosts.len()<=256,"Too many passthrough hosts");
 let mut result=Vec::new();
 for host in hosts {let host=host.trim().to_ascii_lowercase();let domain=host.strip_prefix("*.").unwrap_or(&host);
  anyhow::ensure!(!domain.is_empty()&&domain.len()<=253&&domain.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'.'||b==b'-'),"Invalid passthrough hostname: {host}");
  if !result.contains(&host){result.push(host);}
 }
 store.save_setting("decryption.bypass.v1",&result)
}
#[cfg(test)]mod tests{use super::*;#[test]fn exact_and_subdomain(){assert!(matches("API.EXAMPLE.COM",&["*.example.com".into()]));assert!(!matches("example.com",&["*.example.com".into()]));assert!(!matches("badexample.com",&["*.example.com".into()]));assert!(matches("example.com",&["example.com".into()]));}}
