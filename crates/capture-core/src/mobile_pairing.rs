//! Versioned LAN pairing payload shared by the desktop and Android clients.
//! This is an address and identity hint, not a substitute for authenticating
//! the transport or redeeming the one-use token on the desktop.
use anyhow::{bail, ensure, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use std::net::Ipv4Addr;
use url::Url;

pub const PAIRING_TOKEN_BYTES: usize = 32;
pub const PAIRING_LIFETIME_SECONDS: u64 = 300;

/// The desktop keeps the QR credential in memory only while its pairing
/// window is open. Redeeming it consumes the credential before a session is
/// issued, so concurrent or repeated attempts cannot pair another device.
#[derive(Default)]
pub struct PendingPairing {
    code: Option<PairingCode>,
}

impl PendingPairing {
    pub fn replace(&mut self, code: PairingCode) {
        self.code = Some(code);
    }

    pub fn revoke(&mut self) {
        self.code = None;
    }

    pub fn revoke_if(&mut self, token: &[u8]) {
        if self.code.as_ref().is_some_and(|code| constant_time_token_eq(&code.token, token)) {
            self.revoke();
        }
    }

    pub fn redeem(&mut self, token: &[u8], now: u64) -> Result<()> {
        let code = self.code.as_ref().ok_or_else(|| anyhow::anyhow!("Pairing is closed"))?;
        if now >= code.expires_at {
            self.revoke();
            bail!("Pairing code expired");
        }
        ensure!(constant_time_token_eq(&code.token, token), "Invalid pairing token");
        self.revoke();
        Ok(())
    }
}

fn constant_time_token_eq(expected: &[u8; PAIRING_TOKEN_BYTES], received: &[u8]) -> bool {
    // A wrong-length input still walks the full expected token. The caller
    // must use this only inside the authenticated, fingerprint-pinned TLS link.
    let mut difference = expected.len() ^ received.len();
    for (index, byte) in expected.iter().enumerate() {
        difference |= usize::from(*byte ^ received.get(index).copied().unwrap_or_default());
    }
    difference == 0
}

#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode {
    pub host: Ipv4Addr,
    pub port: u16,
    pub token: [u8; PAIRING_TOKEN_BYTES],
    /// Lowercase SHA-256 digest of the desktop transport certificate's SPKI.
    pub fingerprint: String,
    pub expires_at: u64,
}

impl PairingCode {
    pub fn generate(host: Ipv4Addr, port: u16, fingerprint: &str, now: u64) -> Result<Self> {
        let mut token = [0; PAIRING_TOKEN_BYTES];
        getrandom::getrandom(&mut token).map_err(|error| anyhow::anyhow!("Token generation failed: {error}"))?;
        Self::new(host, port, token, fingerprint, now + PAIRING_LIFETIME_SECONDS)
    }

    pub fn new(host: Ipv4Addr, port: u16, token: [u8; PAIRING_TOKEN_BYTES],
               fingerprint: &str, expires_at: u64) -> Result<Self> {
        ensure!(host.is_private() && !host.is_loopback(), "Pairing requires a private LAN IPv4 address");
        ensure!(port >= 1024, "Invalid pairing port");
        ensure!(token.iter().any(|byte| *byte != 0), "Invalid pairing token");
        ensure!(fingerprint.len() == 64 && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "Invalid desktop identity fingerprint");
        Ok(Self { host, port, token, fingerprint: fingerprint.to_ascii_lowercase(), expires_at })
    }

    pub fn to_uri(&self) -> String {
        let mut url = Url::parse("httpcapture://pair").expect("static pairing URI");
        url.query_pairs_mut()
            .append_pair("v", "1")
            .append_pair("host", &self.host.to_string())
            .append_pair("port", &self.port.to_string())
            .append_pair("token", &URL_SAFE_NO_PAD.encode(self.token))
            .append_pair("fp", &self.fingerprint)
            .append_pair("exp", &self.expires_at.to_string());
        url.into()
    }

    pub fn parse(uri: &str, now: u64) -> Result<Self> {
        ensure!(uri.len() <= 512, "Pairing code is too long");
        let url = Url::parse(uri)?;
        ensure!(url.scheme() == "httpcapture" && url.host_str() == Some("pair")
                && url.path().is_empty() && url.fragment().is_none(), "Invalid pairing URI");
        let mut fields = std::collections::BTreeMap::new();
        for (key, value) in url.query_pairs() {
            ensure!(["v", "host", "port", "token", "fp", "exp"].contains(&key.as_ref())
                    && fields.insert(key.into_owned(), value.into_owned()).is_none(),
                    "Unknown or repeated pairing field");
        }
        ensure!(fields.len() == 6 && fields.get("v").map(String::as_str) == Some("1"),
                "Unsupported pairing code version");
        let host = fields["host"].parse()?;
        let port = fields["port"].parse()?;
        let token = URL_SAFE_NO_PAD.decode(&fields["token"])?;
        let token: [u8; PAIRING_TOKEN_BYTES] = token.try_into().map_err(|_| anyhow::anyhow!("Invalid pairing token length"))?;
        let expires_at: u64 = fields["exp"].parse()?;
        if expires_at <= now { bail!("Pairing code expired"); }
        ensure!(expires_at - now <= PAIRING_LIFETIME_SECONDS, "Pairing code expiry is too far away");
        Self::new(host, port, token, &fields["fp"], expires_at)
    }
}

pub fn token_from_uri(uri: &str) -> Result<[u8; PAIRING_TOKEN_BYTES]> {
    ensure!(uri.len() <= 512, "Pairing code is too long");
    let url = Url::parse(uri)?;
    ensure!(url.scheme() == "httpcapture" && url.host_str() == Some("pair"), "Invalid pairing URI");
    let encoded = url.query_pairs().find(|(key, _)| key == "token")
        .map(|(_, value)| value.into_owned()).ok_or_else(|| anyhow::anyhow!("Missing pairing token"))?;
    URL_SAFE_NO_PAD.decode(encoded)?.try_into()
        .map_err(|_| anyhow::anyhow!("Invalid pairing token length"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_reject_expired_or_repeated_credentials() {
        let code = PairingCode::new("192.168.1.20".parse().unwrap(), 8899,
            [7; PAIRING_TOKEN_BYTES], &"ab".repeat(32), 1_300).unwrap();
        let uri = code.to_uri();
        assert!(PairingCode::parse(&uri, 1_000).unwrap() == code);
        assert!(PairingCode::parse(&uri, 1_300).is_err());
        assert!(PairingCode::parse(&format!("{uri}&token=other"), 1_000).is_err());
        assert!(PairingCode::parse(&uri.replace("192.168.1.20", "8.8.8.8"), 1_000).is_err());
    }

    #[test]
    fn pairing_token_is_one_use_and_revocable() {
        let code = PairingCode::new("192.168.1.20".parse().unwrap(), 8899,
            [7; PAIRING_TOKEN_BYTES], &"ab".repeat(32), 1_300).unwrap();
        let mut pending = PendingPairing::default();
        pending.replace(code.clone());
        assert!(pending.redeem(&[8; PAIRING_TOKEN_BYTES], 1_000).is_err());
        assert!(pending.redeem(&[7; PAIRING_TOKEN_BYTES - 1], 1_000).is_err());
        pending.redeem(&code.token, 1_000).unwrap();
        assert!(pending.redeem(&code.token, 1_000).is_err());
        pending.replace(code.clone());
        pending.revoke();
        assert!(pending.redeem(&code.token, 1_000).is_err());
        pending.replace(code.clone());
        pending.revoke_if(&[8; PAIRING_TOKEN_BYTES]);
        pending.redeem(&code.token, 1_000).unwrap();
        pending.replace(code);
        assert!(pending.redeem(&[7; PAIRING_TOKEN_BYTES], 1_300).is_err());
    }
}
