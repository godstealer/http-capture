use anyhow::{ensure, Context, Result};
use rcgen::{BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose};
use rustls::{pki_types::PrivatePkcs8KeyDer, ServerConfig};
use std::{fs, io::Write, path::{Path, PathBuf}, sync::Arc};

pub struct CertificateAuthority {
    cert: Certificate,
    key: KeyPair,
    pub cert_path: PathBuf,
}

impl CertificateAuthority {
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        let cert_path = dir.join("capture-ca.pem");
        let key_path = dir.join("capture-ca.key");
        ensure!(cert_path.exists() == key_path.exists(), "CA certificate/key incomplete; restore both files or remove both to regenerate");
        if cert_path.exists() {
            let pem = fs::read_to_string(&cert_path)?;
            let key = KeyPair::from_pem(&fs::read_to_string(key_path)?)?;
            let params = CertificateParams::from_ca_cert_pem(&pem)?;
            let cert = params.self_signed(&key)?;
            // Re-signing identical CA parameters is only used as the issuer object;
            // the persisted CA certificate is the one installed by the user.
            return Ok(Self { cert, key, cert_path });
        }
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, "HTTP Capture Local CA");
        let cert = params.self_signed(&key)?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&key_path)?.write_all(key.serialize_pem().as_bytes())?;
        fs::write(&cert_path, cert.pem())?;
        Ok(Self { cert, key, cert_path })
    }

    pub fn server_config(&self, host: &str) -> Result<Arc<ServerConfig>> {
        let mut params = CertificateParams::new(vec![host.to_owned()])?;
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        let key = KeyPair::generate()?;
        let cert = params.signed_by(&key, &self.cert, &self.key)?;
        let mut config = ServerConfig::builder().with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], PrivatePkcs8KeyDer::from(key.serialize_der()).into())
            .context("Create MITM server configuration")?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }
}
