//! Incrementally encode live bodies while keeping each published snapshot valid Base64.
use base64::{engine::general_purpose::STANDARD, Engine as _};

#[derive(Default)]
pub(crate) struct GrowingBase64 { encoded: String, pending: Vec<u8> }

impl GrowingBase64 {
    pub(crate) fn append(&mut self, bytes: &[u8]) {
        let mut remaining = bytes;
        if !self.pending.is_empty() {
            let take = (3 - self.pending.len()).min(remaining.len());
            self.pending.extend_from_slice(&remaining[..take]);
            remaining = &remaining[take..];
            if self.pending.len() == 3 {
                self.encoded.push_str(&STANDARD.encode(&self.pending));
                self.pending.clear();
            }
        }
        let complete = remaining.len() / 3 * 3;
        if complete > 0 { self.encoded.push_str(&STANDARD.encode(&remaining[..complete])); }
        self.pending.extend_from_slice(&remaining[complete..]);
    }

    pub(crate) fn snapshot(&self) -> String {
        let mut encoded = self.encoded.clone();
        if !self.pending.is_empty() { encoded.push_str(&STANDARD.encode(&self.pending)); }
        encoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_match_whole_body_across_split_boundaries() {
        let source: Vec<u8> = (0..1027).map(|n| (n % 256) as u8).collect();
        for chunk_size in [1, 2, 3, 4, 5, 7, 64, 257] {
            let mut growing = GrowingBase64::default(); let mut expected = Vec::new();
            assert_eq!(growing.snapshot(), "");
            for chunk in source.chunks(chunk_size) {
                growing.append(chunk); expected.extend_from_slice(chunk);
                assert_eq!(growing.snapshot(), STANDARD.encode(&expected), "chunk size {chunk_size}, length {}", expected.len());
            }
            growing.append(&[]); assert_eq!(growing.snapshot(), STANDARD.encode(&expected));
        }
    }
}
