//! Complete body exports use the same snapshot as the checked flow revision.
//! A destination is published only after every body byte has been validated.
use super::{body_views, Store};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rusqlite::{params, Connection};
use std::{io::Write, path::Path};

const COPY_CHUNK: usize = 64 * 1024;

impl Store {
    /// Copy the complete, exact entity bytes of one versioned body. Writers may
    /// apply backpressure and must return an error when their consumer cancels.
    /// This does not flush or close the caller's writer. An error can leave a
    /// prefix there; the caller must discard it, rather than label it complete.
    pub fn copy_body_at_revision(
        &self,
        id: &str,
        variant: &str,
        expected_version: &str,
        writer: &mut impl Write,
    ) -> Result<u64> {
        let mut conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let tx = conn.transaction()?;
        body_views::check_revision(&tx, id, expected_version, &self.1)?;
        let size = match body_views::body_location(&tx, id, variant)? {
            body_views::BodyLocation::Blob { rowid, length } => {
                ensure!(length <= crate::body_export::MAX_EXPORT_BYTES, "Export exceeds 256 MiB limit");
                let blob = tx.blob_open(rusqlite::DatabaseName::Main, "flow_bodies", "body", rowid, true)?;
                ensure!(blob.len() as u64 == length, "Stored body length changed");
                let mut chunk = [0u8; COPY_CHUNK];
                let mut offset = 0u64;
                while offset < length {
                    let count = usize::try_from((length - offset).min(COPY_CHUNK as u64))?;
                    blob.read_at_exact(&mut chunk[..count], usize::try_from(offset)?)?;
                    writer.write_all(&chunk[..count])?;
                    offset += count as u64;
                }
                offset
            }
            body_views::BodyLocation::Inline { encoded_length, tail } => {
                // Plan only the length here. The loop validates every quantum,
                // including the middle of large bodies from older databases.
                let plan = crate::body_read::Base64ReadPlan::new(
                    encoded_length,
                    std::str::from_utf8(&tail).context("Invalid stored body ending")?,
                    0,
                    1,
                )?;
                ensure!(plan.body.total_bytes <= crate::body_export::MAX_EXPORT_BYTES, "Export exceeds 256 MiB limit");
                copy_inline(&tx, id, body_views::body_slot(variant)?, encoded_length, plan.body.total_bytes, writer)?
            }
        };
        tx.commit()?;
        Ok(size)
    }

    /// Save a complete body without overwriting an existing destination. The
    /// private staging file is removed on errors or unwinding, including when
    /// the revision became stale before reading. No body path comes from JSON.
    pub fn save_body_at_revision(
        &self,
        id: &str,
        variant: &str,
        expected_version: &str,
        path: &Path,
    ) -> Result<u64> {
        ensure!(path.file_name().is_some(), "Export requires a file name");
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::Builder::new().prefix(".http-capture-body-").tempfile_in(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary.as_file().set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        let size = self.copy_body_at_revision(id, variant, expected_version, temporary.as_file_mut())?;
        temporary.as_file_mut().flush()?;
        temporary.as_file().sync_all()?;
        // persist_noclobber returns ownership of the staging file on failure;
        // dropping that error also closes/removes it, leaving the old file intact.
        temporary.persist_noclobber(path).map_err(|error| error.error)?;
        Ok(size)
    }
}

fn copy_inline(
    conn: &Connection,
    id: &str,
    json_path: &str,
    encoded_length: usize,
    expected_bytes: u64,
    writer: &mut impl Write,
) -> Result<u64> {
    let mut copied = 0u64;
    let mut decoded = [0u8; COPY_CHUNK / 4 * 3];
    // A 64 KiB encoded window consists of complete Base64 quanta. Its decoded
    // buffer is at most 48 KiB; no complete Base64 String or decoded Vec exists.
    let mut statement = conn.prepare(
        "SELECT substr(CAST(json_extract(data,?2) AS BLOB),?3,?4) FROM flows WHERE id=?1",
    )?;
    let mut offset = 0usize;
    while offset < encoded_length {
        let count = (encoded_length - offset).min(COPY_CHUNK);
        let encoded: Vec<u8> = statement.query_row(
            params![id, json_path, i64::try_from(offset)? + 1, i64::try_from(count)?],
            |row| row.get(0),
        )?;
        ensure!(encoded.len() == count, "Incomplete stored body range");
        let final_chunk = offset + count == encoded_length;
        ensure!(final_chunk || !encoded.contains(&b'='), "Invalid Base64 padding before body ending");
        let size = STANDARD.decode_slice(&encoded, &mut decoded).context("Invalid stored Base64 body")?;
        copied = copied.checked_add(size as u64).context("Body export length overflow")?;
        ensure!(copied <= expected_bytes && copied <= crate::body_export::MAX_EXPORT_BYTES, "Invalid body export length");
        writer.write_all(&decoded[..size])?;
        offset += count;
    }
    ensure!(copied == expected_bytes, "Incomplete stored body");
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Flow;
    use sha2::{Digest, Sha256};
    use std::io;

    fn sample(id: &str, body: &[u8]) -> Flow {
        serde_json::from_value(serde_json::json!({
            "id":id,"parentId":null,"startedAt":1,"durationMs":0,"source":"capture","state":"completed",
            "request":{"method":"POST","url":"https://example.invalid/","headers":[],"bodyBase64":STANDARD.encode(body)},
            "response":null,"error":null,"notes":[]
        })).unwrap()
    }

    fn version(store: &Store, id: &str) -> String {
        let value: i64 = store.0.lock().unwrap().query_row(
            "SELECT version FROM flow_changes WHERE id=?1", [id], |row| row.get(0),
        ).unwrap();
        format!("{}:{value}", store.1)
    }

    fn legacy_body(store: &Store, id: &str, encoded: &str) {
        store.0.lock().unwrap().execute(
            "UPDATE flows SET data=json_set(data,'$.request.bodyBase64',?2) WHERE id=?1",
            params![id, encoded],
        ).unwrap();
    }

    fn pattern(length: usize, seed: usize) -> Vec<u8> {
        (0..length).map(|index| ((index + seed) % 251) as u8).collect()
    }

    #[test]
    fn exports_blob_and_legacy_bodies_across_chunks_with_all_padding_lengths() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("store.db")).unwrap();
        for length in [0, 1, 2, 3, COPY_CHUNK - 1, COPY_CHUNK, COPY_CHUNK + 1, 3 * COPY_CHUNK + 2] {
            let bytes = pattern(length, 7);
            let flow = sample("export", &bytes);
            store.insert(&flow).unwrap();
            for legacy in [false, true] {
                if legacy { legacy_body(&store, &flow.id, &STANDARD.encode(&bytes)); }
                let mut output = Vec::new();
                let count = store.copy_body_at_revision(&flow.id, "request", &version(&store, &flow.id), &mut output).unwrap();
                assert_eq!(count, bytes.len() as u64);
                assert_eq!(Sha256::digest(&output), Sha256::digest(&bytes));
                assert_eq!(output, bytes);
            }
        }
    }

    #[test]
    fn all_four_slots_export_their_own_original_or_modified_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("store.db")).unwrap();
        let bodies = [pattern(COPY_CHUNK + 1, 1), pattern(COPY_CHUNK + 2, 2), pattern(COPY_CHUNK + 3, 3), pattern(COPY_CHUNK + 4, 4)];
        let mut flow = sample("slots", &bodies[0]);
        flow.original_request = Some(sample("original", &bodies[2]).request);
        let response = |bytes: &[u8]| serde_json::from_value(serde_json::json!({
            "status":200,"version":"HTTP/2","headers":[],"bodyBase64":STANDARD.encode(bytes)
        })).unwrap();
        flow.response = Some(response(&bodies[1]));
        flow.original_response = Some(response(&bodies[3]));
        store.insert(&flow).unwrap();
        let revision = version(&store, &flow.id);
        for (slot, expected) in ["request", "response", "originalRequest", "originalResponse"].into_iter().zip(bodies) {
            let path = directory.path().join(format!("{slot}.bin"));
            assert_eq!(store.save_body_at_revision(&flow.id, slot, &revision, &path).unwrap(), expected.len() as u64);
            assert_eq!(std::fs::read(&path).unwrap(), expected);
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            }
        }
    }

    #[test]
    fn corrupt_legacy_middle_or_padding_cannot_publish_a_partial_file() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("store.db")).unwrap();
        let flow = sample("legacy", &[]);
        store.insert(&flow).unwrap();
        let correct = STANDARD.encode(pattern(3 * COPY_CHUNK + 1, 3));
        let mut bad_middle = correct.clone();
        bad_middle.replace_range(COPY_CHUNK + 40..COPY_CHUNK + 41, "!");
        let mut premature_padding = correct.clone();
        premature_padding.replace_range(COPY_CHUNK - 4..COPY_CHUNK, "AA==");
        let mut bad_tail = correct.clone();
        bad_tail.replace_range(bad_tail.len() - 4.., "AB==");
        for (index, encoded) in [bad_middle, premature_padding, bad_tail, "A".into()].into_iter().enumerate() {
            legacy_body(&store, &flow.id, &encoded);
            let path = directory.path().join(format!("bad-{index}.bin"));
            let before = std::fs::read_dir(directory.path()).unwrap().count();
            assert!(store.save_body_at_revision(&flow.id, "request", &version(&store, &flow.id), &path).is_err());
            assert!(!path.exists());
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), before);
        }
    }

    #[test]
    fn stale_deleted_wrong_instance_and_missing_or_bad_slots_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("store.db");
        let store = Store::open(&database).unwrap();
        let mut flow = sample("revision", b"first");
        store.insert(&flow).unwrap();
        let old = version(&store, &flow.id);
        flow.request.body_base64 = STANDARD.encode(b"second").into();
        store.insert(&flow).unwrap();
        let current = version(&store, &flow.id);
        let target = directory.path().join("stale.bin");
        assert!(store.save_body_at_revision(&flow.id, "request", &old, &target).is_err());
        assert!(!target.exists());
        for slot in ["response", "originalRequest", "originalResponse", "../../private", "request.bodyBase64"] {
            assert!(store.copy_body_at_revision(&flow.id, slot, &current, &mut Vec::new()).is_err());
        }
        let second = Store::open(&database).unwrap();
        assert!(second.copy_body_at_revision(&flow.id, "request", &current, &mut Vec::new()).is_err());
        legacy_body(&store, &flow.id, "@body:response");
        assert!(store.copy_body_at_revision(&flow.id, "request", &version(&store, &flow.id), &mut Vec::new()).is_err());
        legacy_body(&store, &flow.id, "@body:request");
        assert!(store.copy_body_at_revision(&flow.id, "request", &version(&store, &flow.id), &mut Vec::new()).is_err());
        store.delete_flows(vec![flow.id.clone()]).unwrap();
        assert!(store.copy_body_at_revision(&flow.id, "request", &current, &mut Vec::new()).is_err());
    }

    struct FailingWriter { remaining: usize, largest_write: usize }
    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.largest_write = self.largest_write.max(bytes.len());
            if self.remaining == 0 { return Err(io::Error::new(io::ErrorKind::BrokenPipe, "consumer cancelled")); }
            let size = bytes.len().min(self.remaining);
            self.remaining -= size;
            Ok(size)
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }

    #[test]
    fn writer_failure_releases_transaction_and_existing_destinations_survive() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("store.db")).unwrap();
        let flow = sample("cancel", &pattern(4 * COPY_CHUNK + 2, 1));
        store.insert(&flow).unwrap();
        for legacy in [false, true] {
            if legacy { legacy_body(&store, &flow.id, &flow.request.body_base64); }
            let mut writer = FailingWriter { remaining: COPY_CHUNK + 1, largest_write: 0 };
            let error = store.copy_body_at_revision(&flow.id, "request", &version(&store, &flow.id), &mut writer).unwrap_err();
            assert!(error.to_string().contains("consumer cancelled"));
            assert!(writer.largest_write <= COPY_CHUNK);
            assert!(store.0.try_lock().is_ok());
            // A later write proves the failed read transaction was rolled back.
            store.insert(&flow).unwrap();
        }
        let path = directory.path().join("existing.bin");
        std::fs::write(&path, b"keep existing file").unwrap();
        let before = std::fs::read_dir(directory.path()).unwrap().count();
        assert!(store.save_body_at_revision(&flow.id, "request", &version(&store, &flow.id), &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"keep existing file");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), before);
    }
}
