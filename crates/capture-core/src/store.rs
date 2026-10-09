use crate::model::{Flow, RequestDraft};
use serde::{Serialize, Deserialize};
use rusqlite::OptionalExtension;
use anyhow::Result;
use rusqlite::{params, Connection};
use std::{path::Path, sync::Mutex};

pub struct Store(Mutex<Connection>);

impl Store {
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
        Ok(Self(Mutex::new(conn)))
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
impl Store {
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
