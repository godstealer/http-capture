//! Incrementally encode live bodies while keeping each published snapshot valid Base64.
use anyhow::Result;
use crate::model::SharedBody;

/// Append decoded bytes to a complete Base64 body without retaining a second
/// growing encoded string. The only decoded carry is the old padded quartet.
pub(crate) fn append_complete(body: &mut SharedBody, bytes: &[u8]) -> Result<()> { body.append_complete_base64(bytes) }

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    #[test]
    fn appending_to_complete_body_preserves_encoding_and_previous_snapshots() {
        let bytes: Vec<u8> = (0..65547).map(|n| (n % 251) as u8).collect();
        for first in [0, 1, 2, 3, 4, 5, 65535, 65536, 65537] {
            let encoded = STANDARD.encode(&bytes[..first]);
            let mut body=SharedBody::from(encoded.clone());
            let snapshot=body.clone();
            for chunk in bytes[first..].chunks(257) { append_complete(&mut body,chunk).unwrap(); }
            assert_eq!(body.as_str(), STANDARD.encode(&bytes));
            assert_eq!(snapshot.as_str(), encoded);
        }
    }
    #[test]
    fn appending_to_unique_body_reuses_spare_encoded_capacity() {
        let source=vec![7u8;128*1024+2];
        let mut text=String::with_capacity(400_000);
        STANDARD.encode_string(&source,&mut text);
        let pointer=text.as_ptr();
        let mut body=SharedBody::from(text);
        append_complete(&mut body,b"more bytes").unwrap();
        assert_eq!(body.as_ptr(),pointer);
        assert_eq!(body.as_str(),STANDARD.encode([source,b"more bytes".to_vec()].concat()));
    }
}
