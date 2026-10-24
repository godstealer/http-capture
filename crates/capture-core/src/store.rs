use crate::model::{Flow, RequestDraft, SharedBody};
use crate::body_read::Base64ReadPlan;
use serde::{Serialize, Deserialize};
use rusqlite::OptionalExtension;
use anyhow::Result;
use rusqlite::{params, Connection};
use std::{path::Path, sync::Mutex};
use std::io::Read;
use std::collections::BTreeMap;

#[derive(Serialize,Deserialize)]
pub struct FlowChange {pub position:i64,pub flow:Flow}
#[derive(Serialize,Deserialize)]
pub struct FlowChanges {pub revision:String,pub reset:bool,pub rows:Vec<FlowChange>,pub deleted:Vec<String>}
pub struct Store(Mutex<Connection>, String, std::sync::atomic::AtomicBool,Option<String>);
#[path = "store_mcp.rs"]
mod mcp;
pub use mcp::{McpBodyChunk, McpRequestFilter, McpRequestPage};
#[path = "store_mcp_workflows.rs"]
mod mcp_workflows;
#[path = "store_config_migration.rs"]
mod config_migration;
#[path = "store_audit.rs"]
mod audit;
#[path = "store_fault_rules.rs"]
mod fault_rules;
#[path = "store_body_views.rs"]
mod body_views;
pub use body_views::{BodyReference, FlowMetadata, FlowMetadataChange, FlowMetadataChanges};
#[path = "store_body_export.rs"]
mod body_export;
#[path = "store_response_rewrite.rs"]
mod response_rewrite;

impl Store {
    pub fn backup(&self) -> Result<String> {
        let conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let database = Path::new(conn.path().ok_or_else(|| anyhow::anyhow!("Database has no file path"))?);
        let directory = database.parent().unwrap_or(Path::new(".")).join("backups");
        std::fs::create_dir_all(&directory)?;
        let destination = directory.join(format!("sessions-{}.sqlite", uuid::Uuid::new_v4()));
        conn.execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])?;
        Ok(std::fs::canonicalize(destination)?.to_string_lossy().into_owned())
    }
    pub fn compact(&self) -> Result<()> {
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        conn.execute_batch("VACUUM")?;
        let tx=conn.transaction()?;
        tx.execute_batch("CREATE TEMP TABLE current_projection_compaction(flow_id TEXT PRIMARY KEY) WITHOUT ROWID;
            INSERT INTO current_projection_compaction SELECT p.flow_id FROM flow_projection p JOIN flow_changes c ON c.id=p.flow_id WHERE p.version=c.version;
            UPDATE flows SET started_at=started_at;
            UPDATE flow_projection SET version=(SELECT c.version FROM flow_changes c WHERE c.id=flow_id)
              WHERE flow_id IN (SELECT flow_id FROM current_projection_compaction);
            DROP TABLE current_projection_compaction;")?;
        tx.commit()?;
        Ok(())
    }
    pub fn clear(&self) -> Result<usize> {
        let mut conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let tx = conn.transaction()?;
        tx.execute("INSERT OR IGNORE INTO deleted_flows SELECT id FROM flows", [])?;
        let count = tx.execute("DELETE FROM flows", [])?;
        tx.commit()?;
        Ok(count)
    }
    pub fn begin_import(&self) -> Result<String> {
        let conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS import_jobs(id TEXT PRIMARY KEY, created INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS import_rows(job TEXT NOT NULL, sequence INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(job,sequence));
            DELETE FROM import_rows WHERE job IN (SELECT id FROM import_jobs WHERE created < unixepoch()-86400);
            DELETE FROM import_jobs WHERE created < unixepoch()-86400;")?;
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute("INSERT INTO import_jobs VALUES(?1,unixepoch())", [&id])?;
        Ok(id)
    }
    pub fn append_import(&self, id: &str, offset: usize, flows: Vec<Flow>) -> Result<usize> {
        let mut conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let tx = conn.transaction()?;
        anyhow::ensure!(tx.query_row("SELECT count(*) FROM import_jobs WHERE id=?1", [id], |r| r.get::<_,i64>(0))? == 1, "Import session expired");
        let count: usize = tx.query_row("SELECT count(*) FROM import_rows WHERE job=?1", [id], |r| r.get(0))?;
        anyhow::ensure!(count == offset, "Import offset mismatch; abort and retry");
        for (index, mut flow) in flows.into_iter().enumerate() {
            flow.state = "imported".into();
            flow.id = uuid::Uuid::new_v4().to_string(); flow.parent_id = None; flow.source = "capture".into();
            flow.request.scripts = Default::default(); flow.request.upstream_profile_id = None;
            if let Some(r) = &mut flow.original_request { r.scripts = Default::default(); r.upstream_profile_id = None; }
            flow.notes.push("从文件导入；脚本和上游代理关联已清除。".into());
            tx.execute("INSERT INTO import_rows VALUES(?1,?2,?3)", params![id,offset+index,serde_json::to_string(&flow)?])?;
        }
        let total = tx.query_row("SELECT count(*) FROM import_rows WHERE job=?1", [id], |r| r.get(0))?;
        tx.commit()?; Ok(total)
    }
    pub fn finish_import(&self, id: &str, commit: bool) -> Result<usize> {
        let mut conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let tx = conn.transaction()?;
        anyhow::ensure!(tx.query_row("SELECT count(*) FROM import_jobs WHERE id=?1", [id], |r| r.get::<_,i64>(0))? == 1, "Import session expired");
        let count = if commit {
            let mut stmt=tx.prepare("SELECT data FROM import_rows WHERE job=?1 ORDER BY sequence DESC")?;
            let mut cursor=stmt.query([id])?;let mut count=0;
            while let Some(row)=cursor.next()? { let flow:Flow=serde_json::from_str(&row.get::<_,String>(0)?)?;write_flow(&tx,&flow)?;count+=1; }
            count
        } else { 0 };
        tx.execute("DELETE FROM import_rows WHERE job=?1", [id])?;
        tx.execute("DELETE FROM import_jobs WHERE id=?1", [id])?;
        tx.commit()?; Ok(count)
    }
    pub fn get(&self, id: &str) -> Result<Option<Flow>> {
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx=conn.transaction()?;
        let raw:Option<String>=tx.query_row("SELECT data FROM flows WHERE id=?1",[id],|r|r.get(0)).optional()?;
        let flow=raw.map(|raw|restore_bodies(&tx,&raw)).transpose()?;
        tx.commit()?;Ok(flow)
    }
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS flows (
            id TEXT PRIMARY KEY, started_at INTEGER NOT NULL, data TEXT NOT NULL
        ); CREATE TABLE IF NOT EXISTS deleted_flows (id TEXT PRIMARY KEY); CREATE INDEX IF NOT EXISTS flows_started ON flows(started_at);
            CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")?;
        conn.execute_batch("BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS flow_revision (id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL);
            INSERT OR IGNORE INTO flow_revision VALUES(1,0);
            CREATE TABLE IF NOT EXISTS flow_changes(id TEXT PRIMARY KEY, version INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS flow_changes_version ON flow_changes(version);
            DROP TRIGGER IF EXISTS flows_revision_insert; DROP TRIGGER IF EXISTS flows_revision_update; DROP TRIGGER IF EXISTS flows_revision_delete;
            CREATE TRIGGER flows_revision_insert AFTER INSERT ON flows BEGIN UPDATE flow_revision SET version=version+1 WHERE id=1; INSERT INTO flow_changes VALUES(new.id,(SELECT version FROM flow_revision WHERE id=1)) ON CONFLICT(id) DO UPDATE SET version=excluded.version; END;
            CREATE TRIGGER flows_revision_update AFTER UPDATE ON flows BEGIN UPDATE flow_revision SET version=version+1 WHERE id=1; INSERT INTO flow_changes VALUES(new.id,(SELECT version FROM flow_revision WHERE id=1)) ON CONFLICT(id) DO UPDATE SET version=excluded.version; END;
            CREATE TRIGGER flows_revision_delete AFTER DELETE ON flows BEGIN UPDATE flow_revision SET version=version+1 WHERE id=1; INSERT INTO flow_changes VALUES(old.id,(SELECT version FROM flow_revision WHERE id=1)) ON CONFLICT(id) DO UPDATE SET version=excluded.version; END;
            COMMIT;")?;
        conn.execute_batch("BEGIN IMMEDIATE;
          CREATE TABLE IF NOT EXISTS flow_bodies(flow_id TEXT NOT NULL, slot TEXT NOT NULL, body BLOB NOT NULL, encoded_sha256 BLOB, logical_length INTEGER, PRIMARY KEY(flow_id,slot));
          CREATE TRIGGER IF NOT EXISTS flow_bodies_delete AFTER DELETE ON flows BEGIN DELETE FROM flow_bodies WHERE flow_id=old.id; END;
          CREATE TABLE IF NOT EXISTS flow_projection(flow_id TEXT PRIMARY KEY, version INTEGER NOT NULL, metadata TEXT NOT NULL, body_refs TEXT NOT NULL);
          CREATE TRIGGER IF NOT EXISTS flow_projection_delete AFTER DELETE ON flows BEGIN DELETE FROM flow_projection WHERE flow_id=old.id; END;
          CREATE TRIGGER IF NOT EXISTS flow_body_revision_insert AFTER INSERT ON flow_bodies
            WHEN EXISTS(SELECT 1 FROM flows WHERE id=new.flow_id)
            BEGIN UPDATE flow_revision SET version=version+1 WHERE id=1;
              INSERT INTO flow_changes VALUES(new.flow_id,(SELECT version FROM flow_revision WHERE id=1)) ON CONFLICT(id) DO UPDATE SET version=excluded.version;
              DELETE FROM flow_projection WHERE flow_id=new.flow_id; END;
          CREATE TRIGGER IF NOT EXISTS flow_body_revision_update AFTER UPDATE ON flow_bodies
            WHEN EXISTS(SELECT 1 FROM flows WHERE id=new.flow_id)
            BEGIN UPDATE flow_revision SET version=version+1 WHERE id=1;
              INSERT INTO flow_changes VALUES(new.flow_id,(SELECT version FROM flow_revision WHERE id=1)) ON CONFLICT(id) DO UPDATE SET version=excluded.version;
              DELETE FROM flow_projection WHERE flow_id=new.flow_id; END;
          CREATE TRIGGER IF NOT EXISTS flow_body_revision_delete AFTER DELETE ON flow_bodies
            WHEN EXISTS(SELECT 1 FROM flows WHERE id=old.flow_id)
            BEGIN UPDATE flow_revision SET version=version+1 WHERE id=1;
              INSERT INTO flow_changes VALUES(old.flow_id,(SELECT version FROM flow_revision WHERE id=1)) ON CONFLICT(id) DO UPDATE SET version=excluded.version;
              DELETE FROM flow_projection WHERE flow_id=old.flow_id; END;
          COMMIT;")?;
        {
            // Serialize schema discovery and upgrade across processes opening the
            // same database, so two windows cannot both add the column.
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let columns=tx.prepare("PRAGMA table_info(flow_bodies)")?.query_map([],|row|row.get::<_,String>(1))?.collect::<rusqlite::Result<Vec<_>>>()?;
            if !columns.iter().any(|name|name=="encoded_sha256") {tx.execute_batch("ALTER TABLE flow_bodies ADD COLUMN encoded_sha256 BLOB")?;}
            if !columns.iter().any(|name|name=="logical_length") {tx.execute_batch("ALTER TABLE flow_bodies ADD COLUMN logical_length INTEGER")?;}
            tx.commit()?;
        }
        conn.execute("UPDATE flows SET data=json_set(data,'$.state','interrupted') WHERE json_extract(data,'$.state') IN ('sending','receiving')", [])?;
        crate::audit::initialize(&conn)?;
        let migration_epoch:Option<String>=conn.query_row("SELECT value FROM settings WHERE key='config-migration.epoch.v1'",[],|row|row.get(0)).optional()?;
        Ok(Self(Mutex::new(conn), uuid::Uuid::new_v4().to_string(),std::sync::atomic::AtomicBool::new(false),migration_epoch))
    }
    pub fn insert(&self, flow: &Flow) -> Result<()> {
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx=conn.transaction()?;
        write_flow(&tx,flow)?;
        tx.commit()?; Ok(())
    }
    /// Save a live stream snapshot. The named body may grow within reserved
    /// SQLite BLOB capacity; all other slots keep exact storage semantics.
    pub fn insert_streaming(&self, flow:&Flow, growing_slot:&str) -> Result<()> {
        anyhow::ensure!(matches!(growing_slot,"request"|"response"),"Invalid growing body slot");
        let body=if growing_slot=="request" {Some(&flow.request.body_base64)} else {flow.response.as_ref().map(|response|&response.body_base64)};
        let persisted=body.map(|body| -> Result<(&SharedBody,usize)> {
            let tail=std::str::from_utf8(&body.as_bytes()[body.len().saturating_sub(4)..])?;
            let length=usize::try_from(Base64ReadPlan::new(body.len(),tail,0,1)?.body.total_bytes)?;
            Ok((body,length))
        }).transpose()?;
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx=conn.transaction()?;
        write_flow_with_growth(&tx,flow,Some(growing_slot))?;
        tx.commit()?;
        if let Some((body,length))=persisted {body.mark_persisted_base64(length);}
        Ok(())
    }
    pub fn delete_flows(&self, ids: Vec<String>) -> Result<()> {
        anyhow::ensure!(ids.len() <= 10000, "一次最多删除 10000 条记录");
        let mut conn = self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx = conn.transaction()?;
        for id in ids { tx.execute("INSERT OR IGNORE INTO deleted_flows(id) VALUES(?1)",[&id])?; tx.execute("DELETE FROM flows WHERE id=?1",[&id])?; }
        tx.commit()?; Ok(())
    }
    pub fn import_flows(&self, mut flows: Vec<Flow>) -> Result<usize> {
        anyhow::ensure!(!flows.is_empty() && flows.len() <= 10000, "导入记录数应为 1–10000");
        let mut conn = self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx = conn.transaction()?;
        for flow in &mut flows {
            flow.state = "imported".into();
            flow.id = uuid::Uuid::new_v4().to_string(); flow.parent_id = None; flow.source = "capture".into();
            flow.request.scripts = Default::default(); flow.request.upstream_profile_id = None;
            if let Some(r) = &mut flow.original_request { r.scripts = Default::default(); r.upstream_profile_id = None; }
            flow.notes.push("从文件导入；不是本机实时捕获。脚本和上游代理关联已清除。".into());
            write_flow(&tx,flow)?;
        }
        tx.commit()?; Ok(flows.len())
    }
    pub fn changes(&self, since: Option<&str>) -> Result<FlowChanges> {
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx=conn.transaction()?;
        let version:i64=tx.query_row("SELECT version FROM flow_revision WHERE id=1",[],|r|r.get(0))?;
        let previous=since.and_then(|s|s.strip_prefix(&format!("{}:",self.1))).and_then(|s|s.parse::<i64>().ok()).filter(|n|*n>=0&&*n<=version);
        let reset=previous.is_none();
        let rows={let mut stmt=tx.prepare(if reset {"SELECT rowid,data FROM flows ORDER BY rowid DESC"} else {"SELECT f.rowid,f.data FROM flows f JOIN flow_changes c ON c.id=f.id WHERE c.version>?1 ORDER BY f.rowid DESC"})?;
            let mut cursor=if reset{stmt.query([])?}else{stmt.query([previous.unwrap()])?};let mut rows=Vec::new();while let Some(row)=cursor.next()?{rows.push(FlowChange{position:row.get(0)?,flow:restore_bodies(&tx,&row.get::<_,String>(1)?)?});}rows};
        let deleted=if let Some(previous)=previous{let mut stmt=tx.prepare("SELECT c.id FROM flow_changes c LEFT JOIN flows f ON f.id=c.id WHERE c.version>?1 AND f.id IS NULL")?;let rows=stmt.query_map([previous],|r|r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;rows}else{Vec::new()};
        tx.commit()?;Ok(FlowChanges{revision:format!("{}:{version}",self.1),reset,rows,deleted})
    }
    pub fn revision(&self) -> Result<String> {
        let conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let version: i64 = conn.query_row("SELECT version FROM flow_revision WHERE id=1", [], |row| row.get(0))?;
        // A new store instance also invalidates clients after service/database replacement.
        Ok(format!("{}:{version}", self.1))
    }
    pub fn list(&self) -> Result<Vec<Flow>> {
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx=conn.transaction()?;
        let flows={let mut stmt=tx.prepare("SELECT data FROM flows ORDER BY rowid DESC")?;
          let rows=stmt.query_map([],|r|r.get::<_,String>(0))?;
          rows.map(|r|restore_bodies(&tx,&r?)).collect::<Result<Vec<_>>>()?};
        tx.commit()?;Ok(flows)
    }

    /// Write a desktop-compatible session without retaining every flow in memory.
    /// A separate read transaction keeps the export consistent while capture writes continue.
    pub fn export_session_json(&self, mut output: impl std::io::Write) -> Result<usize> {
        let database = {
            let conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
            conn.path().ok_or_else(|| anyhow::anyhow!("Database has no file path"))?.to_owned()
        };
        let mut reader = Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let tx = reader.transaction()?;
        output.write_all(b"{\"format\":\"http-capture-session\",\"version\":1,\"flows\":[")?;
        let count = {
            let mut statement = tx.prepare("SELECT data FROM flows ORDER BY rowid DESC")?;
            let mut rows = statement.query([])?;
            let mut count = 0;
            while let Some(row) = rows.next()? {
                let raw: String = row.get(0)?;
                let flow = restore_bodies(&tx, &raw)?;
                if count != 0 { output.write_all(b",")?; }
                serde_json::to_writer(&mut output, &flow)?;
                count += 1;
            }
            count
        };
        output.write_all(b"]}")?;
        tx.commit()?;
        Ok(count)
    }

}

// Keep public Flow JSON backwards compatible. Large entity bodies live in the same
// SQLite transaction/backup as metadata; no external paths or dangling files.
const BODY_INLINE_LIMIT:usize=64*1024;
fn body_slots(flow:&mut Flow, mut visit:impl FnMut(&str,&mut SharedBody)->Result<()>) -> Result<()> {
 visit("request",&mut flow.request.body_base64)?;
 if let Some(v)=&mut flow.response {visit("response",&mut v.body_base64)?;}
 if let Some(v)=&mut flow.original_request {visit("originalRequest",&mut v.body_base64)?;}
 if let Some(v)=&mut flow.original_response {visit("originalResponse",&mut v.body_base64)?;}
 Ok(())
}
fn write_flow(conn:&Connection,flow:&Flow)->Result<()> { write_flow_with_growth(conn,flow,None) }

/// Hash the canonical encoding of the old decoded prefix without reading the
/// stored BLOB or allocating a second body. A previous partial Base64 quartet
/// needs its old padding restored before it can match the saved digest.
fn encoded_prefix_matches(body:&SharedBody,old_bytes:usize,digest:&[u8])->Result<bool> {
 use base64::{engine::general_purpose::STANDARD,Engine as _};
 use sha2::{Digest,Sha256};
 let complete=old_bytes/3*4;
 let Some(prefix)=body.as_bytes().get(..complete) else {return Ok(false)};
 let mut hasher=Sha256::new();hasher.update(prefix);
 let remainder=old_bytes%3;
 if remainder!=0 {
  let Some(quantum)=body.as_bytes().get(complete..complete+4) else {return Ok(false)};
  let decoded=STANDARD.decode(quantum)?;
  if decoded.len()<remainder {return Ok(false)}
  hasher.update(STANDARD.encode(&decoded[..remainder]));
 }
 Ok(hasher.finalize().as_slice()==digest)
}

fn reserved_blob_size(size:i32)->Result<i32> {
 const MAX_HEADROOM:usize=16*1024*1024;
 let size=size as usize;
 let headroom=(size/2).clamp(BODY_INLINE_LIMIT*2,MAX_HEADROOM);
 let wanted=size.saturating_add(headroom).min(i32::MAX as usize);
 let rounded=wanted.saturating_add(BODY_INLINE_LIMIT-1)/BODY_INLINE_LIMIT*BODY_INLINE_LIMIT;
 Ok(i32::try_from(rounded.min(i32::MAX as usize))?)
}

fn write_flow_with_growth(conn:&Connection,flow:&Flow,growing_slot:Option<&str>)->Result<()> {
 use base64::engine::general_purpose::STANDARD;
 if conn.query_row("SELECT EXISTS(SELECT 1 FROM deleted_flows WHERE id=?1)",[&flow.id],|r|r.get::<_,bool>(0))? {return Ok(());}
 let mut stored=flow.clone();
 let mut retained=Vec::new();
 let mut body_refs=BTreeMap::new();
 body_slots(&mut stored,|slot,body| {
  let bytes=body.as_bytes();
  let tail=std::str::from_utf8(&bytes[bytes.len().saturating_sub(4)..])?;
  let length=Base64ReadPlan::new(bytes.len(),tail,0,1)?.body.total_bytes;
  body_refs.insert(slot.to_owned(),body_views::BodyReference{byte_length:length});
  if body.len()>BODY_INLINE_LIMIT*4/3 {
   // Allocate inside SQLite, then decode directly into the incremental BLOB.
   // The enclosing transaction rolls back metadata and partial writes on errors.
   anyhow::ensure!(body.len()%4==0,"Invalid Base64 body length");
   let padding=body.as_bytes().iter().rev().take_while(|&&b|b==b'=').count();
   anyhow::ensure!(padding<=2,"Invalid Base64 padding");
   let size=i32::try_from(body.len()/4*3-padding)?;
   let digest=*body.encoded_sha256();
   let existing:Option<(i64,i32,i32,Option<Vec<u8>>)>=conn.query_row(
    "SELECT rowid,length(body),coalesce(logical_length,length(body)),encoded_sha256 FROM flow_bodies WHERE flow_id=?1 AND slot=?2",
    params![flow.id,slot],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
   ).optional()?;
   let growing=growing_slot==Some(slot);
   // A bidirectional gRPC snapshot can update one slot while the other is
   // still growing. Preserve the unchanged slot's spare capacity until the
   // final ordinary insert compacts both bodies.
   let unchanged=existing.as_ref().is_some_and(|(_,capacity,length,hash)|*length==size&&hash.as_deref()==Some(digest.as_slice())&&(growing_slot.is_some()||*capacity==size));
   if !unchanged {
    let append=if let Some((_,capacity,length,Some(hash)))=&existing {
     growing&&*length>=0&&*length<size&&size<=*capacity&&(
      body.matches_persisted_base64(*length as usize,hash)||encoded_prefix_matches(body,*length as usize,hash)?
     )
    } else {false};
    if append {
     let (rowid,_,length,_)=existing.as_ref().unwrap();
     let mut blob=conn.blob_open(rusqlite::DatabaseName::Main,"flow_bodies","body",*rowid,false)?;
     let aligned=(*length as usize)/3*4;
     let mut decoder=base64::read::DecoderReader::new(&body.as_bytes()[aligned..],&STANDARD);
     let mut skipped=[0u8;2];
     std::io::Read::read_exact(&mut decoder,&mut skipped[..(*length as usize)%3])?;
     std::io::Seek::seek(&mut blob,std::io::SeekFrom::Start(*length as u64))?;
     let written=std::io::copy(&mut decoder,&mut blob)?;
     anyhow::ensure!(written==(size-*length) as u64,"Decoded body append size mismatch");
     blob.close()?;
     conn.execute("UPDATE flow_bodies SET logical_length=?3,encoded_sha256=?4 WHERE flow_id=?1 AND slot=?2",params![flow.id,slot,size,digest.as_slice()])?;
    } else {
     conn.execute("DELETE FROM flow_bodies WHERE flow_id=?1 AND slot=?2",params![flow.id,slot])?;
     let capacity=if growing {reserved_blob_size(size)?} else {size};
     conn.execute("INSERT INTO flow_bodies(flow_id,slot,body,encoded_sha256,logical_length) VALUES(?1,?2,?3,?4,?5)",params![flow.id,slot,rusqlite::blob::ZeroBlob(capacity),digest.as_slice(),size])?;
     let rowid=conn.last_insert_rowid();
     let mut blob=conn.blob_open(rusqlite::DatabaseName::Main,"flow_bodies","body",rowid,false)?;
     let mut decoder=base64::read::DecoderReader::new(body.as_bytes(),&STANDARD);
     let written=std::io::copy(&mut decoder,&mut blob)?;
     anyhow::ensure!(written==size as u64,"Decoded body size mismatch");
     blob.close()?;
    }
   }
   retained.push(slot.to_owned());
   // Explicit marker prevents a missing BLOB from looking like an empty body.
   *body=format!("@body:{slot}").into();
  }
  Ok(())
 })?;
 for slot in ["request","response","originalRequest","originalResponse"] {
  if !retained.iter().any(|value|value==slot) {conn.execute("DELETE FROM flow_bodies WHERE flow_id=?1 AND slot=?2",params![flow.id,slot])?;}
 }
 let data=serde_json::to_string(&stored)?;
 conn.execute("INSERT INTO flows(id,started_at,data) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![flow.id,flow.started_at,data])?;
 let version:i64=conn.query_row("SELECT version FROM flow_changes WHERE id=?1",[&flow.id],|row|row.get(0))?;
 conn.execute("INSERT INTO flow_projection(flow_id,version,metadata,body_refs) VALUES(?1,?2,json_remove(?3,'$.request.bodyBase64','$.response.bodyBase64','$.originalRequest.bodyBase64','$.originalResponse.bodyBase64'),?4)
     ON CONFLICT(flow_id) DO UPDATE SET version=excluded.version,metadata=excluded.metadata,body_refs=excluded.body_refs",
     params![flow.id,version,data,serde_json::to_string(&body_refs)?])?;
 Ok(())
}
fn restore_bodies(conn:&Connection,raw:&str)->Result<Flow> {
 use base64::engine::general_purpose::STANDARD;
 let mut flow:Flow=serde_json::from_str(raw)?;let id=flow.id.clone();
 body_slots(&mut flow,|slot,body| {
  if body.starts_with("@body:") {
   anyhow::ensure!(body.as_str()==format!("@body:{slot}"),"Invalid stored body reference");
   let (rowid,length):(i64,i64)=conn.query_row("SELECT rowid,coalesce(logical_length,length(body)) FROM flow_bodies WHERE flow_id=?1 AND slot=?2",params![id,slot],|r|Ok((r.get(0)?,r.get(1)?)))?;
   anyhow::ensure!(length>=0,"Invalid stored body length");
   let mut blob=conn.blob_open(rusqlite::DatabaseName::Main,"flow_bodies","body",rowid,true)?;
   anyhow::ensure!(length as usize<=blob.len(),"Stored body length exceeds BLOB capacity");
   let mut encoder=base64::write::EncoderWriter::new(Vec::new(),&STANDARD);
   std::io::copy(&mut (&mut blob).take(length as u64),&mut encoder)?;
   *body=String::from_utf8(encoder.finish()?)?.into();
  }
  Ok(())
 })?;
 Ok(flow)
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Workspace {
 pub editors: Vec<RequestEditor>,
 pub active_editor: String,
 #[serde(default)] pub revision:u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct RequestEditor {
 pub id:String,
 pub draft:RequestDraft,
 pub selected:Option<String>,
 pub parent_id:Option<String>,
 #[serde(default, skip_serializing_if="Option::is_none")] pub grpc_draft:Option<GrpcDraft>,
 #[serde(default, skip_serializing_if="Option::is_none")] pub saved_request_id:Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct GrpcDraft { pub method:String, pub json:String }
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Library {
 pub revision:u64,
 pub collections:Vec<Collection>,
 pub requests:Vec<SavedRequest>,
 pub environments:Vec<Environment>,
 pub scripts:Vec<SavedScript>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collection {pub id:String,pub name:String}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct SavedRequest {
 pub id:String,pub name:String,pub collection_id:String,pub draft:RequestDraft,
 #[serde(default, skip_serializing_if="Option::is_none")] pub grpc_draft:Option<GrpcDraft>,
 #[serde(default, skip_serializing_if="Option::is_none")] pub request_mode:Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {pub id:String,pub name:String,pub variables:std::collections::BTreeMap<String,String>}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedScript {pub id:String,pub name:String,pub scripts:crate::scripts::Scripts}
impl Store {
 pub fn library(&self)->Result<Library>{Ok(self.setting("library.v1")?.unwrap_or_default())}
 pub fn save_library(&self,mut value:Library)->Result<u64>{
  let mut ids=std::collections::HashSet::new();
  for (id,name) in value.collections.iter().map(|v|(&v.id,&v.name)).chain(value.requests.iter().map(|v|(&v.id,&v.name))).chain(value.environments.iter().map(|v|(&v.id,&v.name))).chain(value.scripts.iter().map(|v|(&v.id,&v.name))) {
   anyhow::ensure!(!id.is_empty()&&id.len()<=128&&ids.insert(id)&&!name.trim().is_empty()&&name.len()<=512,"Library ID or name invalid");
  }
  for request in &value.requests {anyhow::ensure!(value.collections.iter().any(|c|c.id==request.collection_id),"Collection not found");request.draft.scripts.validate()?;}
  for script in &value.scripts {script.scripts.validate()?;}
  let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
  let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
  self.configuration_ready_in(&tx)?;
  let raw:Option<String>=tx.query_row("SELECT value FROM settings WHERE key='library.v1'",[],|r|r.get(0)).optional()?;
  let previous:Library=raw.map(|s|serde_json::from_str(&s)).transpose()?.unwrap_or_default();
  anyhow::ensure!(previous.revision==value.revision,"Library changed in another window; export local edits before reloading");
  value.revision+=1;
  tx.execute("INSERT INTO settings(key,value) VALUES('library.v1',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(&value)?])?;
  tx.commit()?;Ok(value.revision)
 }
 pub fn setting<T:serde::de::DeserializeOwned>(&self,key:&str)->Result<Option<T>> {
  let conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
  let raw:Option<String>=conn.query_row("SELECT value FROM settings WHERE key=?1",[key],|r|r.get(0)).optional()?;
  raw.map(|s|serde_json::from_str(&s).map_err(Into::into)).transpose()
 }
 pub fn save_setting<T:Serialize>(&self,key:&str,value:&T)->Result<()> {
  let raw=serde_json::to_string(value)?;
  let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
  let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
  self.configuration_ready_in(&tx)?;
  tx.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,raw])?;tx.commit()?;Ok(())
 }
 pub fn workspace(&self)->Result<Workspace>{Ok(self.setting("workspace.v1")?.unwrap_or_default())}
 pub fn save_workspace(&self,mut value:Workspace)->Result<u64>{
  anyhow::ensure!(value.editors.len()<=100,"最多保存 100 个请求标签");
  let mut ids=std::collections::HashSet::new();
  for editor in &value.editors {
   anyhow::ensure!(!editor.id.is_empty()&&editor.id.len()<=128&&ids.insert(&editor.id),"请求标签 ID 无效或重复");
   editor.draft.scripts.validate()?;
  }
  anyhow::ensure!(value.active_editor.is_empty()||ids.contains(&value.active_editor),"当前请求标签不存在");
  let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
  let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
  self.configuration_ready_in(&tx)?;
  let raw:Option<String>=tx.query_row("SELECT value FROM settings WHERE key='workspace.v1'",[],|r|r.get(0)).optional()?;
  let previous:Workspace=raw.map(|s|serde_json::from_str(&s)).transpose()?.unwrap_or_default();
  anyhow::ensure!(previous.revision==value.revision,"请求工作区已在其他窗口修改；请保留当前内容后刷新，避免覆盖其他窗口");
  value.revision+=1;
  tx.execute("INSERT INTO settings(key,value) VALUES('workspace.v1',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(&value)?])?;
  tx.commit()?;Ok(value.revision)
 }
}

#[cfg(test)]
mod session_tests {
 use super::*;
 #[test]
 fn delta_and_library_conflicts() {
  let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("sessions.db")).unwrap();
  let initial=store.changes(None).unwrap();assert!(initial.reset);assert!(store.changes(Some(&initial.revision)).unwrap().rows.is_empty());
  let flow:Flow=serde_json::from_value(serde_json::json!({"id":"a","parentId":null,"startedAt":1,"durationMs":1,"source":"capture","request":{"method":"GET","url":"https://example.invalid","headers":[],"bodyBase64":""},"response":null,"error":null,"notes":[]})).unwrap();
  store.insert(&flow).unwrap();let next=store.changes(Some(&initial.revision)).unwrap();assert!(!next.reset);assert_eq!(next.rows.len(),1);
  store.delete_flows(vec!["a".into()]).unwrap();let deleted=store.changes(Some(&next.revision)).unwrap();assert_eq!(deleted.deleted,vec!["a"]);assert!(deleted.rows.is_empty());assert!(store.changes(Some("another-instance:1")).unwrap().reset);
  let mut library=Library::default();library.collections.push(Collection{id:"c".into(),name:"Saved".into()});library.requests.push(SavedRequest{id:"r".into(),name:"Request".into(),collection_id:"c".into(),draft:flow.request,grpc_draft:None,request_mode:None});
  assert_eq!(store.save_library(library.clone()).unwrap(),1);assert!(store.save_library(library).is_err());
  let loaded=store.library().unwrap();assert_eq!(loaded.requests.len(),1);assert_eq!(loaded.revision,1);
 }
 #[test]
 fn staged_import_backup_and_compaction() {
  let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("sessions.db")).unwrap();
  let flow:Flow=serde_json::from_value(serde_json::json!({"id":"a","parentId":null,"startedAt":1,"durationMs":1,"source":"capture","request":{"method":"GET","url":"https://example.invalid","headers":[],"bodyBase64":""},"response":null,"error":null,"notes":[]})).unwrap();
  let id=store.begin_import().unwrap();store.append_import(&id,0,vec![flow.clone();10001]).unwrap();assert!(store.list().unwrap().is_empty());
  assert!(store.append_import(&id,0,vec![flow.clone()]).is_err());assert_eq!(store.finish_import(&id,true).unwrap(),10001);
  store.save_setting("test",&"saved").unwrap();let path=store.backup().unwrap();let backup=Store::open(Path::new(&path)).unwrap();assert_eq!(backup.list().unwrap().len(),10001);assert_eq!(backup.setting::<String>("test").unwrap().as_deref(),Some("saved"));
  assert_eq!(store.clear().unwrap(),10001);store.compact().unwrap();assert!(store.list().unwrap().is_empty());assert_eq!(backup.list().unwrap().len(),10001);
  let id=store.begin_import().unwrap();store.append_import(&id,0,vec![flow]).unwrap();store.finish_import(&id,false).unwrap();assert!(store.list().unwrap().is_empty());
 }
 #[test]
 fn revision_tracks_only_committed_flow_changes_and_reopen() {
  let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("revision.db");
  let store=Store::open(&path).unwrap();
  let initial=store.revision().unwrap();
  store.list().unwrap();store.save_setting("test", &true).unwrap();
  assert_eq!(initial,store.revision().unwrap());
  let mut flow:Flow=serde_json::from_value(serde_json::json!({"id":"test","parentId":null,"startedAt":1,"durationMs":1,"source":"capture","request":{"method":"GET","url":"https://example.invalid","headers":[],"bodyBase64":""},"response":null,"error":null,"notes":[]})).unwrap();
  store.insert(&flow).unwrap();let inserted=store.revision().unwrap();assert_ne!(initial,inserted);
  flow.notes.push("stream update".into());store.insert(&flow).unwrap();let updated=store.revision().unwrap();assert_ne!(inserted,updated);
  { let mut conn=store.0.lock().unwrap();let tx=conn.transaction().unwrap();tx.execute("DELETE FROM flows",[]).unwrap();tx.rollback().unwrap(); }
  assert_eq!(updated,store.revision().unwrap());
  let other=Store::open(&path).unwrap();other.delete_flows(vec![flow.id.clone()]).unwrap();
  let deleted=store.revision().unwrap();assert_ne!(updated,deleted);
  store.insert(&flow).unwrap();assert_eq!(deleted,store.revision().unwrap());
  store.import_flows(vec![flow]).unwrap();assert_ne!(deleted,store.revision().unwrap());
  let before=store.revision().unwrap();drop(store);let reopened=Store::open(&path).unwrap();assert_ne!(before,reopened.revision().unwrap());assert_eq!(reopened.list().unwrap().len(),1);
 }
 #[test]
 fn history_beyond_200_survives_updates_deletion_and_reopen() {
  let directory=tempfile::tempdir().unwrap();
  let path=directory.path().join("history.db");
  let mut flow:Flow=serde_json::from_value(serde_json::json!({"id":"0","parentId":null,"startedAt":1,"durationMs":1,"source":"capture","request":{"method":"GET","url":"https://example.com/","headers":[],"bodyBase64":""},"response":null,"error":null,"notes":[]})).unwrap();
  {
   let store=Store::open(&path).unwrap();
   for i in 0..251 { flow.id=i.to_string();store.insert(&flow).unwrap(); }
   let rows=store.list().unwrap();
   assert_eq!(rows.len(),251);assert_eq!(rows.first().unwrap().id,"250");assert_eq!(rows.last().unwrap().id,"0");
   flow.id="0".into();flow.notes.push("updated".into());store.insert(&flow).unwrap();
   assert_eq!(store.list().unwrap().last().unwrap().notes,vec!["updated"]);
   store.delete_flows(vec!["0".into(),"250".into()]).unwrap();
  }
  let rows=Store::open(&path).unwrap().list().unwrap();
  assert_eq!(rows.len(),249);assert_eq!(rows.first().unwrap().id,"249");assert_eq!(rows.last().unwrap().id,"1");
 }
 #[test]
 fn deleted_flow_cannot_return_and_import_is_persistent() {
  let path=std::env::temp_dir().join(format!("capture-store-{}.db",uuid::Uuid::new_v4()));
  let flow:Flow=serde_json::from_value(serde_json::json!({"id":"original","parentId":null,"startedAt":1,"durationMs":1,"source":"capture","request":{"method":"GET","url":"https://example.com/","headers":[{"name":"X-Test","value":"1"},{"name":"X-Test","value":"2"}],"bodyBase64":"","tls":{"preset":"native"}},"response":null,"error":null,"notes":[]})).unwrap();
  { let store=Store::open(&path).unwrap(); store.insert(&flow).unwrap(); store.delete_flows(vec![flow.id.clone()]).unwrap(); store.insert(&flow).unwrap(); assert!(store.list().unwrap().is_empty()); assert_eq!(store.import_flows(vec![flow.clone()]).unwrap(),1); }
  { let store=Store::open(&path).unwrap(); let rows=store.list().unwrap(); assert_eq!(rows.len(),1); assert_ne!(rows[0].id,flow.id); assert_eq!(rows[0].request.headers[1].value,"2"); store.insert(&flow).unwrap(); assert_eq!(store.list().unwrap().len(),1); }
  let _=std::fs::remove_file(path);
 }
}

#[cfg(test)]
mod body_storage_tests {
 use super::*;
 use base64::{Engine as _,engine::general_purpose::STANDARD};
 fn sample()->Flow {serde_json::from_value(serde_json::json!({"id":"large","startedAt":1,"durationMs":1,"source":"capture","request":{"method":"POST","url":"https://example.invalid","headers":[],"bodyBase64":STANDARD.encode(vec![7u8;100000])},"response":null,"error":null,"notes":[]})).unwrap()}
 #[test]
 fn session_export_restores_large_bodies_in_desktop_format() {
  let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("capture.db")).unwrap();
  let large=sample();store.insert(&large).unwrap();
  let mut small=large.clone();small.id="small".into();small.started_at=2;
  small.request.body_base64=SharedBody::default();store.insert(&small).unwrap();
  let mut output=Vec::new();
  assert_eq!(store.export_session_json(&mut output).unwrap(),2);
  let session:serde_json::Value=serde_json::from_slice(&output).unwrap();
  assert_eq!(session["format"],"http-capture-session");
  assert_eq!(session["version"],1);
  let flows=session["flows"].as_array().unwrap();
  assert_eq!(flows.len(),2);
  assert_eq!(flows[0]["id"],"small");
  assert_eq!(flows[1]["request"]["bodyBase64"],large.request.body_base64.as_str());
  assert!(flows[1]["request"]["bodyBase64"].as_str().unwrap().starts_with("BwcH"));
 }
 #[test]
 fn encoded_prefix_proof_handles_all_base64_boundaries() {
  use sha2::{Digest,Sha256};
  let bytes:Vec<u8>=(0..100).map(|n|n as u8).collect();
  let full=SharedBody::from(STANDARD.encode(&bytes));
  for old in 0..bytes.len() {
   let hash=Sha256::digest(STANDARD.encode(&bytes[..old]));
   assert!(encoded_prefix_matches(&full,old,&hash).unwrap(),"old length {old}");
   if old>0 {
    let mut changed=bytes.clone();changed[old-1]^=0x55;
    assert!(!encoded_prefix_matches(&SharedBody::from(STANDARD.encode(changed)),old,&hash).unwrap(),"old length {old}");
   }
  }
 }
 #[test]
 fn growing_body_appends_in_reserved_blob_and_all_readers_ignore_capacity() {
  let dir=tempfile::tempdir().unwrap();let path=dir.path().join("growth.db");
  let store=Store::open(&path).unwrap();let mut flow=sample();
  store.insert_streaming(&flow,"request").unwrap();
  let location=||store.0.lock().unwrap().query_row("SELECT rowid,length(body),logical_length FROM flow_bodies WHERE flow_id='large' AND slot='request'",[],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,usize>(1)?,row.get::<_,usize>(2)?))).unwrap();
  let first=location();assert_eq!(first.2,100000);assert!(first.1>first.2);
  store.0.lock().unwrap().execute_batch("CREATE TABLE body_write_events(kind TEXT NOT NULL);
    CREATE TRIGGER test_blob_insert AFTER INSERT ON flow_bodies BEGIN INSERT INTO body_write_events VALUES('insert'); END;
    CREATE TRIGGER test_blob_delete AFTER DELETE ON flow_bodies BEGIN INSERT INTO body_write_events VALUES('delete'); END;
    CREATE TRIGGER test_blob_update AFTER UPDATE ON flow_bodies BEGIN INSERT INTO body_write_events VALUES('update'); END;").unwrap();
  let extra=vec![9u8;12345];crate::body_base64::append_complete(&mut flow.request.body_base64,&extra).unwrap();
  store.insert_streaming(&flow,"request").unwrap();
  let second=location();assert_eq!(second.0,first.0);assert_eq!(second.1,first.1);assert_eq!(second.2,112345);
  let events:Vec<String>={let conn=store.0.lock().unwrap();let mut statement=conn.prepare("SELECT kind FROM body_write_events").unwrap();statement.query_map([],|row|row.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap()};
  assert_eq!(events,["update"]);
  assert_eq!(store.get(&flow.id).unwrap().unwrap().request.body_base64,flow.request.body_base64);
  let version=store.metadata(&flow.id,None).unwrap().unwrap().body_revision;
  let end=store.read_body_at_revision(&flow.id,"request",&version,112340,16).unwrap();
  assert_eq!(STANDARD.decode(end.body_base64).unwrap(),vec![9u8;5]);assert_eq!(end.total_bytes,112345);
  let mut exported=Vec::new();assert_eq!(store.copy_body_at_revision(&flow.id,"request",&version,&mut exported).unwrap(),112345);
  assert_eq!(exported.len(),112345);assert_eq!(&exported[100000..],extra);
  assert_eq!(store.mcp_read_body(&flow.id,"request",112340,16).unwrap().total_bytes,112345);
  let backup=store.backup().unwrap();assert_eq!(Store::open(Path::new(&backup)).unwrap().get(&flow.id).unwrap().unwrap().request.body_base64,flow.request.body_base64);
  store.insert(&flow).unwrap();let final_location=location();assert_eq!(final_location.1,112345);assert_eq!(final_location.2,112345);
  assert_eq!(store.get(&flow.id).unwrap().unwrap().request.body_base64,flow.request.body_base64);
 }
 #[test]
 fn growing_body_capacity_change_replacement_and_failure_are_atomic() {
  let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("growth.db")).unwrap();let mut flow=sample();
  store.insert_streaming(&flow,"request").unwrap();
  crate::body_base64::append_complete(&mut flow.request.body_base64,&vec![3u8;40000]).unwrap();
  // A freshly reconstructed Flow has no in-memory commit checkpoint; the
  // encoded-prefix proof must still accept the same append safely.
  flow.request.body_base64=SharedBody::from(flow.request.body_base64.to_string());
  store.insert_streaming(&flow,"request").unwrap();
  let (capacity,length):(usize,usize)=store.0.lock().unwrap().query_row("SELECT length(body),logical_length FROM flow_bodies WHERE slot='request'",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
  assert_eq!(length,140000);assert!(capacity>length);
  let mut modified=flow.clone();modified.request.body_base64.replace_range(0..4,"AQID");
  store.insert_streaming(&modified,"request").unwrap();
  assert_eq!(store.get(&flow.id).unwrap().unwrap().request.body_base64,modified.request.body_base64);
  let mut invalid=modified.clone();invalid.request.body_base64.replace_range(8..9,"!");
  assert!(store.insert_streaming(&invalid,"request").is_err());
  assert_eq!(store.get(&flow.id).unwrap().unwrap().request.body_base64,modified.request.body_base64);
 }
 #[test]
 fn bidirectional_snapshot_keeps_the_other_growing_slot_reserved() {
  let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("bidi.db")).unwrap();let mut flow=sample();
  flow.response=Some(serde_json::from_value(serde_json::json!({"status":200,"version":"HTTP/2","headers":[],"bodyBase64":STANDARD.encode(vec![8u8;100001])})).unwrap());
  store.insert_streaming(&flow,"response").unwrap();
  let response_before:(i64,usize)=store.0.lock().unwrap().query_row("SELECT rowid,length(body) FROM flow_bodies WHERE slot='response'",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
  assert!(response_before.1>100001);
  crate::body_base64::append_complete(&mut flow.request.body_base64,b"request append").unwrap();
  store.insert_streaming(&flow,"request").unwrap();
  let response_after:(i64,usize)=store.0.lock().unwrap().query_row("SELECT rowid,length(body) FROM flow_bodies WHERE slot='response'",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
  assert_eq!(response_after,response_before);
  assert_eq!(store.get(&flow.id).unwrap().unwrap().response.unwrap().body_base64,flow.response.as_ref().unwrap().body_base64);
 }
 #[test]
 fn unchanged_large_body_reuses_blob_across_flow_and_response_updates(){
  let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("s.db")).unwrap();let mut flow=sample();
  store.insert(&flow).unwrap();
  let location=||store.0.lock().unwrap().query_row("SELECT rowid,encoded_sha256 FROM flow_bodies WHERE flow_id='large' AND slot='request'",[],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,Vec<u8>>(1)?))).unwrap();
  let original=location();assert_eq!(original.1.len(),32);
  flow.notes.push("progress".into());store.insert(&flow).unwrap();assert_eq!(location(),original);
  flow.response=Some(serde_json::from_value(serde_json::json!({"status":200,"version":"HTTP/2","headers":[],"bodyBase64":STANDARD.encode(vec![8u8;100000])})).unwrap());
  store.insert(&flow).unwrap();assert_eq!(location(),original);
  flow.request.body_base64=STANDARD.encode(vec![9u8;100000]).into();store.insert(&flow).unwrap();
  assert_ne!(location().1,original.1);assert_eq!(STANDARD.decode(store.get("large").unwrap().unwrap().request.body_base64).unwrap(),vec![9u8;100000]);
  flow.response=None;store.insert(&flow).unwrap();
  assert_eq!(store.0.lock().unwrap().query_row("SELECT count(*) FROM flow_bodies WHERE flow_id='large'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
 }
 #[test]
 fn older_body_table_upgrades_and_missing_digest_is_rebuilt(){
  let dir=tempfile::tempdir().unwrap();let path=dir.path().join("legacy.db");
  Connection::open(&path).unwrap().execute_batch("CREATE TABLE flow_bodies(flow_id TEXT NOT NULL,slot TEXT NOT NULL,body BLOB NOT NULL,PRIMARY KEY(flow_id,slot))").unwrap();
  let store=Store::open(&path).unwrap();let flow=sample();store.insert(&flow).unwrap();
  store.0.lock().unwrap().execute("UPDATE flow_bodies SET encoded_sha256=NULL",[]).unwrap();
  store.insert(&flow).unwrap();
  let digest:Vec<u8>=store.0.lock().unwrap().query_row("SELECT encoded_sha256 FROM flow_bodies",[],|row|row.get(0)).unwrap();
  assert_eq!(digest.len(),32);assert_eq!(STANDARD.decode(store.get(&flow.id).unwrap().unwrap().request.body_base64).unwrap(),vec![7u8;100000]);
 }
 #[test]
 fn large_bodies_roundtrip_backup_update_and_delete(){
  let dir=tempfile::tempdir().unwrap();let path=dir.path().join("sessions.db");let store=Store::open(&path).unwrap();let mut f=sample();store.insert(&f).unwrap();
  {let c=store.0.lock().unwrap();let data:String=c.query_row("SELECT data FROM flows",[],|r|r.get(0)).unwrap();assert!(data.len()<2000);let bytes:Vec<u8>=c.query_row("SELECT body FROM flow_bodies",[],|r|r.get(0)).unwrap();assert_eq!(bytes.len(),100000);}
  assert_eq!(store.get(&f.id).unwrap().unwrap().request.body_base64,f.request.body_base64);
  assert_eq!(store.changes(None).unwrap().rows[0].flow.request.body_base64,f.request.body_base64);
  let backup=store.backup().unwrap();let copied=Store::open(Path::new(&backup)).unwrap();assert_eq!(copied.list().unwrap()[0].request.body_base64,f.request.body_base64);
  drop(store);let store=Store::open(&path).unwrap();assert_eq!(store.list().unwrap()[0].request.body_base64,f.request.body_base64);
  f.request.body_base64=STANDARD.encode(b"small").into();store.insert(&f).unwrap();assert_eq!(store.get(&f.id).unwrap().unwrap().request.body_base64,f.request.body_base64);
  assert_eq!(store.0.lock().unwrap().query_row("SELECT count(*) FROM flow_bodies",[],|r|r.get::<_,i64>(0)).unwrap(),0);
  let f=sample();store.insert(&f).unwrap();store.delete_flows(vec![f.id.clone()]).unwrap();store.insert(&f).unwrap();assert!(store.list().unwrap().is_empty());assert_eq!(store.0.lock().unwrap().query_row("SELECT count(*) FROM flow_bodies",[],|r|r.get::<_,i64>(0)).unwrap(),0);
 }
 #[test]
 fn original_and_response_bodies_are_independent_and_missing_body_is_an_error(){
  let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("s.db")).unwrap();let mut f=sample();
  f.original_request=Some(f.request.clone());
  f.response=Some(serde_json::from_value(serde_json::json!({"status":200,"version":"HTTP/1.1","headers":[],"bodyBase64":STANDARD.encode(vec![8u8;100000])})).unwrap());
  f.original_response=f.response.clone();
  let request_ptr=f.request.body_base64.as_ptr();
  let response_ptr=f.response.as_ref().unwrap().body_base64.as_ptr();
  let mut metadata=f.clone();let mut visited=Vec::new();
  body_slots(&mut metadata,|slot,body| {
   let original=match slot {
    "request"|"originalRequest"=>&f.request.body_base64,
    "response"|"originalResponse"=>&f.response.as_ref().unwrap().body_base64,
    _=>panic!("Unexpected body slot: {slot}"),
   };
   // Borrowing a slot must not coerce it to &mut String and copy the body
   // before the visitor even decides whether an edit is needed.
   assert_eq!(body.as_ptr(),original.as_ptr());
   *body=format!("@body:{slot}").into();visited.push(slot.to_owned());Ok(())
  }).unwrap();
  assert_eq!(visited,["request","response","originalRequest","originalResponse"]);
  body_slots(&mut metadata,|slot,body| {assert_eq!(body.as_str(),format!("@body:{slot}"));Ok(())}).unwrap();
  assert_eq!(f.request.body_base64.as_ptr(),request_ptr);
  assert_eq!(f.original_request.as_ref().unwrap().body_base64.as_ptr(),request_ptr);
  assert_eq!(f.response.as_ref().unwrap().body_base64.as_ptr(),response_ptr);
  assert_eq!(f.original_response.as_ref().unwrap().body_base64.as_ptr(),response_ptr);
  assert_eq!(STANDARD.decode(&f.request.body_base64).unwrap(),vec![7;100000]);
  assert_eq!(STANDARD.decode(&f.response.as_ref().unwrap().body_base64).unwrap(),vec![8;100000]);
  store.insert(&f).unwrap();let restored=store.get(&f.id).unwrap().unwrap();
  assert_eq!(serde_json::to_value(restored).unwrap(),serde_json::to_value(&f).unwrap());
  store.0.lock().unwrap().execute("DELETE FROM flow_bodies WHERE slot='response'",[]).unwrap();
  assert!(store.get(&f.id).is_err());assert!(store.list().is_err());
 }
 #[test]
 fn incremental_blob_padding_and_late_failure_rollback(){
  let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("large.db")).unwrap();
  let mut flow=sample();
  for extra in 0..3 {
   let bytes:Vec<u8>=(0..(9*1024*1024+extra)).map(|i|(i%251) as u8).collect();
   flow.request.body_base64=STANDARD.encode(&bytes).into();
   store.insert(&flow).unwrap();
   assert_eq!(store.get(&flow.id).unwrap().unwrap().request.body_base64,flow.request.body_base64);
  }
  // Fail after several megabytes have already reached SQLite, not at the prefix.
  let mut invalid=flow.clone();let len=invalid.request.body_base64.len();
  assert_eq!(invalid.request.body_base64.as_ptr(),flow.request.body_base64.as_ptr());
  invalid.request.body_base64.replace_range(len-8..len-7,"!");
  assert_ne!(invalid.request.body_base64.as_ptr(),flow.request.body_base64.as_ptr());
  assert!(store.insert(&invalid).is_err());
  assert_eq!(store.get(&flow.id).unwrap().unwrap().request.body_base64,flow.request.body_base64);
  assert_eq!(store.0.lock().unwrap().query_row("SELECT count(*) FROM flow_bodies",[],|r|r.get::<_,i64>(0)).unwrap(),1);
 }
 #[test]
 fn imports_and_failed_updates_are_atomic(){
  let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("s.db")).unwrap();let f=sample();store.insert(&f).unwrap();let mut bad=f.clone();bad.request.body_base64="!".repeat(100000).into();assert!(store.insert(&bad).is_err());assert_eq!(store.get(&f.id).unwrap().unwrap().request.body_base64,f.request.body_base64);
  let job=store.begin_import().unwrap();store.append_import(&job,0,vec![f.clone()]).unwrap();assert_eq!(store.finish_import(&job,true).unwrap(),1);store.import_flows(vec![f.clone()]).unwrap();assert_eq!(store.list().unwrap().len(),3);for row in store.list().unwrap(){assert_eq!(row.request.body_base64,f.request.body_base64);}
  store.clear().unwrap();assert_eq!(store.0.lock().unwrap().query_row("SELECT count(*) FROM flow_bodies",[],|r|r.get::<_,i64>(0)).unwrap(),0);
 }
}
