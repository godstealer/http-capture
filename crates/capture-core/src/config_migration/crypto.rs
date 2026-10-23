//! Optional password-protected portable configuration envelope. The plaintext
//! package format and its validation remain unchanged after decryption.
use super::{strict_json,MAX_DOCUMENT_BYTES};
use aes_gcm::{aead::{Aead,Payload},Aes256Gcm,KeyInit,Nonce};
use anyhow::{anyhow,ensure,Context,Result};
use base64::{engine::general_purpose::STANDARD,Engine as _};
use openssl::{hash::MessageDigest,pkcs5::pbkdf2_hmac};
use serde::{Deserialize,Serialize};
use zeroize::Zeroize;

pub const MAX_ENCRYPTED_DOCUMENT_BYTES:usize=24*1024*1024;
const FORMAT:&str="http-capture-encrypted-config";
const ITERATIONS:usize=600_000;
const AAD:&[u8]=b"http-capture-encrypted-config:v1:pbkdf2-sha256:600000:aes-256-gcm";

#[derive(Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Envelope {
    format:String,
    schema_version:u32,
    kdf:String,
    iterations:usize,
    salt_base64:String,
    cipher:String,
    nonce_base64:String,
    ciphertext_base64:String,
}

fn check_passphrase(passphrase:&str)->Result<()> {
    ensure!(passphrase.chars().count()>=12&&passphrase.len()<=1024,"Passphrase must contain at least 12 characters and no more than 1024 UTF-8 bytes");
    Ok(())
}

/// A strict, bounded format probe. Plain configuration files keep their
/// existing parser and validation path.
pub fn is_encrypted_document(document:&str)->bool {
    document.len()<=MAX_ENCRYPTED_DOCUMENT_BYTES
        && serde_json::from_str::<serde_json::Value>(document).ok()
            .and_then(|value|value.get("format").and_then(serde_json::Value::as_str).map(str::to_owned))
            .is_some_and(|format|format==FORMAT)
}

pub fn encrypt_document(plaintext:&str,passphrase:&str)->Result<String> {
    check_passphrase(passphrase)?;
    ensure!(!plaintext.is_empty()&&plaintext.len()<=MAX_DOCUMENT_BYTES,"Configuration file exceeds 16 MiB");
    let mut salt=[0u8;16];let mut nonce=[0u8;12];
    getrandom::getrandom(&mut salt).map_err(|e|anyhow!("Cannot generate encryption salt: {e}"))?;
    getrandom::getrandom(&mut nonce).map_err(|e|anyhow!("Cannot generate encryption nonce: {e}"))?;
    let mut key=[0u8;32];
    pbkdf2_hmac(passphrase.as_bytes(),&salt,ITERATIONS,MessageDigest::sha256(),&mut key)?;
    let cipher=Aes256Gcm::new_from_slice(&key).map_err(|_|anyhow!("Invalid encryption key"))?;
    let ciphertext=cipher.encrypt(Nonce::from_slice(&nonce),Payload{msg:plaintext.as_bytes(),aad:AAD}).map_err(|_|anyhow!("Cannot encrypt configuration"));
    key.zeroize();
    let envelope=Envelope{format:FORMAT.into(),schema_version:1,kdf:"pbkdf2-sha256".into(),iterations:ITERATIONS,salt_base64:STANDARD.encode(salt),cipher:"aes-256-gcm".into(),nonce_base64:STANDARD.encode(nonce),ciphertext_base64:STANDARD.encode(ciphertext?)};
    let document=serde_json::to_string_pretty(&envelope)?;
    ensure!(document.len()<=MAX_ENCRYPTED_DOCUMENT_BYTES,"Encrypted configuration exceeds 24 MiB");
    Ok(document)
}

pub fn decrypt_document(document:&str,passphrase:&str)->Result<String> {
    check_passphrase(passphrase)?;
    ensure!(document.len()<=MAX_ENCRYPTED_DOCUMENT_BYTES,"Encrypted configuration exceeds 24 MiB");
    let envelope:Envelope=serde_json::from_value(strict_json(document)?).context("Invalid encrypted configuration envelope")?;
    ensure!(envelope.format==FORMAT&&envelope.schema_version==1&&envelope.kdf=="pbkdf2-sha256"&&envelope.iterations==ITERATIONS&&envelope.cipher=="aes-256-gcm","Unsupported encrypted configuration format or parameters");
    ensure!(envelope.salt_base64.len()==24&&envelope.nonce_base64.len()==16&&envelope.ciphertext_base64.len()<=((MAX_DOCUMENT_BYTES+16+2)/3)*4,"Invalid encrypted configuration sizes");
    let salt=STANDARD.decode(envelope.salt_base64).context("Invalid encryption salt")?;
    let nonce=STANDARD.decode(envelope.nonce_base64).context("Invalid encryption nonce")?;
    let ciphertext=STANDARD.decode(envelope.ciphertext_base64).context("Invalid encrypted payload")?;
    ensure!(salt.len()==16&&nonce.len()==12&&ciphertext.len()>=16&&ciphertext.len()<=MAX_DOCUMENT_BYTES+16,"Invalid encrypted configuration sizes");
    let mut key=[0u8;32];
    pbkdf2_hmac(passphrase.as_bytes(),&salt,ITERATIONS,MessageDigest::sha256(),&mut key)?;
    let cipher=Aes256Gcm::new_from_slice(&key).map_err(|_|anyhow!("Invalid encryption key"))?;
    let plaintext=cipher.decrypt(Nonce::from_slice(&nonce),Payload{msg:&ciphertext,aad:AAD});
    key.zeroize();
    let plaintext=plaintext.map_err(|_|anyhow!("Incorrect passphrase or damaged configuration file"))?;
    ensure!(plaintext.len()<=MAX_DOCUMENT_BYTES,"Configuration file exceeds 16 MiB");
    String::from_utf8(plaintext).context("Decrypted configuration is not UTF-8")
}

pub fn validate_encrypted_document(document:&str)->Result<()> {
    ensure!(document.len()<=MAX_ENCRYPTED_DOCUMENT_BYTES,"Encrypted configuration exceeds 24 MiB");
    let envelope:Envelope=serde_json::from_value(strict_json(document)?)?;
    ensure!(envelope.format==FORMAT&&envelope.schema_version==1&&envelope.kdf=="pbkdf2-sha256"&&envelope.iterations==ITERATIONS&&envelope.cipher=="aes-256-gcm","Unsupported encrypted configuration format or parameters");
    ensure!(envelope.salt_base64.len()==24&&envelope.nonce_base64.len()==16&&envelope.ciphertext_base64.len()<=((MAX_DOCUMENT_BYTES+16+2)/3)*4,"Invalid encrypted configuration sizes");
    ensure!(STANDARD.decode(envelope.salt_base64)?.len()==16&&STANDARD.decode(envelope.nonce_base64)?.len()==12,"Invalid encryption salt or nonce");
    let bytes=STANDARD.decode(envelope.ciphertext_base64)?;
    ensure!((16..=MAX_DOCUMENT_BYTES+16).contains(&bytes.len()),"Invalid encrypted payload size");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const PLAIN:&str=r#"{"format":"http-capture-config","schemaVersion":1,"sections":{}}"#;
    #[test]
    fn encrypted_roundtrip_is_random_and_hides_plaintext() {
        let first=encrypt_document(PLAIN,"a strong passphrase").unwrap();
        let second=encrypt_document(PLAIN,"a strong passphrase").unwrap();
        assert_ne!(first,second);
        assert!(!first.contains("http-capture-config"));
        assert!(is_encrypted_document(&first));
        assert_eq!(decrypt_document(&first,"a strong passphrase").unwrap(),PLAIN);
        assert!(decrypt_document(&first,"wrong passphrase").is_err());
    }
    #[test]
    fn rejects_tampering_and_hostile_parameters() {
        let original=encrypt_document(PLAIN,"a test password").unwrap();
        let mut value:serde_json::Value=serde_json::from_str(&original).unwrap();
        value["iterations"]=serde_json::json!(usize::MAX);
        assert!(decrypt_document(&value.to_string(),"a test password").is_err());
        let mut value:serde_json::Value=serde_json::from_str(&original).unwrap();
        value["ciphertextBase64"]=serde_json::json!(STANDARD.encode([0u8;16]));
        assert!(decrypt_document(&value.to_string(),"a test password").is_err());
        let duplicate=original.replacen("\"format\":", "\"format\":\"http-capture-encrypted-config\",\"format\":",1);
        assert!(decrypt_document(&duplicate,"a test password").is_err());
    }
    #[test]
    fn encrypted_file_is_saved_atomically_and_plain_format_stays_separate() {
        let temp=tempfile::tempdir().unwrap();let path=temp.path().join("config.hccfg");
        let sealed=encrypt_document(PLAIN,"a test password").unwrap();
        super::super::save_atomic(&path,&sealed).unwrap();
        assert_eq!(decrypt_document(&std::fs::read_to_string(&path).unwrap(),"a test password").unwrap(),PLAIN);
        assert!(!is_encrypted_document(PLAIN));
        assert!(super::super::save_atomic(&path,"{\"format\":\"http-capture-encrypted-config\"}").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(),sealed);
    }
}
