use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

/// Shared text storage for a body that still crosses public APIs as Base64.
///
/// Cloning a draft or response shares the existing String allocation. Mutation
/// uses copy-on-write, so editing one owner cannot change another owner's body.
/// This type does not validate, decode, or normalize its text representation.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct SharedBody(Arc<String>);

impl From<String> for SharedBody {
    fn from(value: String) -> Self {
        Self(Arc::new(value))
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
        self.0.as_ref()
    }
}

impl DerefMut for SharedBody {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Arc::make_mut(&mut self.0)
    }
}

impl AsRef<[u8]> for SharedBody {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl AsRef<str> for SharedBody {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl Serialize for SharedBody {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_ref().serialize(serializer)
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
