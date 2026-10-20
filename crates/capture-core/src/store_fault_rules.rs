use super::Store;
use crate::fault_rules::{Config,compile,version};
use anyhow::{ensure,Result};
use rusqlite::{OptionalExtension,TransactionBehavior};

impl Store {
    pub(crate) fn save_fault_rules(&self,config:&Config,expected:&str)->Result<()> {
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.configuration_ready_in(&tx)?;
        let raw:Option<String>=tx.query_row("SELECT value FROM settings WHERE key='fault-rules.v1'",[],|row|row.get(0)).optional()?;
        let mut previous=raw.map(|raw|serde_json::from_str::<Config>(&raw)).transpose()?.unwrap_or_default();
        compile(&mut previous)?;
        ensure!(version(&previous)==expected,"模拟配置已被其他内核更改，请重启内核后重新加载");
        tx.execute("INSERT INTO settings(key,value) VALUES('fault-rules.v1',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(config)?])?;
        tx.commit()?;Ok(())
    }
}
