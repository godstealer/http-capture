//! Owner-only history views and exact entity downloads. The parent router must
//! authenticate before JSON extraction; handlers also check that boundary.
use super::{authorize, error, ApiError, Shared};
use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderMap, HeaderValue},
    response::Response,
    routing::post,
    Json, Router,
};
use capture_core::{model::Flow, store::{FlowMetadata, FlowMetadataChanges, McpBodyChunk}};
use serde::{Deserialize, Serialize};
use std::io::{Seek, SeekFrom};
use tokio::io::AsyncReadExt;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Changes { since: Option<String> }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Metadata { id: String, expected_version: Option<String> }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Complete { id: String, expected_version: String }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BodyRead { id: String, variant: String, expected_version: String, offset: u64, length: usize }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BodyDownload { id: String, variant: String, expected_version: String }

#[derive(Serialize)]
struct PreparedBodyDownload { url: String, length: u64 }

async fn prepare_download(State(state): State<Shared>, headers: HeaderMap, Json(input): Json<BodyDownload>) -> Result<Json<PreparedBodyDownload>, ApiError> {
    authorize(&headers, &state)?;
    let store=state.engine.store.clone();
    // Copy and validate the complete, pinned entity before issuing a browser
    // download capability. The database lock is released before navigation.
    let (file,length)=tokio::task::spawn_blocking(move|| -> anyhow::Result<_> {
        let mut file=tempfile::tempfile()?;
        let length=store.copy_body_at_revision(&input.id,&input.variant,&input.expected_version,&mut file)?;
        file.seek(SeekFrom::Start(0))?;
        Ok((file,length))
    }).await.map_err(error)?.map_err(error)?;
    if length>256*1024*1024 {return Err(error("Body download exceeds 256 MiB limit"));}
    let url=super::session_export_routes::stage_body_file(file,length)?;
    Ok(Json(PreparedBodyDownload{url,length}))
}

async fn metadata_changes(State(state): State<Shared>, headers: HeaderMap, Json(input): Json<Changes>) -> Result<Json<FlowMetadataChanges>, ApiError> {
    authorize(&headers, &state)?;
    let store = state.engine.store.clone();
    tokio::task::spawn_blocking(move || store.metadata_changes(input.since.as_deref()))
        .await.map_err(error)?.map(Json).map_err(error)
}

async fn metadata(State(state): State<Shared>, headers: HeaderMap, Json(input): Json<Metadata>) -> Result<Json<Option<FlowMetadata>>, ApiError> {
    authorize(&headers, &state)?;
    let store = state.engine.store.clone();
    tokio::task::spawn_blocking(move || store.metadata(&input.id, input.expected_version.as_deref()))
        .await.map_err(error)?.map(Json).map_err(error)
}

async fn complete(State(state): State<Shared>, headers: HeaderMap, Json(input): Json<Complete>) -> Result<Json<Flow>, ApiError> {
    authorize(&headers, &state)?;
    let store = state.engine.store.clone();
    tokio::task::spawn_blocking(move || store.get_at_revision(&input.id, &input.expected_version))
        .await.map_err(error)?.map(Json).map_err(error)
}

async fn read(State(state): State<Shared>, headers: HeaderMap, Json(input): Json<BodyRead>) -> Result<Json<McpBodyChunk>, ApiError> {
    authorize(&headers, &state)?;
    let store = state.engine.store.clone();
    tokio::task::spawn_blocking(move || store.read_body_at_revision(&input.id, &input.variant, &input.expected_version, input.offset, input.length))
        .await.map_err(error)?.map(Json).map_err(error)
}

async fn download(State(state): State<Shared>, headers: HeaderMap, Json(input): Json<BodyDownload>) -> Result<Response, ApiError> {
    authorize(&headers, &state)?;
    let store = state.engine.store.clone();
    // A private anonymous tempfile is unlinked immediately on Unix and deleted
    // on handle close on Windows. No path or original URL enters the filename.
    // Finish validation/copy under the DB snapshot before returning a body, so
    // slow or cancelled clients never hold the Store mutex or read transaction.
    let (file, length) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let mut file = tempfile::tempfile()?;
        let length = store.copy_body_at_revision(&input.id, &input.variant, &input.expected_version, &mut file)?;
        file.seek(SeekFrom::Start(0))?;
        Ok((file, length))
    }).await.map_err(error)?.map_err(error)?;
    let stream = futures_util::stream::try_unfold((tokio::fs::File::from_std(file), length), |(mut file, remaining)| async move {
        if remaining == 0 { return Ok::<_, std::io::Error>(None); }
        let mut bytes = vec![0; remaining.min(64 * 1024) as usize];
        let count = file.read(&mut bytes).await?;
        if count == 0 { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Incomplete body download")); }
        bytes.truncate(count);
        Ok(Some((Bytes::from(bytes), (file, remaining - count as u64))))
    });
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    headers.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"http-capture-body.bin\""));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from_str(&length.to_string()).map_err(error)?);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    Ok(response)
}

pub(super) fn router() -> Router<Shared> {
    Router::new()
        .route("/flows/metadata/changes", post(metadata_changes))
        .route("/flows/metadata", post(metadata))
        .route("/flows/get", post(complete))
        .route("/flows/body/read", post(read))
        .route("/flows/body/download", post(download))
        .route("/flows/body/prepare", post(prepare_download))
        .layer(DefaultBodyLimit::max(8 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Service;
    use serde_json::{json, Value};
    use std::{net::SocketAddr, sync::Arc, time::Duration};
    use tokio::io::AsyncWriteExt;

    struct Fixture {
        _directory: tempfile::TempDir,
        state: Shared,
        address: SocketAddr,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture { fn drop(&mut self) { self.task.abort(); } }

    async fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let mut engine = capture_core::Engine::open(directory.path()).unwrap();
        Arc::get_mut(&mut engine).unwrap().system_proxy = capture_core::system_proxy::SystemProxy::with_state_directory(directory.path().join("isolated-system-proxy"));
        let state = Service::new(engine, "body-owner-test".into()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = router().route_layer(axum::middleware::from_fn_with_state(state.clone(), crate::authenticated)).with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        Fixture { _directory: directory, state, address, task }
    }

    fn sample(id: &str, encoded: &str) -> Flow {
        serde_json::from_value(json!({
            "id":id,"parentId":null,"startedAt":1,"durationMs":0,"source":"capture","state":"completed",
            "request":{"method":"POST","url":"https://example.invalid/","headers":[],"bodyBase64":encoded},
            "response":null,"error":null,"notes":[]
        })).unwrap()
    }

    async fn raw(address: SocketAddr, path: &str, body: &str, authorized: bool) -> (u16, String, Vec<u8>) {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
            let auth = if authorized { "Authorization: Bearer body-owner-test\r\n" } else { "" };
            socket.write_all(format!("POST {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
            // Intentionally omit even malformed input when unauthenticated.
            if authorized { socket.write_all(body.as_bytes()).await.unwrap(); }
            let mut result = Vec::new(); socket.read_to_end(&mut result).await.unwrap();
            let split = result.windows(4).position(|part| part == b"\r\n\r\n").unwrap();
            let headers = String::from_utf8(result[..split].to_vec()).unwrap();
            let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
            (status, headers, result[split+4..].to_vec())
        }).await.expect("owner body HTTP fixture stalled")
    }

    async fn call(address: SocketAddr, path: &str, body: Value) -> (u16, Value) {
        let (status, _, bytes) = raw(address, path, &body.to_string(), true).await;
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn authenticate_before_extracting_and_reject_unknown_or_unbounded_requests() {
        let fixture = fixture().await;
        for path in ["/flows/metadata/changes", "/flows/metadata", "/flows/get", "/flows/body/read", "/flows/body/download", "/flows/body/prepare"] {
            assert_eq!(raw(fixture.address, path, "{malformed", false).await.0, 401);
            assert_eq!(call(fixture.address, path, json!({"unexpected":"field"})).await.0, 422);
        }
        fixture.state.engine.store.insert(&sample("bounded", "")).unwrap();
        let reference = fixture.state.engine.store.metadata("bounded", None).unwrap().unwrap();
        for length in [0,65_537] {
            assert_eq!(call(fixture.address,"/flows/body/read",json!({"id":"bounded","variant":"request","expectedVersion":reference.body_revision,"offset":0,"length":length})).await.0,400);
        }
        assert_eq!(call(fixture.address,"/flows/body/read",json!({"id":"bounded","variant":"../../body","expectedVersion":reference.body_revision,"offset":0,"length":1})).await.0,400);
        // There is no unversioned compatibility fallback on complete reads.
        assert_eq!(call(fixture.address,"/flows/get",json!({"id":"bounded"})).await.0,422);
        assert_eq!(raw(fixture.address,"/flows/metadata/changes",&format!("{{\"since\":\"{}\"}}","x".repeat(9*1024)),true).await.0,413);
    }

    #[tokio::test]
    async fn history_references_ranges_and_downloads_preserve_exact_versioned_entities() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let fixture = fixture().await;
        let bytes: Vec<u8> = (0..262_151).map(|index| (index%251) as u8).collect();
        let encoded = STANDARD.encode(&bytes);
        for index in 0..251 { fixture.state.engine.store.insert(&sample(&format!("history-{index}"),if index==0 {&encoded} else {""})).unwrap(); }
        let (status, snapshot) = call(fixture.address,"/flows/metadata/changes",json!({})).await;
        assert_eq!(status,200); assert_eq!(snapshot["rows"].as_array().unwrap().len(),251);
        assert!(!snapshot.to_string().contains("bodyBase64"));
        let (status, reference) = call(fixture.address,"/flows/metadata",json!({"id":"history-0"})).await;
        assert_eq!(status,200); assert_eq!(reference["bodyReferences"]["request"]["byteLength"],bytes.len());
        let revision = reference["bodyRevision"].as_str().unwrap();
        let (_, chunk) = call(fixture.address,"/flows/body/read",json!({"id":"history-0","variant":"request","expectedVersion":revision,"offset":65535,"length":65536})).await;
        assert_eq!(STANDARD.decode(chunk["bodyBase64"].as_str().unwrap()).unwrap(),bytes[65_535..131_071]);
        let (status, full) = call(fixture.address,"/flows/get",json!({"id":"history-0","expectedVersion":revision})).await;
        assert_eq!(status,200); assert_eq!(full["request"]["bodyBase64"],encoded);
        let download = json!({"id":"history-0","variant":"request","expectedVersion":revision});
        let (status, headers, downloaded) = raw(fixture.address,"/flows/body/download",&download.to_string(),true).await;
        assert_eq!(status,200); assert_eq!(downloaded,bytes);
        let headers = headers.to_ascii_lowercase();
        for expected in ["content-type: application/octet-stream","content-disposition: attachment; filename=\"http-capture-body.bin\"","x-content-type-options: nosniff","cache-control: no-store"] { assert!(headers.contains(expected)); }
        assert!(headers.contains(&format!("content-length: {}",bytes.len())));
        let (status,prepared)=call(fixture.address,"/flows/body/prepare",download.clone()).await;
        assert_eq!(status,200);
        assert_eq!(prepared["length"],bytes.len());
        let url=prepared["url"].as_str().unwrap();
        assert!(url.starts_with("http://127.0.0.1:1421/session-export/download/"));
        let ticket=url.rsplit('/').next().unwrap().to_string();
        let response=crate::session_export_routes::download(axum::extract::Path(ticket.clone())).await.unwrap();
        assert_eq!(response.headers()[header::CONTENT_TYPE],"application/octet-stream");
        assert_eq!(response.headers()[header::CONTENT_LENGTH],bytes.len().to_string());
        assert_eq!(axum::body::to_bytes(response.into_body(),bytes.len()+1).await.unwrap().as_ref(),bytes);
        assert!(crate::session_export_routes::download(axum::extract::Path(ticket)).await.is_err());
        fixture.state.engine.store.insert(&sample("history-0","bmV3")).unwrap();
        for path in ["/flows/get","/flows/metadata","/flows/body/download","/flows/body/prepare"] {
            let input = if path=="/flows/body/download" || path=="/flows/body/prepare" {download.clone()} else {json!({"id":"history-0","expectedVersion":revision})};
            assert_eq!(call(fixture.address,path,input).await.0,400,"{path} must reject stale references");
        }
        fixture.state.engine.store.delete_flows(vec!["history-0".into()]).unwrap();
        assert_eq!(call(fixture.address,"/flows/metadata",json!({"id":"history-0"})).await,(200,Value::Null));
        assert_eq!(call(fixture.address,"/flows/get",json!({"id":"history-0","expectedVersion":revision})).await.0,400);
    }

    #[tokio::test]
    async fn staged_download_has_bounded_chunks_and_releases_database_before_streaming_or_cancel() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use futures_util::StreamExt;
        let fixture = fixture().await;
        let bytes: Vec<u8> = (0..196_615).map(|index| (index%251) as u8).collect();
        fixture.state.engine.store.insert(&sample("stream",&STANDARD.encode(&bytes))).unwrap();
        let reference = fixture.state.engine.store.metadata("stream",None).unwrap().unwrap();
        let mut headers = HeaderMap::new(); headers.insert(header::AUTHORIZATION,HeaderValue::from_static("Bearer body-owner-test"));
        let response = download(State(fixture.state.clone()),headers,Json(BodyDownload{id:"stream".into(),variant:"request".into(),expected_version:reference.body_revision})).await.unwrap();
        let store = fixture.state.engine.store.clone();
        // The client has not consumed a single response byte yet.
        tokio::time::timeout(Duration::from_secs(2),tokio::task::spawn_blocking(move||store.metadata("stream",None))).await.unwrap().unwrap().unwrap();
        let mut stream = response.into_body().into_data_stream();
        let mut received = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap(); assert!(!chunk.is_empty() && chunk.len()<=65_536);
            assert_eq!(chunk.as_ref(),&bytes[received..received+chunk.len()]); received+=chunk.len();
            if received>=65_536 { break; }
        }
        assert!(received>0 && received<bytes.len());
        drop(stream); // Simulated client cancellation owns/drops the anonymous tempfile.
        assert!(fixture.state.engine.store.metadata("stream",None).unwrap().is_some());
    }
}
