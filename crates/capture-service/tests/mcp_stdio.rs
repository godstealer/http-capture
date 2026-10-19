//! Exercise the production executable's early stdio branch. Protocol behavior
//! with the official HTTP and stdio SDK is covered in capture-mcp::bridge.
use axum::{Json,Router,routing::post};
use serde_json::{json,Value};
use std::{process::Stdio,time::Duration};
use tokio::{io::{AsyncBufReadExt,AsyncWriteExt,BufReader},process::Command};

fn child()->Command {
    let mut command=Command::new(env!("CARGO_BIN_EXE_capture-service"));
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    command
}

#[tokio::test]
async fn stdio_cli_rejects_token_arguments_and_reports_missing_env_only_on_stderr() {
    let directory=tempfile::tempdir().unwrap();
    let output=child().args(["--mcp-stdio","--url","http://127.0.0.1:1/mcp","--token","argv-fixture-must-never-print"])
        .current_dir(directory.path()).output().await.unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(),"errors must never contaminate MCP stdout");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("argv-fixture-must-never-print"));
    let output=child().args(["--mcp-stdio","--url","http://127.0.0.1:1/mcp"])
        .env_remove(capture_mcp::bridge::TOKEN_ENV).current_dir(directory.path()).output().await.unwrap();
    assert!(!output.status.success());assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("HTTP_CAPTURE_MCP_TOKEN"));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(),0,"stdio must not create an Engine, CA, database, or control token");
}

#[tokio::test]
async fn production_stdio_executable_keeps_stdout_protocol_only_and_exits_on_eof() {
    let app=Router::new().route("/mcp",post(|headers:axum::http::HeaderMap,Json(message):Json<Value>|async move {
        assert_eq!(headers.get("authorization").unwrap(),"Bearer child-fixture-private-token");
        let result=match message["method"].as_str().unwrap_or_default() {
            "initialize"=>json!({"protocolVersion":"2025-03-26","serverInfo":{"name":"fixture","version":"1"},"capabilities":{"tools":{}}}),
            "server/discover"=>json!({"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"fixture","version":"1"}}}),
            "tools/list"=>json!({"tools":[{"name":"echo","description":"fixture","inputSchema":{"type":"object"}}]}),
            "tools/call"=>json!({"content":[{"type":"text","text":"fixture-result"}]}),
            _=>return (axum::http::StatusCode::ACCEPTED,Json(Value::Null)),
        };
        (axum::http::StatusCode::OK,Json(json!({"jsonrpc":"2.0","id":message["id"],"result":result})))
    }));
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url=format!("http://{}/mcp",listener.local_addr().unwrap());
    let server=tokio::spawn(async move{axum::serve(listener,app).await.unwrap()});
    let directory=tempfile::tempdir().unwrap();
    let mut child=child().args(["--mcp-stdio","--url",&url]).env(capture_mcp::bridge::TOKEN_ENV,"child-fixture-private-token")
        .current_dir(directory.path()).spawn().unwrap();
    let mut input=child.stdin.take().unwrap();
    let output=child.stdout.take().unwrap();
    let mut lines=BufReader::new(output).lines();
    input.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"child-test-AI","version":"1"}}})).as_bytes()).await.unwrap();
    let initialized=tokio::time::timeout(Duration::from_secs(8),lines.next_line()).await.unwrap().unwrap().unwrap();
    let initialized:Value=serde_json::from_str(&initialized).expect("the first stdout line must be a protocol frame");
    assert_eq!(initialized["id"],1);assert!(initialized["result"].is_object(),"{initialized}");
    input.write_all(format!("{}\n{}\n",json!({"jsonrpc":"2.0","method":"notifications/initialized"}),json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).as_bytes()).await.unwrap();
    let listed=tokio::time::timeout(Duration::from_secs(5),lines.next_line()).await.unwrap().unwrap().unwrap();
    let listed:Value=serde_json::from_str(&listed).unwrap();assert_eq!(listed["result"]["tools"][0]["name"],"echo");
    input.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"message":"中文"}}})).as_bytes()).await.unwrap();
    let called=tokio::time::timeout(Duration::from_secs(5),lines.next_line()).await.unwrap().unwrap().unwrap();
    let called:Value=serde_json::from_str(&called).unwrap();assert_eq!(called["result"]["content"][0]["text"],"fixture-result");
    input.shutdown().await.unwrap();drop(input);
    let output=tokio::time::timeout(Duration::from_secs(8),child.wait_with_output()).await.unwrap().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("child-fixture-private-token"));
    while let Some(line)=lines.next_line().await.unwrap() {let _:Value=serde_json::from_str(&line).expect("every stdout line must be an MCP frame");}
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(),0);
    server.abort();
}

#[tokio::test]
async fn production_stdio_exits_after_server_shutdown_even_if_ai_keeps_stdin_open() {
    let app=Router::new().route("/mcp",post(|Json(message):Json<Value>|async move {
        let result=json!({"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"fixture","version":"1"}}});
        Json(json!({"jsonrpc":"2.0","id":message["id"],"result":result}))
    }));
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url=format!("http://{}/mcp",listener.local_addr().unwrap());
    let (stop,stopped)=tokio::sync::oneshot::channel::<()>();
    let server=tokio::spawn(async move {axum::serve(listener,app).with_graceful_shutdown(async{let _=stopped.await;}).await.unwrap()});
    let directory=tempfile::tempdir().unwrap();
    let mut child=child().args(["--mcp-stdio","--url",&url]).env(capture_mcp::bridge::TOKEN_ENV,"no-eof-fixture-token")
        .current_dir(directory.path()).spawn().unwrap();
    let mut input=child.stdin.take().unwrap();
    let mut lines=BufReader::new(child.stdout.take().unwrap()).lines();
    input.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"idle-test-AI","version":"1"}}})).as_bytes()).await.unwrap();
    let line=tokio::time::timeout(Duration::from_secs(8),lines.next_line()).await.unwrap().unwrap().unwrap();
    let initialized:Value=serde_json::from_str(&line).unwrap();assert!(initialized["result"].is_object(),"{initialized}");
    input.write_all(format!("{}\n",json!({"jsonrpc":"2.0","method":"notifications/initialized"})).as_bytes()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop.send(()).unwrap();
    let output=tokio::time::timeout(Duration::from_secs(8),child.wait_with_output()).await.unwrap().unwrap();
    assert!(!output.status.success(),"the bridge should report that its upstream application stopped");
    assert!(String::from_utf8_lossy(&output.stderr).contains("closed"),"{}",String::from_utf8_lossy(&output.stderr));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("no-eof-fixture-token"));
    // Keep this handle alive until AFTER process exit. A normal Tokio runtime
    // drop waits forever for its uncancellable blocking stdin reader here.
    drop(input);
    while let Some(line)=lines.next_line().await.unwrap(){let _:Value=serde_json::from_str(&line).unwrap();}
    server.await.unwrap();
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(),0);
}
