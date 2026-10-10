use crate::model::{Flow, RequestDraft};
use serde::{Serialize, Deserialize};
use rusqlite::OptionalExtension;
use anyhow::Result;
use rusqlite::{params, Connection};
use std::{path::Path, sync::Mutex};

#[derive(Serialize,Deserialize)]
pub struct FlowChange {pub position:i64,pub flow:Flow}
#[derive(Serialize,Deserialize)]
pub struct FlowChanges {pub revision:String,pub reset:bool,pub rows:Vec<FlowChange>,pub deleted:Vec<String>}
pub struct Store(Mutex<Connection>, String);

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
        self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?.execute_batch("VACUUM; UPDATE flows SET started_at=started_at;")?;
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
            tx.execute("INSERT INTO flows(id,started_at,data) SELECT json_extract(data,'$.id'),json_extract(data,'$.startedAt'),data FROM import_rows WHERE job=?1 ORDER BY sequence DESC", [id])?
        } else { 0 };
        tx.execute("DELETE FROM import_rows WHERE job=?1", [id])?;
        tx.execute("DELETE FROM import_jobs WHERE id=?1", [id])?;
        tx.commit()?; Ok(count)
    }
    pub fn get(&self, id: &str) -> Result<Option<Flow>> {
        let raw: Option<String> = self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?.query_row("SELECT data FROM flows WHERE id=?1", [id], |r| r.get(0)).optional()?;
        raw.map(|s| serde_json::from_str(&s).map_err(Into::into)).transpose()
    }
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
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
        Ok(Self(Mutex::new(conn), uuid::Uuid::new_v4().to_string()))
    }
    pub fn insert(&self, flow: &Flow) -> Result<()> {
        self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?
            .execute("INSERT INTO flows(id, started_at, data) SELECT ?1, ?2, ?3 WHERE NOT EXISTS (SELECT 1 FROM deleted_flows WHERE id=?1)
                ON CONFLICT(id) DO UPDATE SET data=excluded.data", 
                params![flow.id, flow.started_at, serde_json::to_string(flow)?])?;
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
            flow.id = uuid::Uuid::new_v4().to_string(); flow.parent_id = None; flow.source = "capture".into();
            flow.request.scripts = Default::default(); flow.request.upstream_profile_id = None;
            if let Some(r) = &mut flow.original_request { r.scripts = Default::default(); r.upstream_profile_id = None; }
            flow.notes.push("从文件导入；不是本机实时捕获。脚本和上游代理关联已清除。".into());
            tx.execute("INSERT INTO flows(id,started_at,data) VALUES(?1,?2,?3)",params![flow.id,flow.started_at,serde_json::to_string(flow)?])?;
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
            let mut cursor=if reset{stmt.query([])?}else{stmt.query([previous.unwrap()])?};let mut rows=Vec::new();while let Some(row)=cursor.next()?{rows.push(FlowChange{position:row.get(0)?,flow:serde_json::from_str(&row.get::<_,String>(1)?)?});}rows};
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
        let conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let mut stmt = conn.prepare("SELECT data FROM flows ORDER BY rowid DESC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
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
}
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
pub struct SavedRequest {pub id:String,pub name:String,pub collection_id:String,pub draft:RequestDraft}
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
  anyhow::ensure!(serde_json::to_vec(&value)?.len()<=8*1024*1024,"Library exceeds 8 MiB");
  let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
  let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
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
  self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,raw])?;Ok(())
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
  anyhow::ensure!(serde_json::to_vec(&value)?.len()<=8*1024*1024,"请求工作区超过 8 MiB，未保存");
  let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
  let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
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
  let mut library=Library::default();library.collections.push(Collection{id:"c".into(),name:"Saved".into()});library.requests.push(SavedRequest{id:"r".into(),name:"Request".into(),collection_id:"c".into(),draft:flow.request});
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
