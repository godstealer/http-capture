//! Owner UI projections. An unloaded entity is a reference, never an empty Flow
//! body. Legacy Flow/JSON readers keep their full-payload semantics.
use super::{McpBodyChunk, Store};
use crate::body_read::{Base64ReadPlan, BodyReadPlan};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const VARIANTS: [&str; 4] = ["request", "response", "originalRequest", "originalResponse"];
const METADATA_SQL: &str = "CASE WHEN p.version=coalesce(c.version,0) THEN p.metadata ELSE json_remove(f.data,'$.request.bodyBase64','$.response.bodyBase64','$.originalRequest.bodyBase64','$.originalResponse.bodyBase64') END";
const REFS_SQL: &str = "CASE WHEN p.version=coalesce(c.version,0) THEN p.body_refs ELSE NULL END";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyReference { pub byte_length: u64 }

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowMetadata {
    /// Body fields are absent. This cannot be used as a RequestDraft/Flow to
    /// send, persist or export without an explicit complete-body read.
    #[serde(flatten)]
    pub metadata: Value,
    pub body_revision: String,
    pub body_references: BTreeMap<String, BodyReference>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FlowMetadataChange { pub position: i64, pub flow: FlowMetadata }
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FlowMetadataChanges {
    pub revision: String,
    pub reset: bool,
    pub rows: Vec<FlowMetadataChange>,
    pub deleted: Vec<String>,
}

fn valid_id(id: &str) -> Result<()> {
    ensure!(!id.is_empty() && id.len() <= 128, "Invalid recorded flow ID");
    Ok(())
}

/// Resolve only fixed variants. No caller-provided JSON or filesystem path.
pub(super) fn body_slot(variant: &str) -> Result<&'static str> {
    match variant {
        "request" => Ok("$.request.bodyBase64"),
        "response" => Ok("$.response.bodyBase64"),
        "originalRequest" => Ok("$.originalRequest.bodyBase64"),
        "originalResponse" => Ok("$.originalResponse.bodyBase64"),
        _ => anyhow::bail!("Body variant must be request, response, originalRequest or originalResponse"),
    }
}

fn row_revision(conn: &Connection, id: &str, instance: &str) -> Result<String> {
    valid_id(id)?;
    let version: i64 = conn.query_row(
        "SELECT coalesce(c.version,0) FROM flows f LEFT JOIN flow_changes c ON c.id=f.id WHERE f.id=?1",
        [id], |row| row.get(0),
    ).optional()?.context("Recorded flow not found")?;
    Ok(format!("{instance}:{version}"))
}

/// Call inside the same transaction as the metadata/body access. Checking a
/// version in a separate transaction can mix SSE updates or a deleted/reused row.
pub(super) fn check_revision(conn: &Connection, id: &str, expected: &str, instance: &str) -> Result<()> {
    ensure!(!expected.is_empty() && expected.len() <= 128, "Invalid body revision");
    ensure!(row_revision(conn, id, instance)? == expected,
        "Recorded flow changed; reload its metadata before reading the body");
    Ok(())
}

pub(super) enum BodyLocation {
    Blob { rowid: i64, length: u64 },
    Inline { encoded_length: usize, tail: Vec<u8> },
}

/// Find lengths and a bounded tail without restoring the Flow or fetching a
/// complete inline Base64 string into Rust. SQL still parses legacy JSON rows.
pub(super) fn body_location(conn: &Connection, id: &str, variant: &str) -> Result<BodyLocation> {
    valid_id(id)?;
    let path = body_slot(variant)?;
    let (kind, length, tail, marker): (Option<String>, Option<usize>, Option<Vec<u8>>, Option<String>) = conn.query_row(
        "SELECT json_type(data,?2),length(CAST(json_extract(data,?2) AS BLOB)),substr(CAST(json_extract(data,?2) AS BLOB),-4),
         CASE WHEN substr(json_extract(data,?2),1,6)='@body:' THEN json_extract(data,?2) END FROM flows WHERE id=?1",
        params![id, path], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional()?.context("Recorded flow not found")?;
    ensure!(kind.as_deref() == Some("text"), "Requested body is unavailable");
    if let Some(marker) = marker {
        ensure!(marker == format!("@body:{variant}"), "Invalid stored body reference");
        let (rowid, length, capacity, kind): (i64, u64, u64, String) = conn.query_row(
            "SELECT rowid,coalesce(logical_length,length(body)),length(body),typeof(body) FROM flow_bodies WHERE flow_id=?1 AND slot=?2",
            params![id, variant], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).optional()?.context("Stored body is missing")?;
        ensure!(kind == "blob", "Invalid stored body data");
        ensure!(length<=capacity,"Stored body length exceeds BLOB capacity");
        Ok(BodyLocation::Blob { rowid, length })
    } else {
        let encoded_length = length.context("Requested body is unavailable")?;
        let tail = tail.unwrap_or_default();
        Base64ReadPlan::new(encoded_length, std::str::from_utf8(&tail).context("Invalid stored body ending")?, 0, 1)?;
        Ok(BodyLocation::Inline { encoded_length, tail })
    }
}

fn metadata_projection(conn: &Connection, raw: String, cached_refs: Option<String>, instance: &str, version: i64) -> Result<FlowMetadata> {
    let metadata: Value = serde_json::from_str(&raw)?;
    let id = metadata["id"].as_str().context("Invalid recorded flow metadata")?;
    if let Some(cached_refs) = cached_refs {
        let body_references = serde_json::from_str(&cached_refs)?;
        return Ok(FlowMetadata { metadata, body_revision: format!("{instance}:{version}"), body_references });
    }
    let mut body_references = BTreeMap::new();
    for variant in VARIANTS {
        let path = body_slot(variant)?;
        let kind: Option<String> = conn.query_row("SELECT json_type(data,?2) FROM flows WHERE id=?1",
            params![id, path], |row| row.get(0))?;
        if kind.is_none() { continue; }
        let byte_length = match body_location(conn, id, variant)? {
            BodyLocation::Blob { length, .. } => length,
            BodyLocation::Inline { encoded_length, tail } => Base64ReadPlan::new(encoded_length,
                std::str::from_utf8(&tail).context("Invalid stored body ending")?, 0, 1)?.body.total_bytes,
        };
        body_references.insert(variant.into(), BodyReference { byte_length });
    }
    Ok(FlowMetadata { metadata, body_revision: format!("{instance}:{version}"), body_references })
}

impl Store {
    /// Complete history, with the same row order, reset token and tombstones as
    /// changes(). There is deliberately no MCP page limit or history cutoff.
    pub fn metadata_changes(&self, since: Option<&str>) -> Result<FlowMetadataChanges> {
        let mut conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let tx = conn.transaction()?;
        let version: i64 = tx.query_row("SELECT version FROM flow_revision WHERE id=1", [], |row| row.get(0))?;
        let previous = since.and_then(|value| value.strip_prefix(&format!("{}:", self.1)))
            .and_then(|value| value.parse::<i64>().ok()).filter(|value| *value >= 0 && *value <= version);
        let reset = previous.is_none();
        let sql = format!("SELECT f.rowid,{METADATA_SQL},coalesce(c.version,0),{REFS_SQL} FROM flows f LEFT JOIN flow_changes c ON c.id=f.id LEFT JOIN flow_projection p ON p.flow_id=f.id {} ORDER BY f.rowid DESC",
            if reset { "" } else { "WHERE c.version>?1" });
        let rows = {
            let mut statement = tx.prepare(&sql)?;
            let mut cursor = if reset { statement.query([])? } else { statement.query([previous.unwrap()])? };
            let mut rows = Vec::new();
            while let Some(row) = cursor.next()? {
                rows.push(FlowMetadataChange { position: row.get(0)?,
                    flow: metadata_projection(&tx, row.get(1)?, row.get(3)?, &self.1, row.get(2)?)? });
            }
            rows
        };
        let deleted = if let Some(previous) = previous {
            let mut statement = tx.prepare("SELECT c.id FROM flow_changes c LEFT JOIN flows f ON f.id=c.id WHERE c.version>?1 AND f.id IS NULL")?;
            let rows = statement.query_map([previous], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<String>>>()?
        } else { Vec::new() };
        tx.commit()?;
        Ok(FlowMetadataChanges { revision: format!("{}:{version}", self.1), reset, rows, deleted })
    }

    pub fn metadata(&self, id: &str, expected_version: Option<&str>) -> Result<Option<FlowMetadata>> {
        valid_id(id)?;
        let mut conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let tx = conn.transaction()?;
        let sql = format!("SELECT {METADATA_SQL},coalesce(c.version,0),{REFS_SQL} FROM flows f LEFT JOIN flow_changes c ON c.id=f.id LEFT JOIN flow_projection p ON p.flow_id=f.id WHERE f.id=?1");
        let row: Option<(String, i64, Option<String>)> = tx.query_row(&sql, [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
        let result = if let Some((raw, version, cached_refs)) = row {
            if let Some(expected) = expected_version { check_revision(&tx, id, expected, &self.1)?; }
            Some(metadata_projection(&tx, raw, cached_refs, &self.1, version)?)
        } else { None };
        tx.commit()?;
        Ok(result)
    }

    /// Explicit compatibility operation. Full data is restored only after a
    /// same-transaction version check, never from an unloaded metadata DTO.
    pub fn get_at_revision(&self, id: &str, expected_version: &str) -> Result<crate::model::Flow> {
        let mut conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let tx = conn.transaction()?;
        check_revision(&tx, id, expected_version, &self.1)?;
        let raw: String = tx.query_row("SELECT data FROM flows WHERE id=?1", [id], |row| row.get(0))?;
        let flow = super::restore_bodies(&tx, &raw)?;
        tx.commit()?;
        Ok(flow)
    }

    pub fn read_body_at_revision(&self, id: &str, variant: &str, expected_version: &str, offset: u64, length: usize) -> Result<McpBodyChunk> {
        // Validate even when the selected body happens to be empty or missing.
        BodyReadPlan::new(0, offset, length)?;
        let path = body_slot(variant)?;
        let mut conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let tx = conn.transaction()?;
        check_revision(&tx, id, expected_version, &self.1)?;
        let (plan, bytes) = match body_location(&tx, id, variant)? {
            BodyLocation::Blob { rowid, length: total } => {
                let blob = tx.blob_open(rusqlite::DatabaseName::Main, "flow_bodies", "body", rowid, true)?;
                let plan = BodyReadPlan::new(total, offset, length)?;
                let mut bytes = vec![0; plan.length];
                if !bytes.is_empty() { blob.read_at_exact(&mut bytes, usize::try_from(plan.start)?)?; }
                (plan, bytes)
            }
            BodyLocation::Inline { encoded_length, tail } => {
                let range = Base64ReadPlan::new(encoded_length, std::str::from_utf8(&tail).context("Invalid stored body ending")?, offset, length)?;
                let bytes = if range.body.length == 0 { Vec::new() } else {
                    let part: Vec<u8> = tx.query_row("SELECT substr(CAST(json_extract(data,?2) AS BLOB),?3,?4) FROM flows WHERE id=?1",
                        params![id, path, range.encoded_start + 1, range.encoded_length], |row| row.get(0))?;
                    range.decode_part(&part)?
                };
                (range.body, bytes)
            }
        };
        tx.commit()?;
        Ok(McpBodyChunk { offset: plan.offset, total_bytes: plan.total_bytes, body_base64: STANDARD.encode(&bytes),
            text: String::from_utf8(bytes).ok(), next_offset: plan.next_offset })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Flow;
    use serde_json::json;

    fn store(directory: &std::path::Path) -> Store { Store::open(&directory.join("views.db")).unwrap() }
    fn flow(id: &str, length: usize) -> Flow {
        let request: Vec<u8> = (0..length).map(|index| (index % 251) as u8).collect();
        let response: Vec<u8> = (0..length + 1).map(|index| ((index + 17) % 251) as u8).collect();
        let request = json!({"method":"POST","url":"https://example.invalid/path","headers":[{"name":"X-A","value":"one"},{"name":"X-A","value":"two","disabled":true}],"bodyBase64":STANDARD.encode(request)});
        let response = json!({"status":200,"version":"HTTP/2","headers":[{"name":"Content-Type","value":"application/octet-stream"}],"bodyBase64":STANDARD.encode(response)});
        let mut original_request = request.clone(); original_request["bodyBase64"] = json!("");
        let mut original_response = response.clone(); original_response["bodyBase64"] = json!(STANDARD.encode([0, 255, 3, 9]));
        serde_json::from_value(json!({"id":id,"startedAt":1,"durationMs":12,"source":"capture","state":"completed", "parentId":null,
            "request":request,"response":response,"originalRequest":original_request,"originalResponse":original_response,"error":null,"notes":["keep metadata"],
            "trafficOrigin":{"entry":"explicit-proxy","clientAddress":"127.0.0.1:1234","process":null,"processStatus":"unknown"}})).unwrap()
    }
    fn body<'a>(value: &'a Value, variant: &str) -> &'a str { value[variant]["bodyBase64"].as_str().unwrap() }
    fn without_bodies(mut value: Value) -> Value {
        for variant in VARIANTS {
            if let Some(object) = value.get_mut(variant).and_then(Value::as_object_mut) { object.remove("bodyBase64"); }
        }
        value
    }

    #[test]
    fn metadata_keeps_every_history_row_and_non_body_field_without_payloads() {
        let directory = tempfile::tempdir().unwrap(); let store = store(directory.path());
        for index in 0..251 {
            let mut value = flow(&format!("history-{index}"), if index == 0 { 100_001 } else { 3 });
            if index % 2 != 0 { value.source = "replay".into(); }
            store.insert(&value).unwrap();
        }
        let snapshot = store.metadata_changes(None).unwrap();
        assert!(snapshot.reset); assert_eq!(snapshot.rows.len(), 251); assert!(snapshot.deleted.is_empty());
        assert_eq!(snapshot.rows[0].flow.metadata["id"], "history-250");
        assert!(snapshot.rows.windows(2).all(|rows| rows[0].position > rows[1].position));
        let source = flow("history-0", 100_001);
        let metadata = store.metadata("history-0", None).unwrap().unwrap();
        assert_eq!(metadata.metadata, without_bodies(serde_json::to_value(&source).unwrap()));
        assert_eq!(metadata.body_references.len(), 4);
        for (variant, length) in [("request",100_001),("response",100_002),("originalRequest",0),("originalResponse",4)] {
            assert_eq!(metadata.body_references[variant].byte_length, length);
        }
        let encoded = serde_json::to_string(&metadata).unwrap();
        assert!(!encoded.contains("bodyBase64")); assert!(!encoded.contains("@body:")); assert!(encoded.len() < 4096);
        assert!(encoded.contains("bodyReferences") && encoded.contains("bodyRevision"));
        let roundtrip: FlowMetadata = serde_json::from_str(&encoded).unwrap();
        assert_eq!(roundtrip.metadata, metadata.metadata);
        let unchanged = store.metadata_changes(Some(&snapshot.revision)).unwrap();
        assert!(!unchanged.reset); assert!(unchanged.rows.is_empty() && unchanged.deleted.is_empty());
        // Existing full public APIs retain exact bodies and complete history.
        assert_eq!(store.list().unwrap().len(),251);
        assert_eq!(serde_json::to_value(store.get_at_revision("history-0",&metadata.body_revision).unwrap()).unwrap(),serde_json::to_value(source).unwrap());
    }
    #[test]
    fn metadata_cache_survives_restart_and_legacy_rows_fall_back_without_rewrite() {
        let directory = tempfile::tempdir().unwrap(); let path = directory.path().join("views.db");
        let legacy = flow("legacy", 100_001);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE flows(id TEXT PRIMARY KEY, started_at INTEGER NOT NULL, data TEXT NOT NULL);").unwrap();
        connection.execute("INSERT INTO flows VALUES(?1,?2,?3)", params![legacy.id, legacy.started_at, serde_json::to_string(&legacy).unwrap()]).unwrap();
        drop(connection);
        let store = Store::open(&path).unwrap();
        let cached_count: i64 = store.0.lock().unwrap().query_row("SELECT count(*) FROM flow_projection WHERE flow_id='legacy'", [], |row| row.get(0)).unwrap();
        assert_eq!(cached_count, 0, "existing rows are read without an eager whole-database rewrite");
        let old = store.metadata("legacy", None).unwrap().unwrap();
        assert_eq!(old.metadata, without_bodies(serde_json::to_value(&legacy).unwrap()));
        assert_eq!(old.body_references["request"].byte_length, 100_001);

        let current = flow("cached", 100_001); store.insert(&current).unwrap();
        let (cached, refs, projection_version): (String, String, i64) = store.0.lock().unwrap().query_row("SELECT metadata,body_refs,version FROM flow_projection WHERE flow_id='cached'", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&cached).unwrap(), without_bodies(serde_json::to_value(&current).unwrap()));
        assert!(!cached.contains("bodyBase64")); assert!(!cached.contains("@body:"));
        let references: BTreeMap<String, BodyReference> = serde_json::from_str(&refs).unwrap();
        assert_eq!(references["request"].byte_length, 100_001);
        assert_eq!(references["response"].byte_length, 100_002);
        drop(store);
        let connection = Connection::open(&path).unwrap();
        connection.execute("UPDATE flows SET data=json_set(data,'$.notes',json_array('external update')) WHERE id='cached'", []).unwrap();
        drop(connection);
        let reopened = Store::open(&path).unwrap();
        let changed_version: i64 = reopened.0.lock().unwrap().query_row("SELECT version FROM flow_changes WHERE id='cached'", [], |row| row.get(0)).unwrap();
        assert!(changed_version > projection_version, "direct SQL edits invalidate the cached projection");
        assert_eq!(reopened.metadata("cached", None).unwrap().unwrap().metadata["notes"][0], "external update");
        reopened.insert(&flow("fresh",100_001)).unwrap();
        reopened.compact().unwrap();
        let (projection_version_after_compact,record_version_after_compact): (i64,i64) = reopened.0.lock().unwrap().query_row(
            "SELECT p.version,c.version FROM flow_projection p JOIN flow_changes c ON c.id=p.flow_id WHERE p.flow_id='cached'",[],
            |row|Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_ne!(projection_version_after_compact,record_version_after_compact,"compaction must not validate a stale projection");
        let (fresh_projection,fresh_record): (i64,i64) = reopened.0.lock().unwrap().query_row(
            "SELECT p.version,c.version FROM flow_projection p JOIN flow_changes c ON c.id=p.flow_id WHERE p.flow_id='fresh'",[],
            |row|Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(fresh_projection,fresh_record,"compaction keeps a current projection usable");
        assert_eq!(reopened.metadata("cached", None).unwrap().unwrap().metadata["notes"][0], "external update");
        assert_eq!(reopened.metadata("legacy", None).unwrap().unwrap().body_references["response"].byte_length, 100_002);
    }

    #[test]
    fn external_blob_change_invalidates_projection_and_pinned_body_read() {
        let directory=tempfile::tempdir().unwrap();let store=store(directory.path());
        store.insert(&flow("blob-change",100_001)).unwrap();
        let before=store.metadata("blob-change",None).unwrap().unwrap();
        store.0.lock().unwrap().execute("UPDATE flow_bodies SET body=zeroblob(length(body)) WHERE flow_id='blob-change' AND slot='response'",[]).unwrap();
        assert!(store.read_body_at_revision("blob-change","response",&before.body_revision,0,16).is_err());
        let after=store.metadata("blob-change",None).unwrap().unwrap();
        assert_ne!(after.body_revision,before.body_revision);
        assert_eq!(after.body_references["response"].byte_length,100_002);
        let chunk=store.read_body_at_revision("blob-change","response",&after.body_revision,0,16).unwrap();
        assert_eq!(STANDARD.decode(chunk.body_base64).unwrap(),[0u8;16]);
        let cached:i64=store.0.lock().unwrap().query_row("SELECT count(*) FROM flow_projection WHERE flow_id='blob-change'",[],|row|row.get(0)).unwrap();
        assert_eq!(cached,0);
    }

    #[test]
    fn all_four_slots_read_exact_inline_blob_and_empty_ranges() {
        let directory = tempfile::tempdir().unwrap(); let store = store(directory.path());
        for length in [0,1,2,3,65535,65536,65537,100_001] {
            let id = format!("body-{length}"); let source = flow(&id,length); store.insert(&source).unwrap();
            let value = serde_json::to_value(&source).unwrap();
            let metadata = store.metadata(&id,None).unwrap().unwrap();
            for variant in VARIANTS {
                let bytes = STANDARD.decode(body(&value,variant)).unwrap();
                assert_eq!(metadata.body_references[variant].byte_length,bytes.len() as u64);
                for offset in [0,1,2,3,65535,bytes.len() as u64,u64::MAX] {
                    let chunk = store.read_body_at_revision(&id,variant,&metadata.body_revision,offset,65536).unwrap();
                    let start = offset.min(bytes.len() as u64) as usize;
                    let end = (start+65536).min(bytes.len());
                    assert_eq!(STANDARD.decode(&chunk.body_base64).unwrap(),bytes[start..end]);
                    assert_eq!(chunk.offset,offset); assert_eq!(chunk.total_bytes,bytes.len() as u64);
                    assert_eq!(chunk.next_offset,(end<bytes.len()).then_some(end as u64));
                }
            }
        }
        let mut empty = flow("only-request",0); empty.response = None; empty.original_request = None; empty.original_response = None;
        store.insert(&empty).unwrap(); let metadata = store.metadata("only-request",None).unwrap().unwrap();
        assert_eq!(metadata.body_references.len(),1); assert_eq!(metadata.body_references["request"].byte_length,0);
        assert!(store.read_body_at_revision("only-request","response",&metadata.body_revision,0,1).is_err());
        for length in [0,65537,usize::MAX] { assert!(store.read_body_at_revision("only-request","request",&metadata.body_revision,0,length).is_err()); }
        for variant in ["", "../../body", "$.request.bodyBase64", "Request"] { assert!(store.read_body_at_revision("only-request",variant,&metadata.body_revision,0,1).is_err()); }
    }

    #[test]
    fn versions_are_per_flow_and_reject_update_delete_and_reopened_database() {
        let directory = tempfile::tempdir().unwrap(); let store = store(directory.path());
        let mut source = flow("changing",70_001); store.insert(&source).unwrap();
        let first = store.metadata("changing",None).unwrap().unwrap(); let snapshot = store.metadata_changes(None).unwrap();
        store.insert(&flow("unrelated",2)).unwrap();
        assert!(store.read_body_at_revision("changing","request",&first.body_revision,0,4).is_ok(),"unrelated rows must not invalidate an unchanged body");
        source.request.body_base64 = STANDARD.encode(b"new bytes").into(); store.insert(&source).unwrap();
        assert!(store.metadata("changing",Some(&first.body_revision)).is_err());
        assert!(store.get_at_revision("changing",&first.body_revision).is_err());
        assert!(store.read_body_at_revision("changing","request",&first.body_revision,0,4).is_err());
        let current = store.metadata("changing",None).unwrap().unwrap(); assert_ne!(current.body_revision,first.body_revision);
        let delta = store.metadata_changes(Some(&snapshot.revision)).unwrap(); assert!(!delta.reset); assert_eq!(delta.rows.len(),2);
        store.delete_flows(vec!["changing".into()]).unwrap();
        assert!(store.metadata("changing",None).unwrap().is_none());
        assert!(store.get_at_revision("changing",&current.body_revision).is_err());
        assert!(store.read_body_at_revision("changing","request",&current.body_revision,0,4).is_err());
        let deleted = store.metadata_changes(Some(&delta.revision)).unwrap(); assert_eq!(deleted.deleted,vec!["changing"]);
        let remaining = store.metadata("unrelated",None).unwrap().unwrap();
        drop(store); let reopened = self::store(directory.path());
        assert!(reopened.metadata_changes(Some(&deleted.revision)).unwrap().reset);
        assert!(reopened.get_at_revision("unrelated",&remaining.body_revision).is_err());
        let fresh = reopened.metadata("unrelated",None).unwrap().unwrap(); assert_ne!(fresh.body_revision,remaining.body_revision);
        assert!(reopened.get_at_revision("unrelated",&fresh.body_revision).is_ok());
    }

    #[test]
    fn legacy_ranges_do_not_restore_flow_or_decode_unselected_bytes() {
        let directory = tempfile::tempdir().unwrap(); let store = store(directory.path()); store.insert(&flow("legacy",1)).unwrap();
        // A valid final quantum and selected range remain inspectable without
        // decoding a damaged excluded prefix, just like the existing MCP API.
        store.0.lock().unwrap().execute("UPDATE flows SET data=json_set(data,'$.request.bodyBase64','####AQI=','$.request.engine',json_object('future','opaque')) WHERE id='legacy'",[]).unwrap();
        let metadata = store.metadata("legacy",None).unwrap().unwrap();
        assert_eq!(metadata.metadata["request"]["engine"],json!({"future":"opaque"}));
        assert_eq!(metadata.body_references["request"].byte_length,5);
        assert!(store.get_at_revision("legacy",&metadata.body_revision).is_err(),"full Flow deserialization is a separate explicit operation");
        let chunk = store.read_body_at_revision("legacy","request",&metadata.body_revision,3,2).unwrap();
        assert_eq!(STANDARD.decode(chunk.body_base64).unwrap(),[1,2]);
        assert!(store.read_body_at_revision("legacy","request",&metadata.body_revision,0,1).is_err());
        // Inline legacy data can be larger than today's BLOB threshold. It is
        // projected in SQL and sliced only after the revision has been checked.
        let bytes: Vec<u8> = (0..200_003).map(|index|(index%251) as u8).collect();
        store.0.lock().unwrap().execute("UPDATE flows SET data=json_set(data,'$.request.bodyBase64',?1) WHERE id='legacy'",[STANDARD.encode(&bytes)]).unwrap();
        let metadata = store.metadata("legacy",None).unwrap().unwrap();
        assert_eq!(metadata.body_references["request"].byte_length,bytes.len() as u64);
        let chunk = store.read_body_at_revision("legacy","request",&metadata.body_revision,65535,65536).unwrap();
        assert_eq!(STANDARD.decode(chunk.body_base64).unwrap(),bytes[65535..131071]);
        assert!(serde_json::to_string(&metadata).unwrap().len()<4096);
    }

    #[test]
    fn missing_or_wrong_blob_reference_is_never_reported_as_an_empty_body() {
        let directory = tempfile::tempdir().unwrap(); let store = store(directory.path()); store.insert(&flow("broken",70_001)).unwrap();
        let metadata = store.metadata("broken",None).unwrap().unwrap();
        store.0.lock().unwrap().execute("DELETE FROM flow_bodies WHERE flow_id='broken' AND slot='response'",[]).unwrap();
        assert!(store.metadata("broken",None).is_err());
        assert!(store.read_body_at_revision("broken","response",&metadata.body_revision,0,1).is_err());
        store.0.lock().unwrap().execute("UPDATE flows SET data=json_set(data,'$.request.bodyBase64','@body:response') WHERE id='broken'",[]).unwrap();
        let revision = row_revision(&store.0.lock().unwrap(),"broken",&store.1).unwrap();
        assert!(store.read_body_at_revision("broken","request",&revision,0,1).is_err());
    }

    #[test]
    fn concurrent_writer_cannot_mix_a_checked_revision_with_new_or_deleted_body_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let reader = store(directory.path());
        // Open the second connection before starting a snapshot: opening Store
        // also initializes schemas, which is intentionally not the race here.
        let writer = store(directory.path());
        let original = flow("snapshot",100_001);
        reader.insert(&original).unwrap();
        let revision = reader.metadata("snapshot",None).unwrap().unwrap().body_revision;
        let mut connection = reader.0.lock().unwrap();
        let snapshot = connection.transaction().unwrap();
        check_revision(&snapshot,"snapshot",&revision,&reader.1).unwrap();

        // WAL permits another connection to commit both a replacement and a
        // deletion after the revision check. Reads must retain the old snapshot.
        let mut replacement = original.clone();
        replacement.request.body_base64 = STANDARD.encode(vec![239;100_001]).into();
        writer.insert(&replacement).unwrap();
        writer.delete_flows(vec!["snapshot".into()]).unwrap();
        check_revision(&snapshot,"snapshot",&revision,&reader.1).unwrap();
        let BodyLocation::Blob {rowid,length} = body_location(&snapshot,"snapshot","request").unwrap() else {panic!("expected stored BLOB")};
        assert_eq!(length,100_001);
        let blob = snapshot.blob_open(rusqlite::DatabaseName::Main,"flow_bodies","body",rowid,true).unwrap();
        let mut bytes = [0u8;64];
        blob.read_at_exact(&mut bytes,65_535).unwrap();
        assert_eq!(bytes, std::array::from_fn::<_,64,_>(|index|((65_535+index)%251) as u8));
        drop(blob);
        snapshot.commit().unwrap();
        drop(connection);
        assert!(reader.metadata("snapshot",None).unwrap().is_none());
        assert!(reader.get_at_revision("snapshot",&revision).is_err());
        assert!(reader.read_body_at_revision("snapshot","request",&revision,0,64).is_err());
    }
}
