//! Bounded MCP projections. Never deserialize a complete Flow or restore its bodies.
use super::Store;
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

pub struct McpRequestFilter {
    pub url_contains: Option<String>,
    pub method: Option<String>,
    pub status: Option<u16>,
    pub source: Option<String>,
    pub cursor: Option<i64>,
    pub limit: usize,
}
impl Default for McpRequestFilter {
    fn default()->Self {Self{url_contains:None,method:None,status:None,source:None,cursor:None,limit:50}}
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRequestPage { pub requests: Vec<Value>, pub next_cursor: Option<String> }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpBodyChunk {
    pub offset: u64,
    pub total_bytes: u64,
    pub body_base64: String,
    pub text: Option<String>,
    pub next_offset: Option<u64>,
}
impl Store {
    pub fn mcp_list_requests(&self, filter: McpRequestFilter) -> Result<McpRequestPage> {
        ensure!((1..=200).contains(&filter.limit), "limit must be between 1 and 200");
        ensure!(filter.url_contains.as_ref().is_none_or(|s| s.len() <= 2048), "URL filter exceeds 2048 bytes");
        ensure!(filter.method.as_ref().is_none_or(|s| s.len() <= 32), "Method filter exceeds 32 bytes");
        ensure!(filter.source.as_ref().is_none_or(|s| s.len() <= 32), "Source filter exceeds 32 bytes");
        ensure!(filter.cursor.is_none_or(|n| n > 0), "Invalid cursor");
        let conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let mut statement = conn.prepare("SELECT rowid,json_object(
            'id',id,'startedAt',started_at,'durationMs',json_extract(data,'$.durationMs'),
            'source',json_extract(data,'$.source'),'state',json_extract(data,'$.state'),
            'method',substr(json_extract(data,'$.request.method'),1,32),
            'url',substr(json_extract(data,'$.request.url'),1,8192),
            'status',json_extract(data,'$.response.status'),
            'clientProtocol',json_extract(data,'$.clientProtocol'),
            'upstreamProtocol',json_extract(data,'$.response.version')) FROM flows
            WHERE (?1 IS NULL OR rowid<?1)
              AND (?2 IS NULL OR instr(lower(json_extract(data,'$.request.url')),lower(?2))>0)
              AND (?3 IS NULL OR upper(json_extract(data,'$.request.method'))=upper(?3))
              AND (?4 IS NULL OR json_extract(data,'$.response.status')=?4)
              AND (?5 IS NULL OR json_extract(data,'$.source')=?5)
            ORDER BY rowid DESC LIMIT ?6")?;
        let mut rows = statement.query(params![filter.cursor,filter.url_contains,filter.method,filter.status,filter.source,filter.limit+1])?;
        let mut requests = Vec::new();
        let mut last = None;
        let mut more = false;
        while let Some(row) = rows.next()? {
            if requests.len() == filter.limit { more = true; break; }
            last = Some(row.get::<_,i64>(0)?);
            requests.push(serde_json::from_str(&row.get::<_,String>(1)?)?);
        }
        Ok(McpRequestPage { requests, next_cursor: if more { last.map(|v|v.to_string()) } else { None } })
    }
    pub fn mcp_get_request(&self, id: &str) -> Result<Option<Value>> {
        ensure!(!id.is_empty() && id.len() <= 128, "Invalid request ID");
        let conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        // Explicit projection excludes body, raw heads, scripts, credentials and certificates.
        let value: Option<String> = conn.query_row("SELECT json_object(
            'id',id,'parentId',json_extract(data,'$.parentId'),'startedAt',started_at,
            'durationMs',json_extract(data,'$.durationMs'),'source',json_extract(data,'$.source'),
            'state',json_extract(data,'$.state'),'clientProtocol',json_extract(data,'$.clientProtocol'),
            'request',json_object('method',substr(json_extract(data,'$.request.method'),1,32),
                'url',substr(json_extract(data,'$.request.url'),1,8192),'engine',json_extract(data,'$.request.engine'),
                'headers',json((SELECT json_group_array(json_object('name',substr(json_extract(value,'$.name'),1,256),'value',substr(json_extract(value,'$.value'),1,512),'disabled',json_extract(value,'$.disabled'))) FROM (SELECT value FROM json_each(flows.data,'$.request.headers') LIMIT 81)))),
            'response',json(CASE WHEN json_type(data,'$.response')='object' THEN json_object(
                'status',json_extract(data,'$.response.status'),'version',json_extract(data,'$.response.version'),
                'headers',json((SELECT json_group_array(json_object('name',substr(json_extract(value,'$.name'),1,256),'value',substr(json_extract(value,'$.value'),1,512),'disabled',json_extract(value,'$.disabled'))) FROM (SELECT value FROM json_each(flows.data,'$.response.headers') LIMIT 81))),
                'trailers',json((SELECT json_group_array(json_object('name',substr(json_extract(value,'$.name'),1,256),'value',substr(json_extract(value,'$.value'),1,512),'disabled',json_extract(value,'$.disabled'))) FROM (SELECT value FROM json_each(flows.data,'$.response.trailers') LIMIT 81))))
                ELSE 'null' END)) FROM flows WHERE id=?1", [id], |row|row.get(0)).optional()?;
        value.map(|raw|serde_json::from_str(&raw).map_err(Into::into)).transpose()
    }
    pub fn mcp_read_body(&self, id: &str, direction: &str, offset: u64, length: usize) -> Result<McpBodyChunk> {
        ensure!(!id.is_empty() && id.len() <= 128, "Invalid request ID");
        ensure!((1..=65536).contains(&length), "length must be between 1 and 65536 bytes");
        ensure!(offset <= i64::MAX as u64, "Invalid byte offset");
        let path = match direction { "request"=>"$.request.bodyBase64", "response"=>"$.response.bodyBase64", _=>anyhow::bail!("direction must be request or response") };
        let mut conn = self.0.lock().map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        let tx = conn.transaction()?;
        let (kind, encoded_len, ending, marker): (Option<String>,Option<usize>,Option<String>,Option<String>) = tx.query_row(
            "SELECT json_type(data,?2),length(json_extract(data,?2)),substr(json_extract(data,?2),-2),
             CASE WHEN substr(json_extract(data,?2),1,6)='@body:' THEN json_extract(data,?2) END FROM flows WHERE id=?1",
            params![id,path], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?.context("Request not found")?;
        ensure!(kind.as_deref()==Some("text"), "Requested body is unavailable");
        let (total, bytes) = if let Some(marker) = marker {
            ensure!(marker==format!("@body:{direction}"), "Invalid stored body reference");
            let rowid:i64=tx.query_row("SELECT rowid FROM flow_bodies WHERE flow_id=?1 AND slot=?2",params![id,direction],|r|r.get(0))?;
            let blob=tx.blob_open(rusqlite::DatabaseName::Main,"flow_bodies","body",rowid,true)?;
            let total=blob.len() as u64;
            let count=total.saturating_sub(offset).min(length as u64) as usize;
            let mut bytes=vec![0;count];
            if count>0 {blob.read_at_exact(&mut bytes,offset as usize)?;}
            (total,bytes)
        } else {
            let encoded_len=encoded_len.unwrap_or(0);
            ensure!(encoded_len%4==0,"Invalid Base64 body length");
            let padding=ending.unwrap_or_default().bytes().rev().take_while(|&b|b==b'=').count();
            let total=(encoded_len/4*3).checked_sub(padding).context("Invalid Base64 padding")? as u64;
            let count=total.saturating_sub(offset).min(length as u64) as usize;
            let bytes=if count==0 {Vec::new()} else {
                let skip=(offset%3) as usize;
                let start=offset/3*4+1;
                let encoded_count=(skip+count).div_ceil(3)*4;
                let part:String=tx.query_row("SELECT substr(json_extract(data,?2),?3,?4) FROM flows WHERE id=?1",params![id,path,start,encoded_count],|r|r.get(0))?;
                let decoded=STANDARD.decode(part)?;
                ensure!(decoded.len()>=skip+count,"Incomplete stored body");
                decoded[skip..skip+count].to_vec()
            };
            (total,bytes)
        };
        tx.commit()?;
        let end=offset.saturating_add(bytes.len() as u64);
        Ok(McpBodyChunk{offset,total_bytes:total,body_base64:STANDARD.encode(&bytes),text:String::from_utf8(bytes).ok(),next_offset:if end<total {Some(end)} else {None}})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Flow;
    use serde_json::json;
    fn flow(id:&str,body:&[u8])->Flow {serde_json::from_value(json!({"id":id,"parentId":null,"startedAt":1,"durationMs":1,"source":"capture","state":"completed","request":{"method":"POST","url":"https://example.invalid/?token=secret","headers":[],"bodyBase64":STANDARD.encode(body)},"response":null,"error":null,"notes":[]})).unwrap()}
    #[test]
    fn projections_do_not_restore_or_validate_body_payloads() {
        let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("test.db")).unwrap();
        store.insert(&flow("large",&vec![7;100000])).unwrap();
        store.0.lock().unwrap().execute("DELETE FROM flow_bodies",[]).unwrap();
        let page=store.mcp_list_requests(McpRequestFilter{limit:10,..Default::default()}).unwrap();
        assert_eq!(page.requests.len(),1);assert!(!serde_json::to_string(&page).unwrap().contains("@body:"));
        let detail=store.mcp_get_request("large").unwrap().unwrap();assert!(detail["request"].get("bodyBase64").is_none());
        assert!(store.mcp_read_body("large","request",0,16).is_err());
    }
    #[test]
    fn inline_and_sqlite_blob_chunks_preserve_exact_bytes_at_unaligned_offsets() {
        let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("test.db")).unwrap();
        for size in [0,1,2,3,40,70001] {
            let bytes:Vec<u8>=(0..size).map(|i|(i%251) as u8).collect();let id=format!("body-{size}");store.insert(&flow(&id,&bytes)).unwrap();
            for offset in [0,1,2,3,17,65535,size,size+1] {
                let chunk=store.mcp_read_body(&id,"request",offset as u64,65536).unwrap();
                let start=offset.min(size);assert_eq!(STANDARD.decode(chunk.body_base64).unwrap(),bytes[start..(start+65536).min(size)]);assert_eq!(chunk.total_bytes,size as u64);
            }
        }
        assert!(store.mcp_read_body("body-1","request",0,65537).is_err());
    }
    #[test]
    fn summary_cursor_and_filters_are_applied_before_the_limit() {
        let dir=tempfile::tempdir().unwrap();let store=Store::open(&dir.path().join("test.db")).unwrap();
        for id in ["one","two","three"] {store.insert(&flow(id,b"x")).unwrap();}
        let first=store.mcp_list_requests(McpRequestFilter{method:Some("post".into()),url_contains:Some("example.invalid".into()),limit:2,..Default::default()}).unwrap();
        assert_eq!(first.requests.iter().map(|v|v["id"].as_str().unwrap()).collect::<Vec<_>>(),vec!["three","two"]);
        let second=store.mcp_list_requests(McpRequestFilter{cursor:Some(first.next_cursor.unwrap().parse().unwrap()),limit:2,..Default::default()}).unwrap();
        assert_eq!(second.requests[0]["id"],"one");assert!(second.next_cursor.is_none());
    }
}
