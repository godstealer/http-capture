use crate::model::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest, Sha256};
use x509_parser::prelude::*;

pub fn certificates(chain: &[rustls::pki_types::CertificateDer<'_>]) -> Vec<CertificateDetails> {
    chain.iter().map(|der| {
        let mut result = CertificateDetails { subject: String::new(), issuer: String::new(), serial: String::new(),
            not_before: String::new(), not_after: String::new(),
            sha256: Sha256::digest(der.as_ref()).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"),
            dns_names: vec![], der_base64: STANDARD.encode(der.as_ref()), parse_error: None };
        match X509Certificate::from_der(der.as_ref()) {
            Ok((_, cert)) => {
                result.subject = cert.subject().to_string(); result.issuer = cert.issuer().to_string();
                result.serial = cert.raw_serial_as_string();
                result.not_before = cert.validity().not_before.to_string(); result.not_after = cert.validity().not_after.to_string();
                if let Ok(Some(san)) = cert.subject_alternative_name() {
                    result.dns_names = san.value.general_names.iter().map(|name| format!("{name}")).collect();
                }
            }
            Err(error) => result.parse_error = Some(error.to_string()),
        }
        result
    }).collect()
}
pub fn negotiated(state: &rustls::CommonState) -> TlsDetails {
    TlsDetails {
        version: state.protocol_version().map(|v| format!("{v:?}").replace("TLSv", "TLS ").replace('_', ".")),
        cipher_suite: state.negotiated_cipher_suite().map(|s| format!("{:?}", s.suite())),
        alpn: state.alpn_protocol().map(|v| String::from_utf8_lossy(v).into_owned()),
        handshake_kind: state.handshake_kind().map(|v| format!("{v:?}")),
        certificates: certificates(state.peer_certificates().unwrap_or_default()),
        ..Default::default()
    }
}
pub fn client_hello(hello: &rustls::server::ClientHello<'_>) -> TlsDetails {
    TlsDetails {
        server_name: hello.server_name().map(str::to_owned),
        offered_cipher_suites: hello.cipher_suites().iter().map(|v| format!("{v:?}")).collect(),
        offered_alpn: hello.alpn().map(|v| v.map(|p| String::from_utf8_lossy(p).into_owned()).collect()).unwrap_or_default(),
        signature_schemes: hello.signature_schemes().iter().map(|v| format!("{v:?}")).collect(),
        supported_groups: hello.named_groups().unwrap_or_default().iter().map(|v| format!("{v:?}")).collect(),
        ..Default::default()
    }
}

pub fn quic(connection: &quinn::Connection) -> TlsDetails {
    let mut result = TlsDetails { version: Some("TLS 1.3".into()), ..Default::default() };
    if let Some(data) = connection.handshake_data().and_then(|d| d.downcast::<quinn::crypto::rustls::HandshakeData>().ok()) {
        result.alpn = data.protocol.map(|v| String::from_utf8_lossy(&v).into_owned());
        result.server_name = data.server_name;
    }
    if let Some(chain) = connection.peer_identity().and_then(|d| d.downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>().ok()) {
        result.certificates = certificates(&chain);
    }
    // Quinn does not expose the negotiated cipher or the original ClientHello.
    result
}
