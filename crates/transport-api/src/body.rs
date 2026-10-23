use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use anyhow::{ensure, Result};
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, OnceLock};

/// Shared text storage for a body that still crosses public APIs as Base64.
///
/// Cloning a draft or response shares the existing String allocation. Mutation
/// uses copy-on-write, so editing one owner cannot change another owner's body.
/// Construction and serialization do not validate or normalize the text;
/// append_complete_base64 validates the suffix it needs to extend.
#[derive(Clone, Default, Debug)]
pub struct SharedBody(Arc<BodyStorage>);

#[derive(Clone, Copy, Debug)]
struct PersistedDigest { decoded_length: usize, digest: [u8;32] }

#[derive(Default, Debug)]
struct BodyStorage { text: String, encoded_sha256: OnceLock<Box<[u8;32]>>, prefix_sha256: OnceLock<Sha256>, persisted: Mutex<Option<PersistedDigest>> }

impl Clone for BodyStorage {
    fn clone(&self)->Self {
        let persisted=*self.persisted.lock().unwrap_or_else(|error|error.into_inner());
        Self {text:self.text.clone(),encoded_sha256:self.encoded_sha256.clone(),prefix_sha256:self.prefix_sha256.clone(),persisted:Mutex::new(persisted)}
    }
}

impl PartialEq for SharedBody {
    fn eq(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0,&other.0) || self.0.text == other.0.text }
}
impl Eq for SharedBody {}

impl SharedBody {
    /// True only when this append lineage last committed exactly the old BLOB
    /// represented by the database row. An arbitrary String edit clears it.
    pub fn matches_persisted_base64(&self, decoded_length:usize, digest:&[u8])->bool {
        self.0.persisted.lock().unwrap_or_else(|error|error.into_inner())
            .as_ref().is_some_and(|saved|saved.decoded_length==decoded_length&&saved.digest.as_slice()==digest)
    }

    /// Called only after the corresponding SQLite transaction commits. A later
    /// append preserves this checkpoint until the next successful snapshot.
    pub fn mark_persisted_base64(&self,decoded_length:usize) {
        let digest=*self.encoded_sha256();
        *self.0.persisted.lock().unwrap_or_else(|error|error.into_inner())=Some(PersistedDigest{decoded_length,digest});
    }
    /// Cache the digest on the shared immutable allocation. A mutable borrow
    /// invalidates it before the underlying text can change.
    pub fn encoded_sha256(&self) -> &[u8;32] {
        self.0.encoded_sha256.get_or_init(|| {
            let end=self.0.text.len().saturating_sub(4);
            let prefix=self.0.prefix_sha256.get_or_init(|| {
                let mut hash=Sha256::new();hash.update(&self.0.text.as_bytes()[..end]);hash
            });
            let mut hash=prefix.clone();hash.update(&self.0.text.as_bytes()[end..]);
            Box::new(hash.finalize().into())
        })
    }

    /// Extend a complete Base64 body while keeping its hash state at the
    /// boundary before the final quartet. This avoids rehashing the entire
    /// growing text on every live snapshot. Arbitrary String edits still
    /// invalidate both caches through DerefMut.
    pub fn append_complete_base64(&mut self, mut bytes:&[u8]) -> Result<()> {
        if bytes.is_empty() {return Ok(())}
        let padding=self.0.text.as_bytes().iter().rev().take_while(|byte|**byte==b'=').count();
        ensure!(padding<=2&&(padding==0||self.0.text.len()>=4),"Invalid Base64 body suffix");
        let keep=if padding==0 {self.0.text.len()} else {self.0.text.len()-4};
        let mut carry=[0u8;3];let mut carried=0;
        if padding!=0 {
            let decoded=STANDARD.decode(&self.0.text[keep..])?;
            carried=decoded.len();carry[..carried].copy_from_slice(&decoded);
        }
        let total=carried.checked_add(bytes.len()).ok_or_else(||anyhow::anyhow!("Base64 body length overflow"))?;
        let suffix=total.checked_add(2).and_then(|value|value.checked_div(3)).and_then(|value|value.checked_mul(4))
            .ok_or_else(||anyhow::anyhow!("Base64 body length overflow"))?;
        let old_prefix_end=self.0.text.len().saturating_sub(4);
        let mut prefix_hash=self.0.prefix_sha256.get_or_init(|| {
            let mut hash=Sha256::new();hash.update(&self.0.text.as_bytes()[..old_prefix_end]);hash
        }).clone();
        let storage=Arc::make_mut(&mut self.0);
        let text=&mut storage.text;
        text.try_reserve(suffix)?;
        text.truncate(keep);
        if carried!=0 {
            let take=(3-carried).min(bytes.len());
            carry[carried..carried+take].copy_from_slice(&bytes[..take]);
            bytes=&bytes[take..];carried+=take;
            STANDARD.encode_string(&carry[..carried],text);
        }
        if carried==0||carried==3 {
            let aligned=bytes.len()/3*3;
            if aligned!=0 {STANDARD.encode_string(&bytes[..aligned],text);}
            if aligned<bytes.len() {STANDARD.encode_string(&bytes[aligned..],text);}
        }
        let new_prefix_end=text.len().saturating_sub(4);
        prefix_hash.update(&text.as_bytes()[old_prefix_end..new_prefix_end]);
        storage.prefix_sha256.take();let _=storage.prefix_sha256.set(prefix_hash);
        storage.encoded_sha256.take();
        Ok(())
    }
}

impl From<String> for SharedBody {
    fn from(value: String) -> Self {
        Self(Arc::new(BodyStorage { text:value, encoded_sha256:OnceLock::new(), prefix_sha256:OnceLock::new(), persisted:Mutex::new(None) }))
    }
}

impl From<&str> for SharedBody {
    fn from(value: &str) -> Self {
        Self::from(value.to_owned())
    }
}

impl Deref for SharedBody {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0.text
    }
}

impl DerefMut for SharedBody {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let storage=Arc::make_mut(&mut self.0);
        storage.encoded_sha256.take();
        storage.prefix_sha256.take();
        storage.persisted.get_mut().unwrap_or_else(|error|error.into_inner()).take();
        &mut storage.text
    }
}

impl AsRef<[u8]> for SharedBody {
    fn as_ref(&self) -> &[u8] {
        self.0.text.as_bytes()
    }
}

impl AsRef<str> for SharedBody {
    fn as_ref(&self) -> &str {
        self.0.text.as_str()
    }
}

impl Serialize for SharedBody {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.text.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SharedBody {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::SharedBody;
    use crate::{CapturedResponse, RequestDraft};
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde_json::{json, Value};

    fn request(body: Value) -> Value {
        json!({"method": "POST", "url": "https://example.invalid/", "headers": [], "bodyBase64": body})
    }

    fn response(body: Value) -> Value {
        json!({"status": 200, "version": "HTTP/2", "headers": [], "bodyBase64": body})
    }

    #[test]
    fn moving_string_preserves_its_existing_buffer_and_capacity() {
        let mut text = String::with_capacity(4096);
        text.push_str("YWJjZA==");
        let pointer = text.as_ptr();
        let capacity = text.capacity();
        let body = SharedBody::from(text);
        assert_eq!(body.as_ptr(), pointer);
        assert_eq!(body.capacity(), capacity);
        assert_eq!(body.as_str(), "YWJjZA==");
    }

    #[test]
    fn thirty_two_large_body_clones_share_the_same_string_and_bytes() {
        let body = SharedBody::from("A".repeat(8 * 1024 * 1024));
        let pointer = body.as_ptr();
        let mut copies = Vec::with_capacity(32);
        for _ in 0..32 {
            let copy = body.clone();
            assert!(std::ptr::eq::<String>(&*body, &*copy));
            assert_eq!(copy.as_ptr(), pointer);
            assert_eq!(copy.len(), body.len());
            copies.push(copy);
        }
        drop(body);
        assert!(copies.iter().all(|copy| copy.as_ptr() == pointer));
    }

    #[test]
    fn uniquely_owned_mutation_reuses_spare_string_capacity() {
        let mut text = String::with_capacity(4096);
        text.push_str("YWJj");
        let pointer = text.as_ptr();
        let capacity = text.capacity();
        let mut body = SharedBody::from(text);
        body.push_str("ZA==");
        assert_eq!(body.as_ptr(), pointer);
        assert_eq!(body.capacity(), capacity);
        assert_eq!(body.as_str(), "YWJjZA==");
    }

    #[test]
    fn mutating_a_shared_body_copies_only_the_edited_owner() {
        let mut edited = SharedBody::from("YWJj");
        let original = edited.clone();
        let another = original.clone();
        let original_pointer = original.as_ptr();
        edited.push_str("ZA==");
        assert_ne!(edited.as_ptr(), original_pointer);
        assert_eq!(edited.as_str(), "YWJjZA==");
        assert_eq!(original.as_str(), "YWJj");
        assert_eq!(another.as_ptr(), original_pointer);
        assert!(std::ptr::eq::<String>(&*original, &*another));
    }

    #[test]
    fn digest_is_shared_by_clones_and_invalidated_by_mutation() {
        use sha2::{Digest, Sha256};
        let mut body=SharedBody::from("YWJj");
        let same=body.clone();
        let old=*body.encoded_sha256();
        assert_eq!(old, <[u8;32]>::from(Sha256::digest(b"YWJj")));
        assert_eq!(body.encoded_sha256().as_ptr(),same.encoded_sha256().as_ptr());
        body.push_str("ZA==");
        assert_eq!(*body.encoded_sha256(),<[u8;32]>::from(Sha256::digest(b"YWJjZA==")));
        assert_eq!(*same.encoded_sha256(),old);
        let mut unique=SharedBody::from("YQ==");
        let previous=*unique.encoded_sha256();
        unique.clear();
        assert_ne!(*unique.encoded_sha256(),previous);
        assert_eq!(*unique.encoded_sha256(),<[u8;32]>::from(Sha256::digest(b"")));
        assert_eq!(same,SharedBody::from("YWJj"));
    }

    #[test]
    fn incremental_digest_matches_full_hash_across_padding_and_copy_on_write() {
        use sha2::{Digest,Sha256};
        let bytes:Vec<u8>=(0..65547).map(|n|(n%251) as u8).collect();
        for first in [0,1,2,3,4,5,65535,65536,65537] {
            let mut body=SharedBody::from(STANDARD.encode(&bytes[..first]));
            let snapshot=body.clone();
            let old=*snapshot.encoded_sha256();
            for chunk in bytes[first..].chunks(257) {
                body.append_complete_base64(chunk).unwrap();
                assert_eq!(*body.encoded_sha256(),<[u8;32]>::from(Sha256::digest(body.as_bytes())));
            }
            assert_eq!(body.as_str(),STANDARD.encode(&bytes));
            assert_eq!(*snapshot.encoded_sha256(),old);
            assert_eq!(snapshot.as_str(),STANDARD.encode(&bytes[..first]));
            body.replace_range(0..4,"AQID");
            assert_eq!(*body.encoded_sha256(),<[u8;32]>::from(Sha256::digest(body.as_bytes())));
        }
    }

    #[test]
    fn committed_append_checkpoint_survives_appends_but_not_arbitrary_edits() {
        let mut body=SharedBody::from("YQ==");
        let saved=*body.encoded_sha256();
        body.mark_persisted_base64(1);
        let old=body.clone();
        body.append_complete_base64(b"bc").unwrap();
        assert!(body.matches_persisted_base64(1,&saved));
        assert_eq!(body.as_str(),"YWJj");
        body.mark_persisted_base64(3);
        assert!(!body.matches_persisted_base64(1,&saved));
        assert!(body.matches_persisted_base64(3,body.encoded_sha256()));
        assert!(old.matches_persisted_base64(1,&saved));
        body.push_str("ZA==");
        assert!(!body.matches_persisted_base64(3,body.encoded_sha256()));
    }

    #[test]
    fn default_borrowing_equality_and_debug_keep_string_semantics() {
        let empty = SharedBody::default();
        assert!(empty.is_empty());
        let body = SharedBody::from("中文\0Zg==");
        assert_eq!(AsRef::<str>::as_ref(&body), "中文\0Zg==");
        assert_eq!(AsRef::<[u8]>::as_ref(&body), "中文\0Zg==".as_bytes());
        assert_eq!(body, SharedBody::from(String::from("中文\0Zg==")));
        assert!(format!("{body:?}").contains("Zg=="));
    }

    #[test]
    fn serde_delegates_to_string_without_base64_or_text_normalization() {
        let text = " Zg==\r\n\t%%%中文\0";
        let body = SharedBody::from(text);
        assert_eq!(serde_json::to_value(&body).unwrap(), Value::String(text.into()));
        let restored: SharedBody = serde_json::from_value(Value::String(text.into())).unwrap();
        assert_eq!(restored.as_str(), text);
        assert_eq!(serde_json::to_string(&body).unwrap(), serde_json::to_string(text).unwrap());
    }

    #[test]
    fn old_request_and_response_json_keep_plain_string_body_fields() {
        let text = " Zg==\r\n";
        let draft: RequestDraft = serde_json::from_value(request(json!(text))).unwrap();
        let captured: CapturedResponse = serde_json::from_value(response(json!(text))).unwrap();
        assert_eq!(draft.body_base64.as_str(), text);
        assert_eq!(captured.body_base64.as_str(), text);
        assert_eq!(serde_json::to_value(draft).unwrap()["bodyBase64"], json!(text));
        assert_eq!(serde_json::to_value(captured).unwrap()["bodyBase64"], json!(text));
        let empty_draft: RequestDraft = serde_json::from_value(request(json!(""))).unwrap();
        assert!(empty_draft.body_base64.is_empty());
    }

    #[test]
    fn null_numbers_and_other_non_string_json_bodies_are_rejected() {
        for value in [Value::Null, json!(1), json!(1.5), json!(true), json!([]), json!({})] {
            assert!(serde_json::from_value::<SharedBody>(value.clone()).is_err());
            assert!(serde_json::from_value::<RequestDraft>(request(value.clone())).is_err());
            assert!(serde_json::from_value::<CapturedResponse>(response(value)).is_err());
        }
    }

    #[test]
    fn missing_body_fields_do_not_silently_receive_the_default() {
        let mut draft = request(json!(""));
        draft.as_object_mut().unwrap().remove("bodyBase64");
        assert!(serde_json::from_value::<RequestDraft>(draft).is_err());
        let mut captured = response(json!(""));
        captured.as_object_mut().unwrap().remove("bodyBase64");
        assert!(serde_json::from_value::<CapturedResponse>(captured).is_err());
    }
}
