//! Browser downloads without the File System Access API. Authenticated writes
//! build a private temporary file; a short-lived, single-use capability lets
//! the browser navigate directly to the loopback service for a streamed file.
use super::{authorize, error, ApiError, Shared};
use axum::{body::{Body, Bytes}, extract::{Path, State}, http::{header, HeaderMap, HeaderValue, StatusCode}, response::Response, routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::{Seek, SeekFrom, Write}, sync::{Mutex, OnceLock}, time::{Duration, Instant}};
use tokio::io::AsyncReadExt;

const CHUNK_LIMIT: usize = 64 * 1024;
const MAX_DRAFTS: usize = 4;
const LIFETIME: Duration = Duration::from_secs(15 * 60);

struct Export {
    file: std::fs::File,
    format: Format,
    offset: u64,
    ready: bool,
    expires: Instant,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Format { Json, Har, Body }
impl Format {
    fn name(self) -> &'static str { match self { Self::Json => "capture.json", Self::Har => "capture.har", Self::Body => "http-capture-body.bin" } }
    fn content_type(self) -> &'static str { match self { Self::Body => "application/octet-stream", _ => "application/json" } }
}

static EXPORTS: OnceLock<Mutex<HashMap<String, Export>>> = OnceLock::new();
fn exports() -> &'static Mutex<HashMap<String, Export>> { EXPORTS.get_or_init(|| Mutex::new(HashMap::new())) }
fn lock() -> Result<std::sync::MutexGuard<'static, HashMap<String, Export>>, ApiError> {
    exports().lock().map_err(|_| error("Export lock poisoned"))
}
fn prune(items: &mut HashMap<String, Export>) { let now=Instant::now(); items.retain(|_, item| item.expires>now); }
fn ticket() -> String { format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple()) }
fn url(id:&str) -> String { format!("http://127.0.0.1:1421/session-export/download/{id}") }
fn expire_later(id:String) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(LIFETIME).await;
            let Ok(mut items)=exports().lock() else { break };
            let Some(item)=items.get(&id) else { break };
            if item.expires<=Instant::now() { items.remove(&id); break }
        }
    });
}

pub(super) fn stage_body_file(file:std::fs::File,length:u64) -> Result<String,ApiError> {
    let mut items=lock()?;
    prune(&mut items);
    if items.len()>=MAX_DRAFTS { return Err((StatusCode::TOO_MANY_REQUESTS,"Too many pending exports".into())); }
    let id=ticket();
    items.insert(id.clone(),Export{file,format:Format::Body,offset:length,ready:true,expires:Instant::now()+LIFETIME});
    expire_later(id.clone());
    Ok(url(&id))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Begin { format: Format }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Append { id: String, offset: u64, bytes: Vec<u8> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Finish { id: String, commit: bool }
#[derive(Serialize)]
struct Download { url: String }

async fn begin(State(state): State<Shared>, headers: HeaderMap, Json(input): Json<Begin>) -> Result<Json<String>, ApiError> {
    authorize(&headers, &state)?;
    if matches!(input.format,Format::Body) { return Err(error("Unsupported session format")); }
    let mut items=lock()?;
    prune(&mut items);
    if items.len()>=MAX_DRAFTS { return Err((StatusCode::TOO_MANY_REQUESTS,"Too many pending exports".into())); }
    let file=tempfile::tempfile().map_err(error)?;
    let id=ticket();
    items.insert(id.clone(),Export{file,format:input.format,offset:0,ready:false,expires:Instant::now()+LIFETIME});
    // Expire abandoned drafts even when the UI never makes another request.
    expire_later(id.clone());
    Ok(Json(id))
}

async fn append(State(state): State<Shared>, headers: HeaderMap, Json(input): Json<Append>) -> Result<Json<u64>, ApiError> {
    authorize(&headers, &state)?;
    if input.bytes.len()>CHUNK_LIMIT { return Err(error("Export chunk exceeds 64 KiB")); }
    let mut items=lock()?;
    let item=items.get_mut(&input.id).ok_or_else(||error("Export not found"))?;
    if item.ready || item.expires<=Instant::now() || item.offset!=input.offset { return Err(error("Export is complete, expired or has a different offset")); }
    let next=item.offset.checked_add(input.bytes.len() as u64).ok_or_else(||error("Export size overflow"))?;
    item.file.write_all(&input.bytes).map_err(error)?;
    item.offset=next;
    item.expires=Instant::now()+LIFETIME;
    Ok(Json(item.offset))
}

async fn finish(State(state): State<Shared>, headers: HeaderMap, Json(input): Json<Finish>) -> Result<Json<Option<Download>>, ApiError> {
    authorize(&headers, &state)?;
    let mut items=lock()?;
    if !input.commit { items.remove(&input.id); return Ok(Json(None)); }
    let item=items.get_mut(&input.id).ok_or_else(||error("Export not found"))?;
    if item.ready || item.expires<=Instant::now() { return Err(error("Export is complete or expired")); }
    item.file.flush().map_err(error)?;
    item.file.seek(SeekFrom::Start(0)).map_err(error)?;
    item.ready=true;
    item.expires=Instant::now()+LIFETIME;
    Ok(Json(Some(Download{url:url(&input.id)})))
}

pub(super) async fn download(Path(ticket): Path<String>) -> Result<Response, ApiError> {
    if ticket.len()!=64 || !ticket.bytes().all(|byte|byte.is_ascii_hexdigit()) {return Err((StatusCode::NOT_FOUND,"Export not found".into()));}
    let (file, length, format)={
        let mut items=lock()?;
        let item=items.get(&ticket).ok_or_else(||(StatusCode::NOT_FOUND,"Export not found".into()))?;
        if !item.ready || item.expires<=Instant::now() {return Err((StatusCode::NOT_FOUND,"Export not found".into()));}
        let item=items.remove(&ticket).expect("checked export");
        (item.file,item.offset,item.format)
    };
    let stream=futures_util::stream::try_unfold((tokio::fs::File::from_std(file),length),|(mut file,remaining)| async move {
        if remaining==0 {return Ok::<_,std::io::Error>(None)}
        let mut bytes=vec![0; remaining.min(CHUNK_LIMIT as u64) as usize];
        let count=file.read(&mut bytes).await?;
        if count==0 {return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof,"Incomplete export"))}
        bytes.truncate(count);
        Ok(Some((Bytes::from(bytes),(file,remaining-count as u64))))
    });
    let mut response=Response::new(Body::from_stream(stream));
    let headers=response.headers_mut();
    headers.insert(header::CONTENT_TYPE,HeaderValue::from_static(format.content_type()));
    headers.insert(header::CONTENT_DISPOSITION,HeaderValue::from_str(&format!("attachment; filename=\"{}\"",format.name())).map_err(error)?);
    headers.insert(header::CONTENT_LENGTH,HeaderValue::from_str(&length.to_string()).map_err(error)?);
    headers.insert(header::CACHE_CONTROL,HeaderValue::from_static("no-store"));
    headers.insert("x-content-type-options",HeaderValue::from_static("nosniff"));
    headers.insert("referrer-policy",HeaderValue::from_static("no-referrer"));
    Ok(response)
}

pub(super) fn router() -> Router<Shared> {
    Router::new()
        .route("/session-export/begin",post(begin))
        .route("/session-export/append",post(append))
        .route("/session-export/finish",post(finish))
        .route("/session-export/download/{ticket}",get(download))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn ordered_chunks_and_one_use_download() {
        let directory=tempfile::tempdir().unwrap();
        let state=super::super::Service::new(capture_core::Engine::open(directory.path()).unwrap(),"test".into()).unwrap();
        let mut headers=HeaderMap::new();
        assert_eq!(begin(State(state.clone()),headers.clone(),Json(Begin{format:Format::Json})).await.unwrap_err().0,StatusCode::UNAUTHORIZED);
        headers.insert(header::AUTHORIZATION,HeaderValue::from_static("Bearer test"));
        let Json(id)=begin(State(state.clone()),headers.clone(),Json(Begin{format:Format::Json})).await.unwrap();
        assert_eq!(append(State(state.clone()),headers.clone(),Json(Append{id:id.clone(),offset:0,bytes:b"hello".to_vec()})).await.unwrap().0,5);
        assert!(append(State(state.clone()),headers.clone(),Json(Append{id:id.clone(),offset:0,bytes:b"duplicate".to_vec()})).await.is_err());
        assert!(append(State(state.clone()),headers.clone(),Json(Append{id:id.clone(),offset:5,bytes:vec![0;CHUNK_LIMIT+1]})).await.is_err());
        assert_eq!(append(State(state.clone()),headers.clone(),Json(Append{id:id.clone(),offset:5,bytes:b" world".to_vec()})).await.unwrap().0,11);
        let Json(link)=finish(State(state.clone()),headers.clone(),Json(Finish{id:id.clone(),commit:true})).await.unwrap();
        assert_eq!(link.unwrap().url,format!("http://127.0.0.1:1421/session-export/download/{id}"));
        assert!(append(State(state.clone()),headers.clone(),Json(Append{id:id.clone(),offset:11,bytes:b"!".to_vec()})).await.is_err());
        let response=download(Path(id.clone())).await.unwrap();
        assert_eq!(response.headers()[header::CONTENT_LENGTH],"11");
        assert_eq!(response.headers()[header::CONTENT_DISPOSITION],"attachment; filename=\"capture.json\"");
        let bytes=axum::body::to_bytes(response.into_body(),1024).await.unwrap();
        assert_eq!(bytes.as_ref(),b"hello world");
        assert!(download(Path(id)).await.is_err());
        let Json(cancel_id)=begin(State(state.clone()),headers.clone(),Json(Begin{format:Format::Har})).await.unwrap();
        let _=finish(State(state),headers,Json(Finish{id:cancel_id.clone(),commit:false})).await.unwrap();
        assert!(download(Path(cancel_id)).await.is_err());
    }

    #[tokio::test]
    async fn full_router_requires_auth_for_writes_but_accepts_only_one_ticket_download() {
        let directory=tempfile::tempdir().unwrap();
        let state=super::super::Service::new(capture_core::Engine::open(directory.path()).unwrap(),"test".into()).unwrap();
        let mut auth=HeaderMap::new();auth.insert(header::AUTHORIZATION,HeaderValue::from_static("Bearer test"));
        let Json(id)=begin(State(state.clone()),auth.clone(),Json(Begin{format:Format::Har})).await.unwrap();
        let _=append(State(state.clone()),auth.clone(),Json(Append{id:id.clone(),offset:0,bytes:b"{\"log\":{}}".to_vec()})).await.unwrap();
        let _=finish(State(state.clone()),auth,Json(Finish{id:id.clone(),commit:true})).await.unwrap();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address=listener.local_addr().unwrap();
        let task=tokio::spawn(async move { axum::serve(listener,super::super::router(state)).await.unwrap() });
        async fn request(address:std::net::SocketAddr,request:String)->String {
            tokio::time::timeout(Duration::from_secs(5),async move {
                let mut socket=tokio::net::TcpStream::connect(address).await.unwrap();
                socket.write_all(request.as_bytes()).await.unwrap();
                let mut result=Vec::new();socket.read_to_end(&mut result).await.unwrap();
                String::from_utf8(result).unwrap()
            }).await.expect("export HTTP fixture stalled")
        }
        let unauthorized=request(address,"POST /session-export/begin HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n".into()).await;
        assert!(unauthorized.starts_with("HTTP/1.1 401"));
        let path=format!("GET /session-export/download/{id} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        let first=request(address,path.clone()).await;
        assert!(first.starts_with("HTTP/1.1 200"));
        assert!(first.contains("{\"log\":{}}"));
        assert!(first.to_ascii_lowercase().contains("attachment; filename=\"capture.har\""));
        assert!(request(address,path).await.starts_with("HTTP/1.1 404"));
        task.abort();
    }
}
