//! Per-request rustls configuration; never changes process-wide defaults.
use crate::model::TlsProfile;
use anyhow::{ensure, Context, Result};
use std::sync::Arc;

fn entries(value: &str) -> Result<Vec<&str>> {
    ensure!(value.len() <= 4096, "TLS list exceeds 4096 bytes");
    let values: Vec<_> = value.split(|c: char| c == ':' || c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).collect();
    ensure!(!values.is_empty(), "TLS list must not be empty");
    let mut seen = std::collections::HashSet::new();
    ensure!(values.iter().all(|s| seen.insert(s.to_ascii_lowercase())), "Duplicate TLS list entry");
    Ok(values)
}
pub fn config(profile: &TlsProfile, roots: &rustls::RootCertStore, engine: &str) -> Result<rustls::ClientConfig> {
    ensure!(profile.preset == "native", "This engine requires the native TLS profile");
    ensure!(profile.client_hello_hex.is_none(), "ClientHello Hex requires the httpcloak engine");
    ensure!(profile.sigalgs_list.is_none() && profile.grease.is_none() && profile.permute_extensions.is_none(), "Signature algorithms, GREASE and extension permutation require a browser TLS engine");
    let mut provider = rustls::crypto::ring::default_provider();
    if let Some(list) = &profile.cipher_list {
        provider.cipher_suites = entries(list)?.into_iter().map(|name| {
            rustls::crypto::ring::ALL_CIPHER_SUITES.iter().copied().find(|s| format!("{:?}", s.suite()).replace("TLS13_", "TLS_").eq_ignore_ascii_case(name))
                .with_context(|| format!("Unsupported cipher suite: {name}. Use IANA names, e.g. TLS_AES_128_GCM_SHA256"))
        }).collect::<Result<_>>()?;
    }
    if let Some(list) = &profile.curves_list {
        provider.kx_groups = entries(list)?.into_iter().map(|name| {
            let name = match name.to_ascii_lowercase().as_str() { "p-256" | "prime256v1" => "secp256r1", "p-384" => "secp384r1", _ => name };
            rustls::crypto::ring::DEFAULT_KX_GROUPS.iter().copied().find(|g| format!("{:?}", g.name()).eq_ignore_ascii_case(name))
                .with_context(|| format!("Unsupported key exchange group: {name}; use X25519, secp256r1 or secp384r1"))
        }).collect::<Result<_>>()?;
    }
    let versions: Vec<_> = match profile.version.as_deref().unwrap_or("auto") {
        "auto" if engine == "h3" => vec![&rustls::version::TLS13],
        "auto" => vec![&rustls::version::TLS13, &rustls::version::TLS12],
        "1.2" => { ensure!(engine != "h3", "HTTP/3 requires TLS 1.3"); vec![&rustls::version::TLS12] },
        "1.3" => vec![&rustls::version::TLS13],
        other => anyhow::bail!("Unsupported TLS version: {other}"),
    };
    ensure!(provider.cipher_suites.iter().any(|suite| versions.iter().any(|v| suite.version().version == v.version)), "No cipher suite supports the selected TLS version");
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(provider)).with_protocol_versions(&versions)?
        .with_root_certificates(roots.clone()).with_no_client_auth();
    config.alpn_protocols = match engine { "auto" => vec![b"h2".to_vec(), b"http/1.1".to_vec()], "h2" => vec![b"h2".to_vec()], "h3" => vec![b"h3".to_vec()], _ => vec![b"http/1.1".to_vec()] };
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_and_preserves_preferences() {
        let roots = rustls::RootCertStore::empty();
        let mut p = TlsProfile::default();
        p.version = Some("1.3".into());
        p.cipher_list = Some("TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384".into());
        p.curves_list = Some("secp256r1:X25519".into());
        let c = config(&p, &roots, "auto").unwrap();
        assert_eq!(format!("{:?}", c.crypto_provider().cipher_suites[0].suite()).replace("TLS13_", "TLS_"), "TLS_AES_128_GCM_SHA256");
        assert_eq!(format!("{:?}", c.crypto_provider().kx_groups[0].name()), "secp256r1");
        assert_eq!(c.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
        p.version = Some("1.2".into()); assert!(config(&p,&roots,"native").is_err());
        assert!(config(&p,&roots,"h3").is_err());
        p.version = None; p.cipher_list = Some("INVALID".into()); assert!(config(&p,&roots,"native").is_err());
        p.cipher_list = None; p.curves_list = Some("X25519:X25519".into()); assert!(config(&p,&roots,"native").is_err());
        p.curves_list = None; p.grease = Some(false); assert!(config(&p,&roots,"native").is_err());
    }
}
