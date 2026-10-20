//! Bounded views of a body. These limits apply to inspection, never forwarding
//! or persistence. A Base64 range validates its selected quanta and the final
//! quantum; validating an entire saved body remains the full reader's job.
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};

pub const MAX_READ_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyReadPlan {
    /// The caller's byte offset, including offsets beyond the end of the body.
    pub offset: u64,
    /// The actual first byte, clamped to the body length.
    pub start: u64,
    pub length: usize,
    pub total_bytes: u64,
    pub next_offset: Option<u64>,
}

impl BodyReadPlan {
    pub fn new(total_bytes: u64, offset: u64, length: usize) -> Result<Self> {
        ensure!((1..=MAX_READ_BYTES).contains(&length), "Body read length must be between 1 and 65536 bytes");
        let start = offset.min(total_bytes);
        let length = (total_bytes - start).min(length as u64) as usize;
        // start + length is at most total_bytes, including at u64::MAX.
        let end = start + length as u64;
        Ok(Self { offset, start, length, total_bytes, next_offset: (end < total_bytes).then_some(end) })
    }

    pub fn eof(&self) -> bool { self.next_offset.is_none() }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base64ReadPlan {
    pub body: BodyReadPlan,
    /// Zero-based ASCII byte offsets, also usable with SQLite substr (+1).
    pub encoded_start: usize,
    pub encoded_length: usize,
    skip: usize,
}

impl Base64ReadPlan {
    /// `tail` must be the final four ASCII bytes, or empty for an empty body.
    /// Callers with a database/blob can obtain length/tail without loading it.
    pub fn new(encoded_len: usize, tail: &str, offset: u64, length: usize) -> Result<Self> {
        ensure!(encoded_len % 4 == 0, "Invalid Base64 body length");
        ensure!(tail.len() == encoded_len.min(4), "Invalid Base64 body ending");
        let final_bytes = STANDARD.decode(tail).context("Invalid Base64 body ending")?.len();
        let total = if encoded_len == 0 { 0 } else {
            (encoded_len / 4 - 1).checked_mul(3).and_then(|n| n.checked_add(final_bytes))
                .context("Base64 body length overflow")?
        };
        let body = BodyReadPlan::new(u64::try_from(total)?, offset, length)?;
        let start = usize::try_from(body.start).context("Invalid body offset")?;
        let skip = start % 3;
        let (encoded_start, encoded_length) = if body.length == 0 { (encoded_len, 0) } else {
            let from = (start / 3).checked_mul(4).context("Invalid body offset")?;
            let count = (skip + body.length).div_ceil(3) * 4;
            ensure!(from.checked_add(count).is_some_and(|end| end <= encoded_len), "Incomplete Base64 body");
            (from, count)
        };
        Ok(Self { body, encoded_start, encoded_length, skip })
    }

    /// Decode only the projected quanta. Output is at most MAX_READ_BYTES;
    /// alignment uses at most two additional decoded bytes of scratch space.
    pub fn decode_part(&self, encoded: &[u8]) -> Result<Vec<u8>> {
        ensure!(self.body.length <= MAX_READ_BYTES && self.skip < 3
            && self.encoded_length <= (MAX_READ_BYTES + 2).div_ceil(3) * 4,
            "Body read range exceeds 65536 bytes");
        ensure!(encoded.len() == self.encoded_length, "Incomplete Base64 body range");
        if self.body.length == 0 { return Ok(Vec::new()); }
        let mut bytes = STANDARD.decode(encoded).context("Invalid Base64 body range")?;
        let end = self.skip + self.body.length;
        ensure!(bytes.len() >= end, "Incomplete Base64 body range");
        bytes.copy_within(self.skip..end, 0);
        bytes.truncate(self.body.length);
        Ok(bytes)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct BodyReadChunk { pub plan: BodyReadPlan, pub bytes: Vec<u8> }
impl BodyReadChunk {
    /// A byte range may split a UTF-8 character. Never replace invalid bytes in
    /// the exact body or present lossy text as if it were the original payload.
    pub fn utf8(&self) -> Option<&str> { std::str::from_utf8(&self.bytes).ok() }
}

pub fn read_base64_range(encoded: &str, offset: u64, length: usize) -> Result<BodyReadChunk> {
    let tail = encoded.get(encoded.len().saturating_sub(4)..).context("Invalid Base64 body ending")?;
    let plan = Base64ReadPlan::new(encoded.len(), tail, offset, length)?;
    let part = encoded.as_bytes().get(plan.encoded_start..plan.encoded_start + plan.encoded_length)
        .context("Incomplete Base64 body range")?;
    let bytes = plan.decode_part(part)?;
    Ok(BodyReadChunk { plan: plan.body, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_quantum_and_64k_edges_without_decoding_the_rest() {
        let bytes: Vec<u8> = (0..MAX_READ_BYTES * 3 + 7).map(|n| (n % 251) as u8).collect();
        for total in [0, 1, 2, 3, 4, MAX_READ_BYTES - 1, MAX_READ_BYTES, MAX_READ_BYTES + 1, bytes.len()] {
            let encoded = STANDARD.encode(&bytes[..total]);
            for offset in [0, 1, 2, 3, 65535, 65536, total as u64, u64::MAX] {
                for length in [1, 2, 3, MAX_READ_BYTES] {
                    let chunk = read_base64_range(&encoded, offset, length).unwrap();
                    let start = offset.min(total as u64) as usize;
                    assert_eq!(chunk.bytes, bytes[start..(start + length).min(total)]);
                    assert_eq!(chunk.plan.offset, offset);
                    assert_eq!(chunk.plan.total_bytes, total as u64);
                    assert_eq!(chunk.plan.next_offset, ((start + chunk.bytes.len()) < total).then_some((start + chunk.bytes.len()) as u64));
                }
            }
        }
        // The plan does not walk/decode the omitted body. A corrupt excluded
        // middle does not alter a valid explicit range, but is caught when read.
        let mut encoded = STANDARD.encode(&bytes);
        encoded.replace_range(100_000..100_001, "!");
        assert_eq!(read_base64_range(&encoded, 0, 4).unwrap().bytes, bytes[..4]);
        assert!(read_base64_range(&encoded, 75_000, 4).is_err());
        let plan = Base64ReadPlan::new(encoded.len(), &encoded[encoded.len() - 4..], 65535, 65536).unwrap();
        assert!(plan.encoded_length <= (MAX_READ_BYTES + 2).div_ceil(3) * 4);
    }

    #[test]
    fn invalid_encoding_bounds_and_partial_utf8_never_return_lossy_data() {
        for invalid in ["!", "Z", "Zh==", "Zg=", "Zg==AAAA", "Zg==\r\n", "====", "AA=A", "AAAAé"] {
            assert!(read_base64_range(invalid, 0, 65536).is_err(), "accepted invalid encoding");
        }
        assert!(read_base64_range("", 0, 0).is_err());
        assert!(read_base64_range("", 0, MAX_READ_BYTES + 1).is_err());
        assert!(Base64ReadPlan::new(4, "", 0, 1).is_err());
        let plan = Base64ReadPlan::new(4, "YQ==", 0, 1).unwrap();
        assert!(plan.decode_part(b"YQ=").is_err());
        let encoded = STANDARD.encode("A你Z");
        assert_eq!(read_base64_range(&encoded, 1, 3).unwrap().utf8(), Some("你"));
        let partial = read_base64_range(&encoded, 2, 1).unwrap();
        assert_eq!(partial.bytes, [0xbd]);
        assert_eq!(partial.utf8(), None);
        assert_eq!(read_base64_range(&STANDARD.encode([255]), 0, 1).unwrap().utf8(), None);
        let plan = BodyReadPlan::new(u64::MAX, u64::MAX - 1, 65536).unwrap();
        assert_eq!(plan.length, 1);
        assert!(plan.eof());
    }
}
