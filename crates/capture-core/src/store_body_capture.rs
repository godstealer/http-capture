//! Capture availability is checked before touching bytes, including legacy rows.
use crate::body_capture::{state_from_metadata, BodyCaptureState};
use crate::model::Flow;
use anyhow::{ensure, Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) fn recorded_slots(flow: &Flow) -> BTreeMap<&'static str, bool> {
    ["request", "response", "originalRequest", "originalResponse"].into_iter()
        .map(|slot| (slot, flow.ensure_body_recorded(slot).is_ok())).collect()
}

/// json_object emits null both for an absent path and an explicit JSON null.
/// Keep that distinction so old ordinary records remain captured, while malformed
/// explicit declarations fail closed. The discriminator never leaves this layer.
fn normalize_projection(metadata: &mut Value) {
    if let Some(object) = metadata.as_object_mut() {
        if object.remove("_captureBodyType") == Some(Value::Null) {
            object.remove("bodyCapture");
        }
    }
    crate::body_capture::annotate_metadata(metadata);
}

const CAPTURE_STATE_PROJECTION: &str = "json_object('bodyCapture',json_extract(data,'$.bodyCapture'),
    '_captureBodyType',json_type(data,'$.bodyCapture'),
    'trafficOrigin',json_object('entry',json_extract(data,'$.trafficOrigin.entry')))";

/// Metadata-only session JSON needs a new envelope: older readers ignore the new
/// bodyCapture field and would otherwise turn its empty placeholder into a body.
/// Both this scan and the later row export run inside the same read transaction.
pub(super) fn session_format_version(conn: &Connection) -> Result<u8> {
    let sql = format!("SELECT {CAPTURE_STATE_PROJECTION},json_type(data,'$.request'),
        json_type(data,'$.response'),json_type(data,'$.originalRequest'),json_type(data,'$.originalResponse') FROM flows");
    let mut statement = conn.prepare(&sql)?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let raw: String = row.get(0)?;
        let mut metadata: Value = serde_json::from_str(&raw)?;
        normalize_projection(&mut metadata);
        for (index, variant) in crate::body_capture::VARIANTS.into_iter().enumerate() {
            if row.get::<_, Option<String>>(index + 1)?.as_deref() == Some("object")
                && state_from_metadata(&metadata, variant) != BodyCaptureState::Recorded {
                return Ok(2);
            }
        }
    }
    Ok(1)
}

/// Project only provenance and capture states. No full payload enters this check.
pub(super) fn ensure_recorded(conn: &Connection, id: &str, variant: &str) -> Result<()> {
    super::body_views::body_slot(variant)?;
    let raw: String = conn.query_row(
        &format!("SELECT {CAPTURE_STATE_PROJECTION} FROM flows WHERE id=?1"),
        [id], |row| row.get(0),
    ).optional()?.context("Recorded flow not found")?;
    let mut metadata: Value = serde_json::from_str(&raw)?;
    normalize_projection(&mut metadata);
    ensure!(state_from_metadata(&metadata, variant) == BodyCaptureState::Recorded,
        "Requested body was not recorded; it cannot be read, downloaded or copied as an empty body");
    Ok(())
}

/// MCP keeps its existing private projection; the temporary origin is needed
/// only to conservatively classify legacy android-lan metadata records.
pub(super) fn annotate_mcp_metadata(metadata: &mut Value) {
    normalize_projection(metadata);
    // Keep the public MCP contract well-formed even when a legacy database has
    // a malformed declaration. That declaration still denies body access.
    let states = metadata.get("bodyCapture").map(|_| crate::body_capture::VARIANTS.into_iter()
        .map(|slot| (slot.to_owned(), serde_json::to_value(state_from_metadata(metadata, slot)).expect("body state serialization")))
        .collect::<serde_json::Map<_, _>>());
    if let Some(object) = metadata.as_object_mut() {
        object.remove("trafficOrigin");
        if let Some(states) = states { object.insert("bodyCapture".into(), Value::Object(states)); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Store;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use rusqlite::params;
    use serde_json::json;
    const SLOTS: [&str; 4] = ["request", "response", "originalRequest", "originalResponse"];

    fn flow(id: &str, remote: bool, bytes: &[u8]) -> Flow {
        let request = json!({"method":"POST","url":format!("http://example.invalid/{id}"),
            "headers":[],"bodyBase64":STANDARD.encode(bytes)});
        let response = json!({"status":200,"version":"HTTP/1.1","headers":[],"bodyBase64":STANDARD.encode(bytes)});
        let mut value = json!({"id":id,"startedAt":1,"durationMs":0,"state":"completed","source":"capture",
            "request":request,"response":response,"originalRequest":request,"originalResponse":response,
            "error":null,"notes":[]});
        if remote { value["trafficOrigin"] = origin(); }
        serde_json::from_value(value).unwrap()
    }
    fn origin() -> Value { json!({"entry":"android-lan","clientAddress":"192.168.1.2:1234",
        "process":null,"processStatus":"remote"}) }
    fn store(directory: &std::path::Path) -> Store { Store::open(&directory.join("capture.db")).unwrap() }
    fn current_revision(store: &Store, id: &str) -> String { store.metadata(id,None).unwrap().unwrap().body_revision }
    /// Reproduce an older release's valid projection with cached fake zero-byte
    /// refs, without letting the current writer normalize the old tunnel row.
    fn legacy_lan(store: &Store, id: &str, cached: bool) {
        let conn = store.0.lock().unwrap();
        conn.execute("UPDATE flows SET data=json_set(data,'$.trafficOrigin',json(?2)) WHERE id=?1",
            params![id,origin().to_string()]).unwrap();
        if cached {
            conn.execute("UPDATE flow_projection SET version=(SELECT version FROM flow_changes WHERE id=?1),
                metadata=json_remove((SELECT data FROM flows WHERE id=?1),'$.request.bodyBase64','$.response.bodyBase64',
                  '$.originalRequest.bodyBase64','$.originalResponse.bodyBase64') WHERE flow_id=?1",[id]).unwrap();
        } else { conn.execute("DELETE FROM flow_projection WHERE flow_id=?1",[id]).unwrap(); }
    }

    #[test]
    fn old_android_lan_zero_cache_cannot_be_read_downloaded_or_composed() {
        let directory=tempfile::tempdir().unwrap();let store=store(directory.path());
        store.insert(&flow("legacy",false,&[])).unwrap();legacy_lan(&store,"legacy",true);
        let metadata=store.metadata("legacy",None).unwrap().unwrap();
        assert!(metadata.body_references.is_empty());
        for slot in SLOTS {
            assert_eq!(metadata.metadata["bodyCapture"][slot],"notRecorded");
            let error=store.read_body_at_revision("legacy",slot,&metadata.body_revision,0,64).err().unwrap();
            assert!(error.to_string().contains("not recorded"));
            let mut output=Vec::new();
            assert!(store.copy_body_at_revision("legacy",slot,&metadata.body_revision,&mut output).is_err());
            assert!(output.is_empty());
            let destination=directory.path().join(format!("{slot}.bin"));
            assert!(store.save_body_at_revision("legacy",slot,&metadata.body_revision,&destination).is_err());
            assert!(!destination.exists());
        }
        assert!(!std::fs::read_dir(directory.path()).unwrap().flatten().any(|entry|
            entry.file_name().to_string_lossy().starts_with(".http-capture-body-")));
        for direction in ["request","response"] { assert!(store.mcp_read_body("legacy",direction,0,64).is_err()); }
        assert!(store.mcp_compose_request("legacy").is_err());
        let changes=store.metadata_changes(None).unwrap();assert!(changes.rows[0].flow.body_references.is_empty());
        let restored=store.get_at_revision("legacy",&metadata.body_revision).unwrap();
        assert!(restored.ensure_body_recorded("request").is_err());
        assert_eq!(serde_json::to_value(restored).unwrap()["bodyCapture"]["request"],"notRecorded");
    }

    #[test]
    fn uncached_legacy_tunnel_does_not_restore_fake_or_missing_body_bytes() {
        let directory=tempfile::tempdir().unwrap();let store=store(directory.path());
        store.insert(&flow("legacy",false,&[])).unwrap();legacy_lan(&store,"legacy",false);
        store.0.lock().unwrap().execute("UPDATE flows SET data=json_set(data,'$.request.bodyBase64','@body:request',
            '$.response.bodyBase64','invalid base64') WHERE id='legacy'",[]).unwrap();
        let metadata=store.metadata("legacy",None).unwrap().unwrap();assert!(metadata.body_references.is_empty());
        let restored=store.get("legacy").unwrap().unwrap();
        assert!(restored.request.body_base64.is_empty());assert!(restored.response.unwrap().body_base64.is_empty());
        assert!(store.mcp_read_body("legacy","request",0,64).err().unwrap().to_string().contains("not recorded"));
        let mut session=Vec::new();assert_eq!(store.export_session_json(&mut session).unwrap(),1);
        let value:Value=serde_json::from_slice(&session).unwrap();
        assert_eq!(value["flows"][0]["bodyCapture"]["request"],"notRecorded");
        assert_eq!(value["flows"][0]["request"]["bodyBase64"],"");
    }

    #[test]
    fn ordinary_empty_and_explicit_recorded_remote_empty_still_read_as_zero_bytes() {
        let directory=tempfile::tempdir().unwrap();let store=store(directory.path());
        store.insert(&flow("local",false,&[])).unwrap();
        let mut remote=flow("remote",true,&[]);remote.state="receiving".into();
        remote.body_capture.insert("request".into(),BodyCaptureState::Recorded);
        remote.body_capture.insert("originalRequest".into(),BodyCaptureState::Recorded);
        store.insert(&remote).unwrap();
        for (id,slots) in [("local",SLOTS.as_slice()),("remote",&["request","originalRequest"][..])] {
            let metadata=store.metadata(id,None).unwrap().unwrap();
            for slot in slots {
                assert_eq!(metadata.body_references[*slot].byte_length,0);
                let body=store.read_body_at_revision(id,slot,&metadata.body_revision,0,64).unwrap();
                assert_eq!(body.total_bytes,0);assert_eq!(body.body_base64,"");
            }
            assert_eq!(store.mcp_read_body(id,"request",0,64).unwrap().total_bytes,0);
            assert_eq!(store.mcp_compose_request(id).unwrap()["bodyBase64"],"");
        }
        let metadata=store.metadata("remote",None).unwrap().unwrap();
        assert_eq!(metadata.body_references.len(),2);
        for slot in ["response","originalResponse"] {
            assert!(store.read_body_at_revision("remote",slot,&metadata.body_revision,0,64).is_err());
        }
    }

    #[test]
    fn unrecording_a_slot_clears_its_blob_without_changing_other_recorded_entities() {
        let directory=tempfile::tempdir().unwrap();let store=store(directory.path());
        let bytes=vec![0xa5;100_000];let mut captured=flow("changing",false,&bytes);
        store.insert(&captured).unwrap();
        for (index,slot) in SLOTS.into_iter().enumerate() {
            captured.body_capture.insert(slot.into(),BodyCaptureState::NotRecorded);store.insert(&captured).unwrap();
            let metadata=store.metadata("changing",None).unwrap().unwrap();
            assert_eq!(metadata.body_references.len(),3-index);
            assert!(metadata.body_references.get(slot).is_none());
            let blobs:i64=store.0.lock().unwrap().query_row("SELECT count(*) FROM flow_bodies WHERE flow_id='changing'",[],|row|row.get(0)).unwrap();
            assert_eq!(blobs,3-index as i64);
            let restored=serde_json::to_value(store.get("changing").unwrap().unwrap()).unwrap();
            for (other_index,other) in SLOTS.into_iter().enumerate() {
                if other_index<=index { assert_eq!(restored[other]["bodyBase64"],""); }
                else { assert_eq!(restored[other]["bodyBase64"],STANDARD.encode(&bytes)); }
            }
        }
        captured.body_capture.insert("request".into(),BodyCaptureState::Recorded);store.insert(&captured).unwrap();
        assert_eq!(store.mcp_read_body("changing","request",3,64).unwrap().total_bytes,100_000);
        assert_eq!(store.get("changing").unwrap().unwrap().request.body_base64.as_str(),STANDARD.encode(bytes));
    }

    #[test]
    fn session_json_and_import_keep_unrecorded_distinct_from_real_empty_body() {
        let directory=tempfile::tempdir().unwrap();let source=store(directory.path());
        source.insert(&flow("remote",true,&[])).unwrap();source.insert(&flow("local",false,&[])).unwrap();
        let mut bytes=Vec::new();assert_eq!(source.export_session_json(&mut bytes).unwrap(),2);
        let document:Value=serde_json::from_slice(&bytes).unwrap();
        assert_eq!(document["format"],"http-capture-session");assert_eq!(document["version"],2);
        let flows:Vec<Flow>=serde_json::from_value(document["flows"].clone()).unwrap();
        let other=tempfile::tempdir().unwrap();let target=store(other.path());let import=target.begin_import().unwrap();
        assert_eq!(target.append_import(&import,0,flows).unwrap(),2);assert_eq!(target.finish_import(&import,true).unwrap(),2);
        let metadata=target.metadata_changes(None).unwrap();assert_eq!(metadata.rows.len(),2);
        for row in metadata.rows {
            assert_eq!(row.flow.metadata["state"],"imported");
            if row.flow.metadata["request"]["url"].as_str().unwrap().ends_with("/remote") {
                assert!(row.flow.body_references.is_empty());
                assert_eq!(row.flow.metadata["bodyCapture"]["request"],"notRecorded");
                assert!(target.mcp_compose_request(&row.flow.metadata["id"].as_str().unwrap()).is_err());
            } else {assert_eq!(row.flow.body_references["request"].byte_length,0);}
        }
    }

    #[test]
    fn stale_revision_is_rejected_before_current_unrecorded_state() {
        let directory=tempfile::tempdir().unwrap();let store=store(directory.path());let mut captured=flow("versioned",false,&[]);
        store.insert(&captured).unwrap();let old=current_revision(&store,"versioned");
        captured.body_capture.insert("request".into(),BodyCaptureState::NotRecorded);store.insert(&captured).unwrap();
        let error=store.read_body_at_revision("versioned","request",&old,0,64).err().unwrap();
        assert!(error.to_string().contains("changed"));
        let current=current_revision(&store,"versioned");assert_ne!(current,old);
        assert!(store.read_body_at_revision("versioned","request",&current,0,64).err().unwrap().to_string().contains("not recorded"));
        let path=directory.path().join("stale.bin");assert!(store.save_body_at_revision("versioned","request",&old,&path).is_err());assert!(!path.exists());
    }

    #[test]
    fn mcp_metadata_exposes_capture_state_without_private_or_internal_projection_fields() {
        let directory=tempfile::tempdir().unwrap();let store=store(directory.path());
        store.insert(&flow("legacy",false,&[])).unwrap();legacy_lan(&store,"legacy",true);
        store.insert(&flow("ordinary",false,&[])).unwrap();
        let detail=store.mcp_get_request("legacy").unwrap().unwrap();
        for slot in SLOTS {assert_eq!(detail["bodyCapture"][slot],"notRecorded");}
        assert!(store.mcp_get_request("ordinary").unwrap().unwrap().get("bodyCapture").is_none());
        let page=store.mcp_list_requests(Default::default()).unwrap();
        let legacy=page.requests.iter().find(|value|value["id"]=="legacy").unwrap();
        assert_eq!(legacy["bodyCapture"]["request"],"notRecorded");
        for value in page.requests.iter().chain(std::iter::once(&detail)) {
            assert!(value.get("trafficOrigin").is_none());assert!(value.get("_captureBodyType").is_none());
            assert!(!value.to_string().contains("bodyBase64"));
        }
        store.0.lock().unwrap().execute("UPDATE flows SET data=json_set(data,'$.bodyCapture',json('null')) WHERE id='ordinary'",[]).unwrap();
        assert!(store.mcp_read_body("ordinary","request",0,64).is_err());
        let malformed=store.mcp_get_request("ordinary").unwrap().unwrap();
        for slot in SLOTS {assert_eq!(malformed["bodyCapture"][slot],"notRecorded");}
    }

    #[test]
    fn complete_sessions_keep_version_one_and_only_existing_entities_select_v2() {
        let directory=tempfile::tempdir().unwrap();let store=store(directory.path());
        let read_version=|| {let mut out=Vec::new();store.export_session_json(&mut out).unwrap();serde_json::from_slice::<Value>(&out).unwrap()["version"].clone()};
        assert_eq!(read_version(),1);
        let mut complete=flow("local",false,b"complete bytes");
        complete.body_capture.insert("request".into(),BodyCaptureState::Recorded);
        store.insert(&complete).unwrap();assert_eq!(read_version(),1);
        // A declaration for an entity that does not exist cannot invalidate the
        // actual request-only session's compatibility with older readers.
        complete.response=None;complete.original_request=None;complete.original_response=None;
        complete.body_capture.insert("response".into(),BodyCaptureState::NotRecorded);
        store.insert(&complete).unwrap();assert_eq!(read_version(),1);
        complete.body_capture.insert("request".into(),BodyCaptureState::NotRecorded);
        store.insert(&complete).unwrap();assert_eq!(read_version(),2);

        // An old tunnel with no bodyCapture map also requires the v2 envelope.
        let other=tempfile::tempdir().unwrap();let old_store=Store::open(&other.path().join("capture.db")).unwrap();
        old_store.insert(&flow("old",false,&[])).unwrap();legacy_lan(&old_store,"old",true);
        let mut out=Vec::new();old_store.export_session_json(&mut out).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&out).unwrap()["version"],2);
    }

    #[test]
    fn session_version_prescan_and_rows_share_the_same_snapshot() {
        struct InsertOnHeader<'a> { store: &'a Store, insert: Option<Flow>, bytes: Vec<u8> }
        impl std::io::Write for InsertOnHeader<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if let Some(flow) = self.insert.take() { self.store.insert(&flow).map_err(|error| std::io::Error::other(error.to_string()))?; }
                self.bytes.extend_from_slice(bytes);Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let directory=tempfile::tempdir().unwrap();let store=store(directory.path());
        store.insert(&flow("snapshot",false,b"complete")).unwrap();
        let mut writer=InsertOnHeader{store:&store,insert:Some(flow("later-metadata",true,&[])),bytes:Vec::new()};
        assert_eq!(store.export_session_json(&mut writer).unwrap(),1);
        let exported:Value=serde_json::from_slice(&writer.bytes).unwrap();
        assert_eq!(exported["version"],1);assert_eq!(exported["flows"].as_array().unwrap().len(),1);
        assert_eq!(exported["flows"][0]["id"],"snapshot");assert_eq!(store.list().unwrap().len(),2);
        let mut after=Vec::new();store.export_session_json(&mut after).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&after).unwrap()["version"],2);
    }

}
