//! Private, handle-owned scratch storage. No paths are accepted from the UI.
//! Anonymous/delete-on-close temporary files disappear on cancellation and exit.
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, write::EncoderStringWriter};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};

const MEMORY_LIMIT: usize = 64 * 1024;

pub(crate) struct BodySpool {
    file: Option<tokio::fs::File>,
    memory: std::io::Cursor<Vec<u8>>,
    needs_rewind: bool,
    failed: bool,
    pub len: u64,
}

// A cancelled write may already have appended part of its input. Discard it
// rather than allowing the caller to send a body whose recorded length is stale.
struct AppendGuard<'a> {
    spool: &'a mut BodySpool,
    complete: bool,
}
impl Drop for AppendGuard<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.spool.file = None;
            self.spool.memory = std::io::Cursor::new(Vec::new());
            self.spool.len = 0;
            self.spool.needs_rewind = false;
            self.spool.failed = true;
        }
    }
}

impl BodySpool {
    pub fn empty() -> Self {
        Self {
            file: None,
            memory: std::io::Cursor::new(Vec::new()),
            needs_rewind: false,
            failed: false,
            len: 0,
        }
    }

    pub async fn append(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(!self.failed, "Temporary body was discarded after an incomplete append");
        if bytes.is_empty() {
            return Ok(());
        }
        let mut guard = AppendGuard { spool: self, complete: false };
        guard.spool.append_inner(bytes).await?;
        guard.complete = true;
        Ok(())
    }

    async fn append_inner(&mut self, bytes: &[u8]) -> Result<()> {
        let len = self.len.checked_add(u64::try_from(bytes.len())?)
            .ok_or_else(|| anyhow::anyhow!("Temporary body length overflow"))?;
        if self.file.is_none() && len <= MEMORY_LIMIT as u64 {
            // Avoid Vec's geometric growth exceeding the in-memory ceiling.
            self.memory.get_mut().try_reserve_exact(bytes.len())?;
            self.memory.get_mut().extend_from_slice(bytes);
            self.memory.set_position(0);
        } else {
            if self.file.is_none() {
                // Unlinked on Unix and delete-on-close on Windows; only this
                // handle is retained, and no external path can be supplied.
                let file = tokio::task::spawn_blocking(tempfile::tempfile).await??;
                let mut file = tokio::fs::File::from_std(file);
                file.write_all(self.memory.get_ref()).await?;
                self.file = Some(file);
                self.memory = std::io::Cursor::new(Vec::new());
                self.needs_rewind = true;
            }
            let file = self.file.as_mut().unwrap();
            if !self.needs_rewind {
                // Appending after a read starts subsequent reads at byte zero.
                file.seek(std::io::SeekFrom::End(0)).await?;
            }
            file.write_all(bytes).await?;
            // Tokio file writes are buffered in a blocking task. Observe any
            // delayed write error before committing the new body length.
            file.flush().await?;
            self.needs_rewind = true;
        }
        self.len = len;
        Ok(())
    }

    pub async fn decode(encoded: &str) -> Result<Self> {
        let mut result = Self::empty();
        let mut decoder = base64::read::DecoderReader::new(encoded.as_bytes(), &STANDARD);
        let mut chunk = vec![0; MEMORY_LIMIT];
        loop {
            let count = std::io::Read::read(&mut decoder, &mut chunk)?;
            if count == 0 {
                break;
            }
            result.append(&chunk[..count]).await?;
        }
        Ok(result)
    }

    async fn rewind(&mut self) -> Result<()> {
        ensure!(!self.failed, "Temporary body was discarded after an incomplete append");
        if let Some(file) = &mut self.file {
            file.rewind().await?;
        }
        self.memory.set_position(0);
        self.needs_rewind = false;
        Ok(())
    }

    pub async fn next_chunk(&mut self) -> Result<Vec<u8>> {
        ensure!(!self.failed, "Temporary body was discarded after an incomplete append");
        if self.needs_rewind {
            self.rewind().await?;
        }
        let mut chunk = vec![0; MEMORY_LIMIT];
        let count = match &mut self.file {
            Some(file) => file.read(&mut chunk).await?,
            None => std::io::Read::read(&mut self.memory, &mut chunk)?,
        };
        chunk.truncate(count);
        Ok(chunk)
    }

    pub async fn send<W: AsyncWrite + Unpin + ?Sized>(&mut self, writer: &mut W) -> Result<()> {
        self.rewind().await?;
        let copied = if let Some(file) = &mut self.file {
            tokio::io::copy(file, writer).await?
        } else {
            tokio::io::copy(&mut self.memory, writer).await?
        };
        ensure!(copied == self.len, "Incomplete temporary body");
        Ok(())
    }

    pub async fn into_base64(mut self) -> Result<String> {
        self.rewind().await?;
        let mut encoder = EncoderStringWriter::new(&STANDARD);
        let mut copied = 0u64;
        loop {
            let chunk = self.next_chunk().await?;
            if chunk.is_empty() {
                break;
            }
            copied = copied.checked_add(chunk.len() as u64)
                .ok_or_else(|| anyhow::anyhow!("Temporary body length overflow"))?;
            ensure!(copied <= self.len, "Temporary body exceeds recorded length");
            // The encoder retains incomplete triples between chunks and adds
            // padding only once. No complete decoded binary Vec is constructed.
            std::io::Write::write_all(&mut encoder, &chunk)?;
        }
        ensure!(copied == self.len, "Incomplete temporary body");
        Ok(encoder.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use sha2::{Digest, Sha256};

    fn pattern(offset: usize, length: usize) -> Vec<u8> {
        (offset..offset + length).map(|i| (i % 251) as u8).collect()
    }

    fn file_spool(file: std::fs::File) -> BodySpool {
        BodySpool { file: Some(tokio::fs::File::from_std(file)), ..BodySpool::empty() }
    }

    #[tokio::test]
    async fn threshold_uses_binary_bytes_and_spills_only_when_exceeded() {
        let bytes = pattern(0, MEMORY_LIMIT);
        let mut spool = BodySpool::empty();
        spool.append(&bytes[..MEMORY_LIMIT - 1]).await.unwrap();
        assert!(spool.file.is_none());
        spool.append(&bytes[MEMORY_LIMIT - 1..]).await.unwrap();
        assert_eq!(spool.len, MEMORY_LIMIT as u64);
        assert!(spool.file.is_none());
        assert_eq!(spool.memory.get_ref().len(), MEMORY_LIMIT);
        // This base64 text is longer than 64 KiB, but its decoded body is not.
        let decoded = BodySpool::decode(&STANDARD.encode(&bytes)).await.unwrap();
        assert!(decoded.file.is_none());
        spool.append(b"!").await.unwrap();
        assert!(spool.file.is_some());
        assert!(spool.memory.get_ref().is_empty());
        assert_eq!(spool.len, MEMORY_LIMIT as u64 + 1);
        let mut expected = bytes;
        expected.push(b'!');
        assert_eq!(spool.into_base64().await.unwrap(), STANDARD.encode(expected));
    }

    #[tokio::test]
    async fn uneven_memory_appends_never_allocate_above_the_threshold() {
        let mut spool = BodySpool::empty();
        for length in [40 * 1024, 20 * 1024, 4 * 1024] {
            spool.append(&vec![7; length]).await.unwrap();
            assert!(spool.file.is_none());
            assert!(spool.memory.get_ref().capacity() <= MEMORY_LIMIT);
        }
        assert_eq!(spool.len, MEMORY_LIMIT as u64);
        spool.append(b"!").await.unwrap();
        assert!(spool.file.is_some());
        assert_eq!(spool.memory.get_ref().capacity(), 0);
    }

    #[tokio::test]
    async fn arbitrary_append_boundaries_preserve_padding_and_empty_bodies() {
        for length in [0, 1, 2, 3, 4, 5, 7, MEMORY_LIMIT - 1, MEMORY_LIMIT, MEMORY_LIMIT + 1, MEMORY_LIMIT + 2] {
            let bytes = pattern(0, length);
            for chunk_size in [1, 2, 3, 127, MEMORY_LIMIT - 1, MEMORY_LIMIT + 7] {
                let mut spool = BodySpool::empty();
                for chunk in bytes.chunks(chunk_size) {
                    spool.append(chunk).await.unwrap();
                    spool.append(&[]).await.unwrap();
                }
                assert_eq!(spool.len, length as u64);
                assert_eq!(spool.into_base64().await.unwrap(), STANDARD.encode(&bytes),
                    "length {length}, chunk size {chunk_size}");
            }
        }
        let mut empty = BodySpool::decode("").await.unwrap();
        assert!(empty.next_chunk().await.unwrap().is_empty());
        empty.send(&mut tokio::io::sink()).await.unwrap();
        assert_eq!(empty.into_base64().await.unwrap(), "");
    }

    #[tokio::test]
    async fn large_generated_stream_preserves_hash_without_a_complete_binary_buffer() {
        let length = 9 * 1024 * 1024 + 2;
        let mut expected_hash = Sha256::new();
        let mut spool = BodySpool::empty();
        let mut offset = 0;
        while offset < length {
            let chunk = pattern(offset, (length - offset).min(8191));
            expected_hash.update(&chunk);
            spool.append(&chunk).await.unwrap();
            assert!(spool.memory.get_ref().len() <= MEMORY_LIMIT);
            offset += chunk.len();
        }
        assert!(spool.file.is_some());
        assert_eq!(spool.len, length as u64);
        let encoded = spool.into_base64().await.unwrap();
        assert_eq!(encoded.len(), 4 * length.div_ceil(3));
        let mut decoder = base64::read::DecoderReader::new(encoded.as_bytes(), &STANDARD);
        let mut actual_hash = Sha256::new();
        let mut count = 0;
        let mut chunk = vec![0; 7919];
        loop {
            let read = std::io::Read::read(&mut decoder, &mut chunk).unwrap();
            if read == 0 {
                break;
            }
            actual_hash.update(&chunk[..read]);
            count += read;
        }
        assert_eq!(count, length);
        assert_eq!(actual_hash.finalize(), expected_hash.finalize());
    }

    #[tokio::test]
    async fn binary_roundtrip_rewind_and_invalid_tail() {
        let bytes = pattern(0, 9 * 1024 * 1024 + 2);
        let encoded = STANDARD.encode(&bytes);
        let mut spool = BodySpool::decode(&encoded).await.unwrap();
        assert_eq!(spool.len, bytes.len() as u64);
        assert_eq!(spool.next_chunk().await.unwrap(), bytes[..MEMORY_LIMIT]);
        for _ in 0..2 {
            let mut out = Vec::new();
            spool.send(&mut out).await.unwrap();
            assert_eq!(out, bytes);
        }
        assert_eq!(spool.into_base64().await.unwrap(), encoded);
        let mut bad = encoded;
        let n = bad.len();
        bad.replace_range(n - 8..n - 7, "!");
        assert!(BodySpool::decode(&bad).await.is_err());
        for invalid in ["!", "Z", "Zh==", "Zg=", "Zg==AAAA", "Zg==\r\n"] {
            assert!(BodySpool::decode(invalid).await.is_err(), "{invalid:?}");
        }
    }

    #[tokio::test]
    async fn append_after_partial_read_rewinds_without_overwriting_existing_bytes() {
        for length in [5, MEMORY_LIMIT + 5] {
            let bytes = pattern(0, length);
            let mut spool = BodySpool::empty();
            spool.append(&bytes).await.unwrap();
            assert!(!spool.next_chunk().await.unwrap().is_empty());
            spool.append(b"tail").await.unwrap();
            let mut expected = bytes;
            expected.extend_from_slice(b"tail");
            assert_eq!(spool.into_base64().await.unwrap(), STANDARD.encode(expected));
        }
    }

    #[tokio::test]
    async fn failed_append_discards_the_handle_and_cannot_send_partial_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("read-only");
        std::fs::write(&path, b"existing").unwrap();
        let mut spool = file_spool(std::fs::File::open(&path).unwrap());
        assert!(spool.append(b"cannot write").await.is_err());
        assert!(spool.file.is_none());
        assert_eq!(spool.len, 0);
        assert!(spool.append(b"retry").await.is_err());
        assert!(spool.send(&mut tokio::io::sink()).await.is_err());
        assert!(spool.into_base64().await.is_err());
        std::fs::remove_file(path).unwrap();
        dir.close().unwrap();
    }

    #[tokio::test]
    async fn incomplete_file_is_not_returned_as_a_successful_body() {
        let mut file = tempfile::tempfile().unwrap();
        std::io::Write::write_all(&mut file, b"short").unwrap();
        let mut spool = file_spool(file);
        spool.len = 6;
        assert!(spool.send(&mut tokio::io::sink()).await.is_err());
        assert!(spool.into_base64().await.is_err());
    }

    #[tokio::test]
    async fn cancel_blocked_transfer_releases_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = tempfile::tempfile_in(dir.path()).unwrap();
        let mut spool = file_spool(file);
        spool.append(&vec![1; 1024 * 1024]).await.unwrap();
        let (mut writer, mut reader) = tokio::io::duplex(1);
        let task = tokio::spawn(async move { spool.send(&mut writer).await });
        // Receiving a byte proves the task is actually blocked in its transfer,
        // with the temporary body alive, before cancellation.
        reader.read_u8().await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        // On Windows this also rejects a leaked delete-on-close file handle.
        dir.close().unwrap();
    }
}
