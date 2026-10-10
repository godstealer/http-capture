//! HTTP/1 Upgrade capture. Relay frame bytes unchanged, record unmasked payloads.
use crate::{http1::*, model::*, Engine};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Serialize, Deserialize};
use sha1::{Sha1, Digest};
use std::{sync::Arc, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
use tokio::{io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader}, sync::mpsc};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub struct Frame { pub direction: String, pub at_ms: u64, pub opcode: u8, pub fin: bool, pub compressed: bool, pub payload_base64: String }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session { pub state: String, pub frames: Vec<Frame>, #[serde(default)] pub messages:Vec<crate::websocket_messages::Message> }
pub(crate) fn token_header(headers: &[Header], name: &str, token: &str) -> bool {
    values(headers, name).flat_map(|v|v.split(',')).any(|v|v.trim().eq_ignore_ascii_case(token))
}
pub fn requested(headers: &[Header]) -> bool { token_header(headers, "upgrade", "websocket") }
pub fn accept(key: &str) -> String { STANDARD.encode(Sha1::digest(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes())) }
fn single<'a>(headers: &'a [Header], name: &str) -> Result<&'a str> {
    let mut entries = headers.iter().filter(|h|h.name.eq_ignore_ascii_case(name));
    let value = entries.next().with_context(||format!("Missing {name}"))?;
    ensure!(entries.next().is_none(), "Duplicate {name}"); Ok(value.value.trim())
}

fn check_handshake_rules(flow: &Flow, config: &crate::intercept::Config, script: &crate::scripts::Scripts, response: bool) -> Result<()> {
    if !crate::intercept::matches_rules(&config.rules,flow) { return Ok(()); }
    let stage = if response { "响应" } else { "请求" };
    let code = if response { &script.after } else { &script.before };
    ensure!(!script.enabled || code.trim().is_empty(), "此 WebSocket 握手命中了{stage}脚本规则；当前尚不支持握手脚本，请调整匹配条件或停用该阶段脚本后重试");
    let enabled = if response { config.response } else { config.request };
    ensure!(!enabled || !(config.scope == "all" || config.scope == flow.source), "此 WebSocket 握手命中了{stage}拦截规则；当前尚不支持握手拦截，请调整该规则后重试");
    Ok(())
}

struct Guard<'a> { engine: &'a Engine, flow: Flow, done: bool }
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.flow.error = Some("WebSocket stopped: proxy shutdown or connection timeout".into());
            if let Some(ws) = &mut self.flow.websocket { ws.state = "stopped".into(); }
            let _ = self.engine.store.insert(&self.flow); let _ = self.engine.events.send(self.flow.clone());
        }
    }
}

pub async fn capture<S: AsyncRead + AsyncWrite + Unpin>(client: &mut BufReader<S>, request: RequestDraft, raw_head: &[u8], engine: &Engine, client_tls: Option<TlsDetails>) -> Result<()> {
    run(client,request,raw_head,engine,client_tls,"capture",None).await
}

pub(crate) async fn run<S: AsyncRead + AsyncWrite + Unpin>(client: &mut BufReader<S>, request: RequestDraft, raw_head: &[u8], engine: &Engine, client_tls: Option<TlsDetails>, source:&str, execution_id:Option<String>) -> Result<()> {
    let mut execution = engine.executions.begin(match execution_id {Some(id)=>id,None=>engine.executions.prepare()?})?;
    let started = Instant::now();
    let mut guard = Guard { engine, done: false, flow: Flow {
        websocket: Some(Session { state: "connecting".into(), frames: vec![], messages:vec![] }), id: execution.id.clone(),
        started_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64, duration_ms: 0, parent_id: None,
        source: source.into(), request, response: None, error: None, client_protocol: Some("HTTP/1.1".into()), client_tls,
        original_request: None, original_response: None, raw_request_head_base64: if raw_head.is_empty(){None}else{Some(STANDARD.encode(raw_head))},
        notes: vec!["WebSocket 帧原样转发；正文列表为去掩码帧载荷。消息视图重组并尝试解压；单连接最多 300 秒、8 MiB 载荷或 10000 帧。".into()],
    }};
    if source=="replay" {guard.flow.notes.push("主动 WebSocket：原生 HTTP/1 Upgrade，不协商压缩；握手专用字段自动生成。消息发送采用新随机掩码，历史记录不被修改。".into());}
    engine.store.insert(&guard.flow)?;
    let mut upgraded = false;
    let operation = async {
        let config = engine.intercept.snapshot().config;
        let (script, _) = if source=="capture" {engine.scripts.snapshot()} else {(guard.flow.request.scripts.clone(),0)};
        check_handshake_rules(&guard.flow, &config, &script, false)?;
        let request = &guard.flow.request;
        ensure!(request.method == "GET" && token_header(&request.headers, "connection", "upgrade"), "Invalid WebSocket upgrade request");
        ensure!(single(&request.headers, "sec-websocket-version")? == "13", "WebSocket version must be 13");
        let key = single(&request.headers, "sec-websocket-key")?.to_owned();
        ensure!(STANDARD.decode(&key)?.len() == 16, "Invalid WebSocket key");
        ensure!(values(&request.headers, "transfer-encoding").next().is_none() && values(&request.headers, "content-length").all(|v|v == "0"), "WebSocket handshake must not have a body");
        let mut transport_url=url::Url::parse(&request.url)?;
        if transport_url.scheme()=="ws" {transport_url.set_scheme("http").map_err(|_|anyhow::anyhow!("Invalid URL"))?;}
        else if transport_url.scheme()=="wss" {transport_url.set_scheme("https").map_err(|_|anyhow::anyhow!("Invalid URL"))?;}
        let url = validate_url(transport_url.as_str())?;
        let route = if source=="capture" {engine.upstream.snapshot()} else {request.upstream_profile_id.as_deref().map(|id|engine.upstream.profile_snapshot(id)).transpose()?};
        let connection = crate::upstream::connect(&url, route.as_ref()).await?;
        let mut tls_details = None;
        let stream: crate::replay::Stream = if url.scheme() == "https" {
            let config = crate::tls_config::config(&request.tls, &engine.upstream_roots, "native")?;
            let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(hostname(&url).try_into()?, connection.stream).await?;
            let mut details = crate::tls_details::negotiated(tls.get_ref().1); details.server_name = Some(hostname(&url)); tls_details = Some(details);
            Box::new(tls)
        } else { connection.stream };
        let mut server = BufReader::new(stream);
        let mut headers: Vec<_> = request.headers.iter().filter(|h| !h.name.eq_ignore_ascii_case("proxy-authorization") && !h.name.eq_ignore_ascii_case("proxy-connection")).cloned().collect();
        ensure!(values(&headers, "host").count() <= 1, "Duplicate Host");
        if let Some(host) = headers.iter_mut().find(|h|h.name.eq_ignore_ascii_case("host")) { host.value = authority(&url); }
        else { headers.insert(0, Header { name: "Host".into(), value: authority(&url) }); }
        let target = if connection.absolute_form { url.as_str() } else { &url[url::Position::BeforePath..url::Position::AfterQuery] };
        let mut out = format!("GET {target} HTTP/1.1\r\n").into_bytes();
        if connection.absolute_form { if let Some(auth) = connection.authorization { out.extend_from_slice(format!("Proxy-Authorization: {auth}\r\n").as_bytes()); } }
        for h in &headers { out.extend_from_slice(h.name.as_bytes()); out.extend_from_slice(b": "); out.extend(header_bytes(&h.value)?); out.extend_from_slice(b"\r\n"); }
        out.extend_from_slice(b"\r\n"); server.write_all(&out).await?; server.flush().await?;
        let head = read_head(&mut server).await?;
        let (status, version, fields) = parse_response(&head)?;
        guard.flow.response = Some(CapturedResponse { status, version, tls_version: tls_details.as_ref().and_then(|d|d.version.clone()), upstream_tls: tls_details, headers: fields.clone(), body_base64: String::new(), raw_head_base64: Some(STANDARD.encode(&head)), sent_request_headers: Some(headers) });
        check_handshake_rules(&guard.flow, &config, &script, true)?;
        ensure!(status == 101, "WebSocket upgrade rejected by server: HTTP {status}");
        ensure!(token_header(&fields, "connection", "upgrade") && requested(&fields) && single(&fields, "sec-websocket-accept")? == accept(&key), "Invalid WebSocket upgrade response");
        if let Some(protocol) = values(&fields, "sec-websocket-protocol").next() {
            ensure!(single(&fields, "sec-websocket-protocol")? == protocol && values(&request.headers, "sec-websocket-protocol").flat_map(|v|v.split(',')).any(|v|v.trim() == protocol), "Unrequested WebSocket subprotocol");
        }
        // Only permessage-deflate is accepted; compressed payloads are relayed without interpreting them.
        let mut compressed = false;
        for extension in values(&fields, "sec-websocket-extensions").flat_map(|v|v.split(',')) {
            ensure!(extension.split(';').next().unwrap_or("").trim() == "permessage-deflate" && values(&request.headers, "sec-websocket-extensions").flat_map(|v|v.split(',')).any(|v|v.split(';').next().unwrap_or("").trim() == "permessage-deflate"), "Unsupported WebSocket extension"); compressed = true;
        }
        client.write_all(&head).await?; client.flush().await?; upgraded = true;
        guard.flow.websocket.as_mut().unwrap().state = "open".into(); engine.store.insert(&guard.flow)?; let _ = engine.events.send(guard.flow.clone());
        let (client_read, server_write) = tokio::io::split(client);
        let (server_read, client_write) = tokio::io::split(server);
        let (tx, mut rx) = mpsc::channel(8);
        let left = relay(client_read, client_write, true, compressed, started, tx.clone());
        let right = relay(server_read, server_write, false, compressed, started, tx);
        let relays = async { tokio::try_join!(left, right)?; Ok::<_, anyhow::Error>(()) }; tokio::pin!(relays);
        let mut assembler=crate::websocket_messages::Assembler::new(&fields);
        let mut count = 0usize; let mut dirty = false; let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut completed: Option<Result<()>> = None;
        loop {
            if completed.is_some() && rx.is_empty() { completed.take().unwrap()?; break; }
            tokio::select! {
            biased;
            Some(frame) = rx.recv() => {
                count += frame.payload_base64.len();
                let ws = guard.flow.websocket.as_mut().unwrap();
                ensure!(count <= MAX_BODY * 4 / 3 + 4 && ws.frames.len() < 10000, "WebSocket capture limit reached");
                if let Some(message)=assembler.push(&frame){ws.messages.push(message);}
                ws.frames.push(frame); dirty = true;
            }
            result = &mut relays, if completed.is_none() => { completed = Some(result); }
            _ = tick.tick(), if dirty => { guard.flow.duration_ms = started.elapsed().as_millis() as u64; engine.store.insert(&guard.flow)?; let _ = engine.events.send(guard.flow.clone()); dirty = false; }
        }}
        Ok::<_, anyhow::Error>(())
    };
    let result = tokio::select! {
        _ = execution.cancelled() => Err(anyhow::anyhow!("WebSocket stopped by user")),
        result = tokio::time::timeout(Duration::from_secs(300), operation) => result.unwrap_or_else(|_|Err(anyhow::anyhow!("WebSocket session reached 300 second limit"))),
    };
    guard.flow.websocket.as_mut().unwrap().state = if result.is_ok() { "closed" } else { "stopped" }.into();
    guard.flow.error = result.as_ref().err().map(|e|format!("{e:#}")); guard.flow.duration_ms = started.elapsed().as_millis() as u64;
    engine.store.insert(&guard.flow)?; let _ = engine.events.send(guard.flow.clone()); guard.done = true;
    // Never inject an HTTP error response into an upgraded WebSocket connection.
    if upgraded { Ok(()) } else { result }
}

pub(crate) async fn relay<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(mut reader: R, mut writer: W, masked: bool, extensions: bool, started: Instant, tx: mpsc::Sender<Frame>) -> Result<()> {
    let mut fragmented = false; let mut compressed_message = false;
    loop {
        let mut head = [0; 2]; reader.read_exact(&mut head).await.context("WebSocket disconnected without Close frame")?;
        let fin = head[0] & 128 != 0; let opcode = head[0] & 15; let rsv1 = head[0] & 64 != 0;
        ensure!(head[0] & 48 == 0 && (extensions || !rsv1), "Invalid WebSocket reserved bits");
        ensure!((head[1] & 128 != 0) == masked, "Invalid WebSocket masking direction");
        ensure!(matches!(opcode, 0 | 1 | 2 | 8 | 9 | 10), "Unsupported WebSocket opcode");
        if opcode >= 8 { ensure!(fin && !rsv1 && head[1] & 127 <= 125, "Invalid WebSocket control frame"); }
        else if opcode == 0 { ensure!(fragmented && !rsv1, "Unexpected continuation frame"); }
        else { ensure!(!fragmented, "New data frame during fragmented message"); compressed_message = rsv1; }
        let mut wire = head.to_vec();
        let len = match head[1] & 127 {
            126 => { let mut n = [0;2]; reader.read_exact(&mut n).await?; wire.extend(n); let n = u16::from_be_bytes(n) as u64; ensure!(n >= 126, "Nonminimal frame length"); n }
            127 => { let mut n = [0;8]; reader.read_exact(&mut n).await?; wire.extend(n); let n = u64::from_be_bytes(n); ensure!(n >= 65536 && n < (1u64 << 63), "Invalid frame length"); n }
            n => n as u64,
        };
        ensure!(len <= MAX_BODY as u64, "WebSocket frame exceeds 8 MiB");
        let mut mask = [0;4]; if masked { reader.read_exact(&mut mask).await?; wire.extend(mask); }
        let mut payload = vec![0;len as usize]; reader.read_exact(&mut payload).await?; wire.extend_from_slice(&payload);
        if masked { for (i,b) in payload.iter_mut().enumerate() { *b ^= mask[i%4]; } }
        ensure!(opcode != 8 || payload.len() != 1, "Invalid Close frame");
        writer.write_all(&wire).await?; writer.flush().await?;
        tx.send(Frame { direction: if masked { "client" } else { "server" }.into(), at_ms: started.elapsed().as_millis() as u64, opcode, fin, compressed: opcode < 8 && compressed_message, payload_base64: STANDARD.encode(&payload) }).await.map_err(|_|anyhow::anyhow!("Capture closed"))?;
        if opcode < 8 { fragmented = !fin; }
        if opcode == 8 { return Ok(()); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handshake_rules_match_stage_status_scope_and_nonempty_scripts() {
        let mut flow: Flow = serde_json::from_value(serde_json::json!({"id":"test","parentId":null,"startedAt":0,"durationMs":0,"source":"capture","request":{"method":"GET","url":"https://example.test/ws","headers":[],"bodyBase64":""},"response":null,"error":null,"rawRequestHeadBase64":null,"notes":[]})).unwrap();
        let mut config = crate::intercept::Config { request: true, response: true, scope: "all".into(), rules: vec![crate::intercept::Rule { host: "example.test".into(), status: Some(101), ..Default::default() }] };
        let mut script = crate::scripts::Scripts::default();
        assert!(check_handshake_rules(&flow, &config, &script, false).is_ok());
        flow.response = Some(serde_json::from_value(serde_json::json!({"status":101,"version":"HTTP/1.1","headers":[],"bodyBase64":"","rawHeadBase64":null})).unwrap());
        assert!(check_handshake_rules(&flow, &config, &script, true).unwrap_err().to_string().contains("响应拦截"));
        config.scope = "replay".into();
        assert!(check_handshake_rules(&flow, &config, &script, true).is_ok());
        script.enabled = true; script.before = "before()".into();
        assert!(check_handshake_rules(&flow, &config, &script, true).is_ok());
        script.after = "after()".into();
        assert!(check_handshake_rules(&flow, &config, &script, true).unwrap_err().to_string().contains("响应脚本"));
        config.rules.clear();
        assert!(check_handshake_rules(&flow, &config, &script, true).is_ok());
    }
    #[tokio::test]
    async fn invalid_mask_control_continuation_and_reserved_bits_are_rejected() {
        for wire in [vec![0x81, 1, b'a'], vec![0x09, 0x80, 0,0,0,0], vec![0x80, 0x80, 0,0,0,0], vec![0xa1, 0x80, 0,0,0,0], vec![0xc1, 0x80, 0,0,0,0]] {
            let (tx, _rx) = mpsc::channel(8);
            assert!(relay(wire.as_slice(), tokio::io::sink(), true, false, Instant::now(), tx).await.is_err());
        }
    }
    #[tokio::test]
    async fn negotiated_compression_is_labeled_and_close_is_recorded() {
        let bytes = [0xc1, 0x80, 0,0,0,0, 0x88, 0x80, 0,0,0,0];
        let (tx, mut rx) = mpsc::channel(8);
        relay(bytes.as_slice(), tokio::io::sink(), true, true, Instant::now(), tx).await.unwrap();
        assert!(rx.recv().await.unwrap().compressed);
        let close = rx.recv().await.unwrap(); assert_eq!(close.opcode, 8); assert!(!close.compressed);
    }
}
