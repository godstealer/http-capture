use super::Store;
use crate::config_migration::{Snapshot,CONFIG_KEYS};
use anyhow::{ensure,Result};
use rusqlite::{params,OptionalExtension,TransactionBehavior};
use serde_json::Value;
use sha2::{Digest,Sha256};
use std::{collections::BTreeMap,sync::atomic::Ordering};

const MAX_SNAPSHOT_BYTES:usize=32*1024*1024;
fn snapshot(conn:&rusqlite::Connection,instance:&str)->Result<Snapshot>{
    let mut values=BTreeMap::new();let mut total=0usize;
    for key in CONFIG_KEYS {
        let length:Option<usize>=conn.query_row("SELECT length(CAST(value AS BLOB)) FROM settings WHERE key=?1",[key],|row|row.get(0)).optional()?;
        total=total.checked_add(length.unwrap_or_default()).ok_or_else(||anyhow::anyhow!("Configuration is too large"))?;
        ensure!(total<=MAX_SNAPSHOT_BYTES,"Saved configuration exceeds the 32 MiB migration snapshot limit");
        let raw:Option<String>=conn.query_row("SELECT value FROM settings WHERE key=?1",[key],|row|row.get(0)).optional()?;
        if let Some(raw)=raw {values.insert((*key).to_owned(),serde_json::from_str::<Value>(&raw)?);}
    }
    let canonical=serde_json::to_vec(&values)?;
    Ok(Snapshot{revision:format!("{instance}:{:x}",Sha256::digest(canonical)),settings:values})
}
impl Store {
    pub(crate) fn configuration_migration_status(&self)->Result<Value>{
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;let tx=conn.transaction()?;
        let raw:Option<String>=tx.query_row("SELECT value FROM settings WHERE key='config-migration.pending.v1'",[],|row|row.get(0)).optional()?;
        let mut result=raw.map(|s|serde_json::from_str::<Value>(&s)).transpose()?.unwrap_or_else(||serde_json::json!({"importedSections":[],"warnings":[]}));
        let epoch:Option<String>=tx.query_row("SELECT value FROM settings WHERE key='config-migration.epoch.v1'",[],|row|row.get(0)).optional()?;
        result["frozen"]=serde_json::json!(self.configuration_frozen()||epoch!=self.3);tx.commit()?;Ok(result)
    }
    pub fn configuration_frozen(&self)->bool{self.2.load(Ordering::Acquire)}
    pub fn configuration_ready(&self)->Result<()>{let conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;self.configuration_ready_in(&conn)}
    pub(super) fn configuration_ready_in(&self,conn:&rusqlite::Connection)->Result<()>{
        ensure!(!self.configuration_frozen(),"Configuration was imported; restart the application before editing settings");
        let epoch:Option<String>=conn.query_row("SELECT value FROM settings WHERE key='config-migration.epoch.v1'",[],|row|row.get(0)).optional()?;
        ensure!(epoch==self.3,"Configuration was imported by another window/process; restart the application before editing settings");Ok(())
    }
    pub(crate) fn configuration_snapshot(&self)->Result<Snapshot>{
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;let tx=conn.transaction()?;
        let result=snapshot(&tx,&self.1)?;tx.commit()?;Ok(result)
    }
    pub(crate) fn commit_configuration(&self,expected:&str,updates:BTreeMap<String,Value>,recovery:&Value,pending:&Value)->Result<()> {
        ensure!(!updates.is_empty(),"Choose at least one configuration category");
        ensure!(updates.keys().all(|key|CONFIG_KEYS.contains(&key.as_str())),"Unsupported configuration key");
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.configuration_ready_in(&tx)?;
        ensure!(snapshot(&tx,&self.1)?.revision==expected,"Configuration changed after preview; preview the file again before importing");
        tx.execute("INSERT INTO settings(key,value) VALUES('config-migration.recovery.v1',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(recovery)?])?;
        for (key,value) in updates {tx.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,serde_json::to_string(&value)?])?;}
        let epoch=pending["epoch"].as_str().ok_or_else(||anyhow::anyhow!("Missing migration epoch"))?;uuid::Uuid::parse_str(epoch)?;
        tx.execute("INSERT INTO settings(key,value) VALUES('config-migration.epoch.v1',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[epoch])?;
        tx.execute("INSERT INTO settings(key,value) VALUES('config-migration.pending.v1',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(pending)?])?;
        tx.commit()?;
        // The database lock is still held: queued UI/MCP configuration writes
        // observe the freeze before they can overwrite the new snapshot.
        self.2.store(true,Ordering::Release);Ok(())
    }
}
