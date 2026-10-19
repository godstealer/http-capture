//! Atomic MCP request-library/workspace changes. JSON preserves fields owned by
//! the UI (including scripts and future extensions) that MCP does not edit.
use super::Store;
use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use base64::{engine::general_purpose::STANDARD,Engine as _};

const MAX_SETTING_BYTES: usize = 32 * 1024 * 1024;

fn empty_workspace() -> Value { json!({"revision":0,"editors":[],"activeEditor":""}) }
fn empty_library() -> Value { json!({"revision":0,"collections":[],"requests":[],"environments":[],"scripts":[]}) }
fn revision(value: &Value) -> Result<u64> { value.get("revision").and_then(Value::as_u64).context("Invalid saved workspace/library revision") }
fn read(conn: &rusqlite::Connection, key: &str, default: fn() -> Value) -> Result<Value> {
    let length: Option<usize> = conn.query_row("SELECT length(CAST(value AS BLOB)) FROM settings WHERE key=?1",[key],|row|row.get(0)).optional()?;
    ensure!(length.is_none_or(|n|n<=MAX_SETTING_BYTES),"Saved workspace/library exceeds the MCP editing limit; use the application to reduce it");
    let raw: Option<String> = conn.query_row("SELECT value FROM settings WHERE key=?1",[key],|row|row.get(0)).optional()?;
    let mut value:Value = raw.map(|text|serde_json::from_str(&text)).transpose()?.unwrap_or_else(default);
    // Old workspaces predate the compare-and-swap revision field.
    if value.is_object() && value.get("revision").is_none() {value["revision"]=json!(0);}
    revision(&value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transaction_rolls_back_first_write_when_second_setting_validation_fails() {
        let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("workflows.db")).unwrap();
        let before=store.mcp_workspace_snapshot().unwrap();
        let result=store.mcp_change_workspace(Some(0),Some(0),|workspace,library|{
            workspace["futureField"]=json!("must roll back");
            library["collections"]=json!([{"id":"duplicate","name":"One"},{"id":"duplicate","name":"Two"}]);Ok(())
        });
        assert!(result.is_err());assert_eq!(store.mcp_workspace_snapshot().unwrap(),before);assert_eq!(store.workspace_revisions().unwrap(),json!({"workspaceRevision":0,"libraryRevision":0}));
    }
    #[test]
    fn conflicts_are_checked_before_mutation_and_other_settings_remain_intact() {
        let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("workflows.db")).unwrap();
        store.save_setting("unrelated.secret",&json!({"token":"private"})).unwrap();
        store.mcp_change_workspace(Some(0),None,|workspace,_|{workspace["futureField"]=json!("preserved");Ok(())}).unwrap();
        let failed=store.mcp_change_workspace(Some(0),Some(0),|_,_|->Result<()>{panic!("stale CAS callback must not execute")});
        assert!(failed.is_err());assert_eq!(store.workspace_revisions().unwrap(),json!({"workspaceRevision":1,"libraryRevision":0}));assert_eq!(store.setting::<Value>("unrelated.secret").unwrap().unwrap()["token"],"private");
    }
    #[test]
    fn captured_composition_reads_inline_or_blob_exactly_and_rejects_oversize_without_modifying_source() {
        let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("workflows.db")).unwrap();
        for length in [0,4,65535,65536,65537,100000] {
            let bytes=(0..length).map(|i|(i%251) as u8).collect::<Vec<_>>();let id=format!("f{length}");
            let flow:crate::model::Flow=serde_json::from_value(json!({"id":id,"parentId":null,"startedAt":1,"durationMs":1,"source":"capture","state":"completed","request":{"method":"POST","url":"https://example.invalid/","headers":[{"name":"X-A","value":"one"},{"name":"X-A","value":"two","disabled":true}],"bodyBase64":STANDARD.encode(&bytes)},"response":null,"error":null,"notes":[]})).unwrap();
            store.insert(&flow).unwrap();let draft=store.mcp_compose_request(&id);
            if length<=65536 {let draft=draft.unwrap();assert_eq!(STANDARD.decode(draft["bodyBase64"].as_str().unwrap()).unwrap(),bytes);assert_eq!(draft["headers"][1]["value"],"two");assert_eq!(draft["headers"][1]["disabled"],true);} else {assert!(draft.is_err());}
            assert_eq!(store.get(&id).unwrap().unwrap().request.body_base64,flow.request.body_base64);
        }
    }
}
fn validate_workspace(value: &Value) -> Result<()> {
    let editors=value["editors"].as_array().context("Invalid saved request workspace")?;
    ensure!(editors.len()<=100,"At most 100 request tabs can be saved");
    let mut ids=std::collections::HashSet::new();
    for editor in editors {
        let id=editor["id"].as_str().context("Invalid request editor ID")?;
        ensure!(!id.is_empty()&&id.len()<=128&&ids.insert(id),"Invalid or duplicate request editor ID");
        ensure!(editor["draft"].is_object(),"Invalid request draft");
    }
    let active=value["activeEditor"].as_str().context("Invalid active request editor")?;
    ensure!(active.is_empty()||ids.contains(active),"Active request editor does not exist");
    Ok(())
}
fn validate_library(value: &Value) -> Result<()> {
    let mut ids=std::collections::HashSet::new();
    for key in ["collections","requests","environments","scripts"] {
        let items=value[key].as_array().context("Invalid saved request library")?;
        ensure!(items.len()<=10000,"Saved library exceeds the MCP item limit");
        for item in items {
            let id=item["id"].as_str().context("Invalid library ID")?;
            let name=item["name"].as_str().context("Invalid library name")?;
            ensure!(!id.is_empty()&&id.len()<=128&&ids.insert(id)&&!name.trim().is_empty()&&name.len()<=512,"Invalid or duplicate library ID/name");
        }
    }
    for request in value["requests"].as_array().unwrap() {
        ensure!(value["collections"].as_array().unwrap().iter().any(|c|c["id"]==request["collectionId"]),"Collection not found");
    }
    Ok(())
}
impl Store {
    /// Lightweight UI invalidation, without deserializing any drafts or bodies.
    pub fn workspace_revisions(&self) -> Result<Value> {
        let conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let mut revisions=json!({"workspaceRevision":0,"libraryRevision":0});
        let mut statement=conn.prepare("SELECT key,coalesce(json_extract(value,'$.revision'),0) FROM settings WHERE key IN ('workspace.v1','library.v1')")?;
        let rows=statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,u64>(1)?)))?;
        for row in rows {let (key,value)=row?;revisions[if key=="workspace.v1" {"workspaceRevision"} else {"libraryRevision"}]=json!(value);}
        Ok(revisions)
    }
    pub fn mcp_workspace_snapshot(&self) -> Result<(Value,Value)> {
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx=conn.transaction()?;
        let values=(read(&tx,"workspace.v1",empty_workspace)?,read(&tx,"library.v1",empty_library)?);
        tx.commit()?;
        Ok(values)
    }
    /// Both expected revisions are checked before the callback. Only explicitly
    /// selected settings are written, in one transaction; conflicts never retry.
    pub fn mcp_change_workspace<R>(&self, expected_workspace: Option<u64>, expected_library: Option<u64>, change: impl FnOnce(&mut Value,&mut Value)->Result<R>) -> Result<(R,u64,u64)> {
        ensure!(expected_workspace.is_some()||expected_library.is_some(),"An expected revision is required");
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.configuration_ready_in(&tx)?;
        let mut workspace=read(&tx,"workspace.v1",empty_workspace)?;
        let mut library=read(&tx,"library.v1",empty_library)?;
        let workspace_revision=revision(&workspace)?;let library_revision=revision(&library)?;
        ensure!(expected_workspace.is_none_or(|r|r==workspace_revision),"Workspace revision conflict: current expectedRevision is {workspace_revision}; read the workspace again before editing");
        ensure!(expected_library.is_none_or(|r|r==library_revision),"Library revision conflict: current expectedLibraryRevision is {library_revision}; read collections again before editing");
        let result=change(&mut workspace,&mut library)?;
        let next_workspace=if expected_workspace.is_some() {workspace_revision.checked_add(1).context("Workspace revision overflow")?} else {workspace_revision};
        let next_library=if expected_library.is_some() {library_revision.checked_add(1).context("Library revision overflow")?} else {library_revision};
        for (key,value,expected,next) in [("workspace.v1",&mut workspace,expected_workspace,next_workspace),("library.v1",&mut library,expected_library,next_library)] {
            if expected.is_none() {continue;}
            if key=="workspace.v1" {validate_workspace(value)?;} else {validate_library(value)?;}
            value["revision"]=json!(next);
            let raw=serde_json::to_string(value)?;
            ensure!(raw.len()<=MAX_SETTING_BYTES,"Saved workspace/library exceeds the MCP editing limit");
            tx.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,raw])?;
        }
        tx.commit()?;
        Ok((result,next_workspace,next_library))
    }
    /// Exact ordinary HTTP fields only. Never loads TLS/proxy/script state or a
    /// huge body while composing a captured request into an editor.
    pub fn mcp_compose_request(&self,id:&str)->Result<Value> {
        ensure!(!id.is_empty()&&id.len()<=128,"Invalid captured request ID");
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx=conn.transaction()?;
        let lengths: Option<(usize,usize,usize)> = tx.query_row("SELECT length(CAST(json_extract(data,'$.request.url') AS BLOB)),length(CAST(json_extract(data,'$.request.headers') AS BLOB)),length(CAST(json_extract(data,'$.request.method') AS BLOB)) FROM flows WHERE id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        let (url,headers,method)=lengths.context("Captured request not found")?;
        ensure!(url<=8192&&headers<=128*1024&&method<=32,"Captured request exceeds MCP editor limits");
        let raw:String=tx.query_row("SELECT json_object('method',json_extract(data,'$.request.method'),'url',json_extract(data,'$.request.url'),'headers',json_extract(data,'$.request.headers')) FROM flows WHERE id=?1",[id],|row|row.get(0))?;
        let mut draft:Value=serde_json::from_str(&raw)?;
        let length:usize=tx.query_row("SELECT length(json_extract(data,'$.request.bodyBase64')) FROM flows WHERE id=?1",[id],|row|row.get(0))?;
        ensure!(length<=65536usize.div_ceil(3)*4,"Captured request body exceeds 65536 bytes; compose it in the application");
        let encoded:String=tx.query_row("SELECT json_extract(data,'$.request.bodyBase64') FROM flows WHERE id=?1",[id],|row|row.get(0))?;
        let encoded=if encoded.starts_with("@body:") {
            ensure!(encoded=="@body:request","Invalid stored body reference");
            let rowid:i64=tx.query_row("SELECT rowid FROM flow_bodies WHERE flow_id=?1 AND slot='request'",[id],|row|row.get(0))?;
            let blob=tx.blob_open(rusqlite::DatabaseName::Main,"flow_bodies","body",rowid,true)?;
            ensure!(blob.len()<=65536,"Captured request body exceeds 65536 bytes; compose it in the application");
            let mut bytes=vec![0;blob.len()];if !bytes.is_empty(){blob.read_at_exact(&mut bytes,0)?;}STANDARD.encode(bytes)
        } else {ensure!(STANDARD.decode(&encoded)?.len()<=65536,"Captured request body exceeds 65536 bytes; compose it in the application");encoded};
        draft["bodyBase64"]=json!(encoded);
        tx.commit()?;
        Ok(draft)
    }
}
