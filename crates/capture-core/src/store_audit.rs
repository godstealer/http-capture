//! Storage-only implementation; include as a child of `store` for connection access.
use super::Store;
use crate::audit::{AuditCompletion, AuditEntry, AuditOutcome, AuditPage, AuditQuery, AuditStart, RETENTION_LIMIT};
use anyhow::{ensure, Result};
use rusqlite::{params, TransactionBehavior};

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis().min(i64::MAX as u128) as i64
}

impl Store {
    /// Persist a receipt before running one authenticated MCP mutation. If this
    /// fails the caller must not perform the operation. Audit writes intentionally
    /// remain possible while configuration commands are frozen, so rejections
    /// during shutdown/import can still be recorded without changing settings.
    pub fn begin_mcp_audit(&self, event: &AuditStart) -> Result<i64> {
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO mcp_operation_audit(started_at,actor,entry_point,client_family,client_ref,operation,target_kind,target_id,outcome)
            VALUES(?1,'mcpTokenHolder','mcpHttp',?2,?3,?4,?5,?6,'pending')",params![now_ms(),event.client.family.as_str(),event.client.reference,event.operation.as_str(),event.operation.target_kind(),event.target_id])?;
        let id=tx.last_insert_rowid();
        tx.execute("DELETE FROM mcp_operation_audit WHERE id IN (SELECT id FROM mcp_operation_audit ORDER BY id DESC LIMIT -1 OFFSET ?1)",[RETENTION_LIMIT as i64])?;
        tx.commit()?;Ok(id)
    }
    /// Update the same receipt once. A failure does not undo the already executed
    /// operation: preserve its result and expose an audit warning rather than
    /// returning a new operation error that could cause an unsafe retry.
    pub fn finish_mcp_audit(&self, id: i64, completion: &AuditCompletion) -> Result<()> {
        ensure!(id>0 && completion.outcome!=AuditOutcome::Pending, "Invalid audit completion");
        ensure!(completion.outcome!=AuditOutcome::Succeeded || completion.reason.is_none(), "Successful audit completion cannot have a failure reason");
        let conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let changed=conn.execute("UPDATE mcp_operation_audit SET finished_at=max(started_at,?1),outcome=?2,reason=?3,target_id=coalesce(?4,target_id) WHERE id=?5 AND outcome='pending'",params![now_ms(),completion.outcome.as_str(),completion.reason.map(|reason|reason.as_str()),completion.target_id,id])?;
        ensure!(changed==1, "Audit receipt is missing or already completed");Ok(())
    }
    pub fn mcp_audit_page(&self, query: AuditQuery) -> Result<AuditPage> {
        let (cursor,limit)=query.checked()?;
        let mut conn=self.0.lock().map_err(|_|anyhow::anyhow!("Database lock poisoned"))?;
        let tx=conn.transaction()?;
        let retained_count=tx.query_row("SELECT count(*) FROM mcp_operation_audit",[],|row|row.get::<_,usize>(0))?;
        let mut entries={
            let mut statement=tx.prepare("SELECT id,started_at,finished_at,actor,entry_point,client_family,client_ref,operation,target_kind,target_id,outcome,reason FROM mcp_operation_audit WHERE (?1 IS NULL OR id<?1) ORDER BY id DESC LIMIT ?2")?;
            let rows=statement.query_map(params![cursor,(limit+1) as i64],|row| Ok(AuditEntry {
                id:row.get::<_,i64>(0)?.to_string(),started_at:row.get(1)?,finished_at:row.get(2)?,actor:row.get(3)?,entry_point:row.get(4)?,client_family:row.get(5)?,client_ref:row.get(6)?,operation:row.get(7)?,target_kind:row.get(8)?,target_id:row.get(9)?,outcome:row.get(10)?,reason:row.get(11)?,
            }))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let next_cursor=if entries.len()>limit {entries.truncate(limit);entries.last().map(|entry|entry.id.clone())} else {None};
        tx.commit()?;Ok(AuditPage {entries,next_cursor,retained_count,retention_limit:RETENTION_LIMIT})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{self, AuditClient, AuditOperation, AuditReason};
    use std::sync::Arc;
    fn store() -> (tempfile::TempDir, Store) {
        let directory=tempfile::tempdir().unwrap();
        let store=Store::open(&directory.path().join("audit.db")).unwrap();
        audit::initialize(&store.0.lock().unwrap()).unwrap();(directory,store)
    }
    fn start() -> AuditStart {AuditStart::new(AuditOperation::UpdateRequest,AuditClient::unknown())}
    fn page(store:&Store) -> AuditPage {store.mcp_audit_page(AuditQuery::default()).unwrap()}
    #[test]
    fn receipts_persist_with_safe_source_and_no_client_or_argument_text() {
        let (directory,store)=store();
        let credential="Authorization Bearer must-not-be-saved https://secret.test/body=private-script";
        let client=AuditClient::from_reported(&format!("Codex {credential}"),credential);
        let id=store.begin_mcp_audit(&AuditStart::new(AuditOperation::UpdateRequest,client).with_target_id(Some(credential))).unwrap();
        let first=page(&store);
        assert_eq!(first.entries[0].actor,"mcpTokenHolder");assert_eq!(first.entries[0].entry_point,"mcpHttp");
        assert_eq!(first.entries[0].client_family,"codex");assert_eq!(first.entries[0].client_ref.as_ref().unwrap().len(),64);
        assert!(first.entries[0].target_id.is_none());assert_eq!(first.entries[0].outcome,"pending");
        assert!(!serde_json::to_string(&first).unwrap().contains("must-not-be-saved"));
        let target=uuid::Uuid::new_v4().to_string();
        store.finish_mcp_audit(id,&AuditCompletion::new(AuditOutcome::Failed).with_reason(AuditReason::Conflict).with_target_id(Some(&target))).unwrap();
        drop(store);let reopened=Store::open(&directory.path().join("audit.db")).unwrap();let entry=&page(&reopened).entries[0];
        assert_eq!(entry.outcome,"failed");assert_eq!(entry.reason.as_deref(),Some("conflict"));assert_eq!(entry.target_id.as_deref(),Some(target.as_str()));
        assert!(entry.finished_at.unwrap()>=entry.started_at);
        let bytes=std::fs::read(directory.path().join("audit.db")).unwrap();assert!(!bytes.windows(17).any(|part|part==b"must-not-be-saved"));
    }
    #[test]
    fn cursor_pages_are_bounded_unique_and_stable_when_new_entries_arrive() {
        let (_directory,store)=store();for _ in 0..5 {store.begin_mcp_audit(&start()).unwrap();}
        let first=store.mcp_audit_page(AuditQuery{cursor:None,limit:Some(2)}).unwrap();
        assert_eq!(first.entries.iter().map(|entry|entry.id.as_str()).collect::<Vec<_>>(),["5","4"]);
        store.begin_mcp_audit(&start()).unwrap();
        let second=store.mcp_audit_page(AuditQuery{cursor:first.next_cursor,limit:Some(2)}).unwrap();
        assert_eq!(second.entries.iter().map(|entry|entry.id.as_str()).collect::<Vec<_>>(),["3","2"]);
        let last=store.mcp_audit_page(AuditQuery{cursor:second.next_cursor,limit:Some(2)}).unwrap();assert_eq!(last.entries[0].id,"1");assert!(last.next_cursor.is_none());
        for cursor in ["", "0", "-1", "secret", "9223372036854775808"] {assert!(store.mcp_audit_page(AuditQuery{cursor:Some(cursor.into()),limit:None}).is_err());}
        for limit in [0,101,usize::MAX] {assert!(store.mcp_audit_page(AuditQuery{cursor:None,limit:Some(limit)}).is_err());}
        assert!(serde_json::from_value::<AuditQuery>(serde_json::json!({"body":"secret"})).is_err());
    }
    #[test]
    fn completion_failure_keeps_unknown_receipt_and_does_not_claim_rollback() {
        let (_directory,store)=store();let id=store.begin_mcp_audit(&start()).unwrap();
        store.0.lock().unwrap().execute_batch("CREATE TRIGGER fail_audit_update BEFORE UPDATE ON mcp_operation_audit BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(store.finish_mcp_audit(id,&AuditCompletion::new(AuditOutcome::Succeeded)).is_err());
        let entry=&page(&store).entries[0];assert_eq!(entry.outcome,"pending");assert!(entry.finished_at.is_none());
        store.0.lock().unwrap().execute_batch("DROP TRIGGER fail_audit_update;").unwrap();
        store.finish_mcp_audit(id,&AuditCompletion::new(AuditOutcome::Rejected).with_reason(AuditReason::PermissionDenied)).unwrap();
        assert!(store.finish_mcp_audit(id,&AuditCompletion::new(AuditOutcome::Succeeded)).is_err());
        assert_eq!(page(&store).entries[0].outcome,"rejected");
        assert!(store.finish_mcp_audit(id,&AuditCompletion::new(AuditOutcome::Pending)).is_err());
    }
    #[test]
    fn begin_failure_does_not_issue_a_receipt_or_partially_trim_existing_rows() {
        let (_directory,store)=store();store.begin_mcp_audit(&start()).unwrap();
        store.0.lock().unwrap().execute_batch("CREATE TRIGGER fail_audit_insert BEFORE INSERT ON mcp_operation_audit BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(store.begin_mcp_audit(&start()).is_err());assert_eq!(page(&store).retained_count,1);
    }
    #[test]
    fn concurrent_receipts_complete_independently_and_retention_is_bounded() {
        let (_directory,store)=store();let store=Arc::new(store);
        std::thread::scope(|scope| {for _ in 0..8 {let store=store.clone();scope.spawn(move || {for _ in 0..16 {let id=store.begin_mcp_audit(&start()).unwrap();store.finish_mcp_audit(id,&AuditCompletion::new(AuditOutcome::Succeeded)).unwrap();}});}});
        assert_eq!(page(&store).retained_count,128);assert!(page(&store).entries.iter().all(|entry|entry.outcome=="succeeded"));
        // Fill the table efficiently, then exercise the real transactional trim.
        store.0.lock().unwrap().execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<10000) INSERT INTO mcp_operation_audit(started_at,actor,entry_point,client_family,operation,target_kind,outcome) SELECT 0,'mcpTokenHolder','mcpHttp','unknown','stop_capture','listener','succeeded' FROM n;").unwrap();
        let newest=store.begin_mcp_audit(&start()).unwrap();let result=page(&store);
        assert_eq!(result.retained_count,RETENTION_LIMIT);assert_eq!(result.retention_limit,RETENTION_LIMIT);assert_eq!(result.entries[0].id,newest.to_string());
    }
    #[test]
    fn only_known_writes_are_audited_and_target_ids_are_canonical() {
        assert!(AuditOperation::from_tool_name("read_body").is_none());assert!(AuditOperation::from_tool_name("get_global_scripts").is_none());assert!(AuditOperation::from_tool_name("secret-script-source").is_none());
        assert!(AuditOperation::from_tool_name("set_global_scripts_enabled").is_some());
        assert_eq!(AuditClient::from_reported("", "secret").reference,None);
        assert_eq!(AuditClient::from_reported("Cursor", "1").reference,AuditClient::from_reported("Cursor", "1").reference);
        let (directory,store)=store();let uuid=uuid::Uuid::new_v4();let id=store.begin_mcp_audit(&start().with_target_id(Some(&uuid.simple().to_string()))).unwrap();
        assert_eq!(page(&store).entries[0].target_id,Some(uuid.to_string()));
        // Read access and ledger finalization survive configuration freeze;
        // they neither mutate settings nor re-enable network commands.
        store.2.store(true,std::sync::atomic::Ordering::Release);
        store.finish_mcp_audit(id,&AuditCompletion::new(AuditOutcome::Cancelled).with_reason(AuditReason::Cancelled)).unwrap();
        assert_eq!(page(&store).entries[0].outcome,"cancelled");assert!(store.configuration_ready().is_err());drop(store);drop(directory);
    }
}
