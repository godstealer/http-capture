//! Bounded HTTP/1 framing. Header order is never represented by a map.
use crate::model::{Header, RequestDraft};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::io::Read;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use url::Url;

pub const MAX_HEAD: usize = 64 * 1024;
pub const MAX_BODY: usize = 8 * 1024 * 1024;

pub fn latin1(bytes: &[u8]) -> String { bytes.iter().map(|b| char::from(*b)).collect() }
pub fn header_bytes(value: &str) -> Result<Vec<u8>> {
    value.chars().map(|c| {
        ensure!((c as u32) <= 255 && c != '\r' && c != '\n' && (c as u32 >= 32 || c == '\t') && c != '\u{7f}', "Invalid HTTP header value");
        Ok(c as u8)
    }).collect()
}
pub fn token(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
}
pub fn validate_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("Invalid absolute URL")?;
    ensure!(matches!(url.scheme(), "http" | "https") && url.host_str().is_some(), "Only http:// and https:// URLs are supported");
    ensure!(url.username().is_empty() && url.password().is_none(), "Put credentials in an Authorization header, not the URL");
    ensure!(url.fragment().is_none(), "HTTP request URLs cannot contain a fragment");
    Ok(url)
}
pub fn authority(url: &Url) -> String {
    let host = match url.host().expect("validated URL") {
        url::Host::Ipv6(ip) => format!("[{ip}]"),
        host => host.to_string(),
    };
    match url.port() { Some(port) => format!("{host}:{port}"), None => host }
}
pub fn hostname(url: &Url) -> String {
    match url.host().expect("validated URL") {
        url::Host::Ipv6(ip) => ip.to_string(),
        host => host.to_string(),
    }
}

async fn line<R: AsyncBufRead + Unpin>(reader: &mut R, max: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let buf = reader.fill_buf().await?;
        if buf.is_empty() { ensure!(out.is_empty(), "Unexpected EOF in HTTP line"); return Ok(out); }
        let n = buf.iter().position(|b| *b == b'\n').map_or(buf.len(), |i| i + 1);
        ensure!(out.len() + n <= max, "HTTP line or headers exceed limit");
        let done = buf[n - 1] == b'\n';
        out.extend_from_slice(&buf[..n]);
        reader.consume(n);
        if done { ensure!(out.ends_with(b"\r\n"), "HTTP requires CRLF"); return Ok(out); }
    }
}

pub async fn read_head<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    let mut head = Vec::new();
    loop {
        let next = line(reader, MAX_HEAD.saturating_sub(head.len())).await?;
        if next.is_empty() { ensure!(head.is_empty(), "Unexpected EOF in headers"); return Ok(head); }
        let done = next == b"\r\n";
        head.extend(next);
        if done { return Ok(head); }
    }
}

pub fn parse_request(head: &[u8]) -> Result<(String, String, Vec<Header>)> {
    let mut storage = [httparse::EMPTY_HEADER; 256];
    let mut parsed = httparse::Request::new(&mut storage);
    ensure!(parsed.parse(head)?.is_complete(), "Incomplete request head");
    ensure!(parsed.version == Some(1), "Capture currently supports HTTP/1.1 only");
    let headers = parsed.headers.iter().map(|h| Header { disabled: false, name: h.name.into(), value: latin1(h.value) }).collect();
    Ok((parsed.method.context("Missing method")?.into(), parsed.path.context("Missing target")?.into(), headers))
}
pub fn parse_response(head: &[u8]) -> Result<(u16, String, Vec<Header>)> {
    let mut storage = [httparse::EMPTY_HEADER; 256];
    let mut parsed = httparse::Response::new(&mut storage);
    ensure!(parsed.parse(head)?.is_complete(), "Incomplete response head");
    Ok((parsed.code.context("Missing status")?, format!("HTTP/1.{}", parsed.version.unwrap_or(1)),
        parsed.headers.iter().map(|h| Header { disabled: false, name: h.name.into(), value: latin1(h.value) }).collect()))
}

pub fn values<'a>(headers: &'a [Header], name: &'a str) -> impl Iterator<Item = &'a str> {
    headers.iter().filter(move |h| !h.disabled && h.name.eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

/// Returns (decoded entity bytes, trailers were present).
pub async fn read_body<R: AsyncBufRead + Unpin>(reader: &mut R, headers: &[Header], eof_body: bool) -> Result<(Vec<u8>, bool)> {
    read_body_observed(reader, headers, eof_body, false).await
}
pub async fn read_body_observed<R: AsyncBufRead + Unpin>(reader: &mut R, headers: &[Header], eof_body: bool, streaming: bool) -> Result<(Vec<u8>, bool)> {
    let mut body=Vec::new();
    let (_,trailers)=copy_body(reader,&mut body,headers,eof_body,if streaming { MAX_BODY as u64 } else { u64::MAX },streaming).await?;
    Ok((body,trailers))
}

/// Encode as bytes arrive; retain the public Base64 representation without a
/// second full-sized decoded body. EncoderWriter preserves partial triplets.
pub async fn read_body_base64<R: AsyncBufRead + Unpin>(reader:&mut R,headers:&[Header],eof_body:bool,streaming:bool)->Result<(String,bool)> {
    struct Sink(base64::write::EncoderWriter<'static,base64::engine::GeneralPurpose,Vec<u8>>);
    impl AsyncWrite for Sink {
        fn poll_write(mut self:std::pin::Pin<&mut Self>,_:&mut std::task::Context<'_>,bytes:&[u8])->std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(std::io::Write::write(&mut self.0,bytes))
        }
        fn poll_flush(mut self:std::pin::Pin<&mut Self>,_:&mut std::task::Context<'_>)->std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(std::io::Write::flush(&mut self.0))
        }
        fn poll_shutdown(self:std::pin::Pin<&mut Self>,_:&mut std::task::Context<'_>)->std::task::Poll<std::io::Result<()>> {std::task::Poll::Ready(Ok(()))}
    }
    let mut sink=Sink(base64::write::EncoderWriter::new(Vec::new(),&STANDARD));
    let (_,trailers)=copy_body(reader,&mut sink,headers,eof_body,if streaming {MAX_BODY as u64}else{u64::MAX},streaming).await?;
    Ok((String::from_utf8(sink.0.finish()?)?,trailers))
}

/// Stream decoded entity bytes into a caller-owned destination with bounded memory.
/// Leaves the next message unread. On error the destination may contain a prefix;
/// callers must discard it or explicitly mark it incomplete. Does not flush/close it.
pub async fn copy_body<R: AsyncBufRead + Unpin,W: AsyncWrite + Unpin>(reader:&mut R,writer:&mut W,headers:&[Header],eof_body:bool,limit:u64,streaming:bool)->Result<(u64,bool)> {
    let lengths: Vec<_> = values(headers, "content-length").collect();
    let transfers: Vec<_> = values(headers, "transfer-encoding").collect();
    ensure!(lengths.len() <= 1, "Duplicate Content-Length is not accepted");
    ensure!(lengths.is_empty() || transfers.is_empty(), "Ambiguous Transfer-Encoding + Content-Length");
    if !transfers.is_empty() {
        ensure!(transfers.len() == 1 && transfers[0].eq_ignore_ascii_case("chunked"), "Unsupported transfer coding");
        let mut written = 0u64;
        loop {
            let chunk = line(reader, 4096).await?;
            let chunk = std::str::from_utf8(&chunk)?.trim_end_matches("\r\n");
            let size_text = chunk.split(';').next().unwrap_or("");
            ensure!(!size_text.is_empty() && size_text.bytes().all(|b| b.is_ascii_hexdigit()), "Invalid chunk size");
            let size = u64::from_str_radix(size_text, 16)?;
            ensure!(size <= limit - written, "Body exceeds configured limit ({limit} bytes)");
            if size == 0 {
                let mut total = 0;
                let mut trailers = false;
                loop {
                    let trailer = line(reader, MAX_HEAD - total).await?;
                    ensure!(!trailer.is_empty(), "Unexpected EOF in chunk trailers");
                    total += trailer.len();
                    if trailer == b"\r\n" { return Ok((written, trailers)); }
                    trailers = true;
                }
            }
            read_piece(reader, writer, size, streaming).await?;
            written += size;
            let mut crlf = [0; 2]; reader.read_exact(&mut crlf).await?;
            ensure!(crlf == *b"\r\n", "Invalid chunk terminator");
        }
    }
    if let Some(length) = lengths.first() {
        ensure!(!length.is_empty() && length.bytes().all(|b| b.is_ascii_digit()), "Invalid Content-Length");
        let length: u64 = length.parse()?;
        ensure!(length <= limit, "Body exceeds configured limit ({limit} bytes)");
        read_piece(reader, writer, length, streaming).await?;
        return Ok((length, false));
    }
    if eof_body {
        let mut written = 0u64;
        let mut buf = [0; 16384];
        loop {
            let count = reader.read(&mut buf).await?;
            if count == 0 { break; }
            ensure!(count as u64 <= limit - written, "Body exceeds configured limit ({limit} bytes)");
            writer.write_all(&buf[..count]).await?; written += count as u64;
            if streaming { crate::sse::emit(crate::sse::Event::Data(buf[..count].to_vec())).await?; }
        }
        return Ok((written, false));
    }
    Ok((0, false))
}

async fn read_piece<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(reader: &mut R, writer: &mut W, mut remaining: u64, streaming: bool) -> Result<()> {
    let mut buf = [0; 16384];
    while remaining > 0 {
        let len = remaining.min(buf.len() as u64) as usize;
        let count = reader.read(&mut buf[..len]).await?;
        ensure!(count > 0, "Unexpected EOF in response body");
        writer.write_all(&buf[..count]).await?; remaining -= count as u64;
        if streaming { crate::sse::emit(crate::sse::Event::Data(buf[..count].to_vec())).await?; }
    }
    Ok(())
}

/// Remove hop-by-hop headers; retain the relative order and casing of all others.
pub fn end_to_end(headers: &[Header]) -> Vec<Header> {
    let connection_tokens: Vec<_> = values(headers, "connection")
        .flat_map(|v| v.split(',')).map(|v| v.trim().to_ascii_lowercase()).collect();
    headers.iter().filter(|h| !h.disabled).filter(|h| {
        let n = h.name.to_ascii_lowercase();
        !["connection", "proxy-connection", "proxy-authorization", "proxy-authenticate", "keep-alive", "transfer-encoding", "te", "trailer", "upgrade"].contains(&n.as_str())
            && !connection_tokens.contains(&n)
    }).cloned().collect()
}

/// RFC 9113 section 8.2.3: split HTTP/2 Cookie fields must be joined with
/// "; " before forwarding to HTTP/1.1. Other duplicates (notably Set-Cookie)
/// retain their order and individual field lines.
pub fn join_request_cookies(headers: &mut Vec<Header>) -> bool {
    let cookies: Vec<_> = headers.iter().filter(|h| h.name.eq_ignore_ascii_case("cookie")).map(|h| h.value.as_str()).collect();
    if cookies.len() < 2 { return false; }
    let value = cookies.join("; ");
    let mut first = true;
    headers.retain_mut(|h| {
        if !h.name.eq_ignore_ascii_case("cookie") { return true; }
        if !first { return false; }
        first = false;
        h.value = value.clone();
        true
    });
    true
}

pub fn prepare(draft: &RequestDraft) -> Result<(Url, Vec<Header>, Vec<u8>, Vec<String>)> {
    let body = STANDARD.decode(&draft.body_base64).context("Invalid body base64")?;
    let (url,headers,notes)=prepare_fields(draft,body.len() as u64)?;
    Ok((url,headers,body,notes))
}
/// Validate an edited request without materializing its decoded body. Sending
/// paths still call `prepare` or their own streaming encoder as appropriate.
pub fn validate_draft(draft:&RequestDraft)->Result<()> {
    prepare_validated_fields(draft).map(|_|())
}
/// Produce normalized fields using a streaming body validation pass, for
/// transports that serialize the original encoded body separately.
pub(crate) fn prepare_validated_fields(draft:&RequestDraft)->Result<(Url,Vec<Header>,Vec<String>)> {
    let mut reader=base64::read::DecoderReader::new(draft.body_base64.as_bytes(),&STANDARD);
    let mut buffer=[0u8;16*1024];
    let mut length=0u64;
    loop {
        let read=reader.read(&mut buffer).context("Invalid body base64")?;
        if read==0 {break;}
        length=length.checked_add(read as u64).context("Request body is too large")?;
    }
    prepare_fields(draft,length)
}
pub(crate) fn prepare_fields(draft:&RequestDraft,body_len:u64)->Result<(Url,Vec<Header>,Vec<String>)> {
    ensure!(token(&draft.method), "Invalid method");
    ensure!(!["CONNECT", "TRACE"].contains(&draft.method.as_str()), "CONNECT and TRACE are not supported by the editor");
    let url = validate_url(&draft.url)?;
    ensure!(draft.headers.len() <= 256, "Too many headers");
    for h in draft.headers.iter().filter(|h| !h.disabled) { ensure!(token(&h.name), "Invalid header name: {}", h.name); header_bytes(&h.value)?; }
    ensure!(values(&draft.headers, "host").count() <= 1, "Multiple Host fields are ambiguous");
    ensure!(values(&draft.headers, "content-length").count() <= 1, "Multiple Content-Length fields are ambiguous");
    ensure!(values(&draft.headers, "expect").all(|v| v.eq_ignore_ascii_case("100-continue")), "Unsupported Expect header");
    let mut headers = end_to_end(&draft.headers);
    let mut notes = Vec::new();
    if values(&headers, "expect").next().is_some() {
        headers.retain(|h| !h.name.eq_ignore_ascii_case("expect"));
        notes.push("100-continue 已由代理处理，转发完整正文时移除 Expect。".into());
    }
    if headers.len() != draft.headers.len() { notes.push("已移除逐跳代理头；分块正文按实体字节重新定长。".into()); }
    let host = authority(&url);
    if let Some(h) = headers.iter_mut().find(|h| h.name.eq_ignore_ascii_case("host")) {
        if h.value != host { h.value = host; notes.push("Host 已按目标 URL 更新，位置保持不变。".into()); }
    } else { headers.insert(0, Header { disabled: false, name: "Host".into(), value: host }); notes.push("自动补充 Host。".into()); }
    if let Some(h) = headers.iter_mut().find(|h| h.name.eq_ignore_ascii_case("content-length")) {
        if h.value != body_len.to_string() { notes.push("Content-Length 已按正文字节数更新。".into()); }
        h.value = body_len.to_string();
    } else if body_len != 0 || matches!(draft.method.as_str(), "POST" | "PUT" | "PATCH") {
        headers.push(Header { disabled: false, name: "Content-Length".into(), value: body_len.to_string() });
        notes.push("自动补充 Content-Length。".into());
    }
    Ok((url, headers, notes))
}

pub fn encode_request(method: &str, url: &Url, headers: &[Header], body: &[u8]) -> Result<Vec<u8>> {
    let mut out=encode_request_head(method,url,headers)?;
    out.extend_from_slice(body);Ok(out)
}

pub fn encode_request_head(method:&str,url:&Url,headers:&[Header])->Result<Vec<u8>> {
    let target = &url[url::Position::BeforePath..url::Position::AfterQuery];
    let mut out = format!("{method} {target} HTTP/1.1\r\n").into_bytes();
    for h in headers.iter().filter(|h| !h.disabled) {
        out.extend_from_slice(h.name.as_bytes()); out.extend_from_slice(b": ");
        out.extend(header_bytes(&h.value)?); out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    ensure!(out.len() <= MAX_HEAD, "Encoded headers exceed limit");
    Ok(out)
}

pub fn ensure_browser_header_order(headers: &[Header]) -> Result<()> {
    // OrigHeaderMap groups duplicate names. Do not silently reorder A, B, A.
    let mut seen = std::collections::HashSet::new();
    let mut last = String::new();
    for h in headers.iter().filter(|h| !h.disabled) {
        let name = h.name.to_ascii_lowercase();
        if name != last && !seen.insert(name.clone()) {
            bail!("浏览器发送暂不支持交错重复头；请选择 native 保序模式，或将同名字段放到相邻位置");
        }
        last = name;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streaming_request_validation_matches_send_preparation() {
        for encoded in ["", "YQ==", "YWI=", "YWJj", "AAECAw==", "YQ", "YQ=", "A", "AAAA=", "AA==A", "Y Q==", "YWJj\n", "_w=="] {
            let draft:RequestDraft=serde_json::from_value(serde_json::json!({
                "method":"POST","url":"https://example.test/upload","headers":[{"name":"X-Check","value":"yes"}],"bodyBase64":encoded,
            })).unwrap();
            assert_eq!(validate_draft(&draft).is_ok(),prepare(&draft).is_ok(),"{encoded:?}");
            if let Ok((expected_url,expected_headers,_,expected_notes))=prepare(&draft) {
                let (url,headers,notes)=prepare_validated_fields(&draft).unwrap();
                assert_eq!(url,expected_url);assert_eq!(headers,expected_headers);assert_eq!(notes,expected_notes);
            }
        }
        let bytes=vec![17u8;8*1024*1024+3];
        let draft:RequestDraft=serde_json::from_value(serde_json::json!({
            "method":"POST","url":"https://example.test/upload","headers":[{"name":"Content-Length","value":"0"}],"bodyBase64":STANDARD.encode(&bytes),
        })).unwrap();
        let pointer=draft.body_base64.as_ptr();
        let (validated_url,validated_headers,validated_notes)=prepare_validated_fields(&draft).unwrap();
        assert_eq!(draft.body_base64.as_ptr(),pointer);
        let (url,headers,body,notes)=prepare(&draft).unwrap();
        assert_eq!(body,bytes);
        assert_eq!(validated_url,url);assert_eq!(validated_headers,headers);assert_eq!(validated_notes,notes);
        assert_eq!(headers.iter().find(|h|h.name.eq_ignore_ascii_case("content-length")).unwrap().value,bytes.len().to_string());
    }
    #[test]
    fn joins_only_request_cookies_preserving_other_fields_and_positions() {
        let mut headers = vec![
            Header { disabled: false, name: "Cookie".into(), value: "a=1".into() },
            Header { disabled: false, name: "X-Test".into(), value: "first".into() },
            Header { disabled: false, name: "cookie".into(), value: "session=abc; locale=ja".into() },
            Header { disabled: false, name: "X-Test".into(), value: "second".into() },
            Header { disabled: false, name: "Set-Cookie".into(), value: "one=1".into() },
            Header { disabled: false, name: "Set-Cookie".into(), value: "two=2".into() },
        ];
        assert!(join_request_cookies(&mut headers));
        let pairs: Vec<_> = headers.iter().map(|h| (h.name.as_str(), h.value.as_str())).collect();
        assert_eq!(pairs, [("Cookie", "a=1; session=abc; locale=ja"), ("X-Test", "first"), ("X-Test", "second"), ("Set-Cookie", "one=1"), ("Set-Cookie", "two=2")]);
        assert!(!join_request_cookies(&mut headers));
        assert!(!join_request_cookies(&mut vec![]));
    }
    #[tokio::test]
    async fn base64_stream_preserves_triplets_and_message_boundaries() {
        for size in [0,1,2,3,16383,16384,16385,100001] {
            let bytes:Vec<u8>=(0..size).map(|n|(n%251) as u8).collect();
            let mut wire=bytes.clone();wire.extend_from_slice(b"NEXT");
            let mut reader=tokio::io::BufReader::with_capacity(7,wire.as_slice());
            let headers=vec![Header { disabled: false,name:"Content-Length".into(),value:size.to_string()}];
            let (encoded,trailers)=read_body_base64(&mut reader,&headers,false,false).await.unwrap();
            assert_eq!(encoded,STANDARD.encode(bytes));assert!(!trailers);
            let mut next=Vec::new();reader.read_to_end(&mut next).await.unwrap();assert_eq!(next,b"NEXT");
        }
        let headers=vec![Header { disabled: false,name:"Transfer-Encoding".into(),value:"chunked".into()}];
        let mut reader=&b"1\r\na\r\n2\r\nbc\r\n1\r\nd\r\n0\r\n\r\n"[..];
        assert_eq!(read_body_base64(&mut reader,&headers,false,false).await.unwrap().0,"YWJjZA==");
        let mut short=&b"abc"[..];let headers=vec![Header { disabled: false,name:"Content-Length".into(),value:"4".into()}];
        assert!(read_body_base64(&mut short,&headers,false,false).await.is_err());
    }
    #[test]
    fn capture_keeps_interleaving_and_casing() {
        let (_, _, headers) = parse_request(b"GET / HTTP/1.1\r\nX-A: 1\r\nX-B: 2\r\nx-a: 3\r\n\r\n").unwrap();
        assert_eq!(headers.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["X-A", "X-B", "x-a"]);
        assert!(ensure_browser_header_order(&headers).is_err());
    }
    #[tokio::test]
    async fn chunked_body_and_trailers_are_framed() {
        let mut input = &b"3\r\nabc\r\n2\r\nde\r\n0\r\nX-T: yes\r\n\r\n"[..];
        let headers = vec![Header { disabled: false, name: "Transfer-Encoding".into(), value: "chunked".into() }];
        assert_eq!(read_body(&mut input, &headers, false).await.unwrap(), (b"abcde".to_vec(), true));
    }
    #[tokio::test]
    async fn rejects_ambiguous_framing() {
        let mut input = &b""[..];
        let headers = vec![Header { disabled: false, name: "Content-Length".into(), value: "0".into() }, Header { disabled: false, name: "Transfer-Encoding".into(), value: "chunked".into() }];
        assert!(read_body(&mut input, &headers, false).await.is_err());
    }
    #[test]
    fn prevents_crlf_injection() { assert!(header_bytes("a\r\nInjected: true").is_err()); }
    #[test]
    fn binary_header_values_round_trip() { assert_eq!(header_bytes(&latin1(&[0x80, 0xff])).unwrap(), [0x80, 0xff]); }
    #[tokio::test]
    async fn streams_large_entity_to_disk_without_consuming_next_message(){
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("body.bin");
        let size=9*1024*1024;
        let bytes=vec![0x81;size];let mut wire=bytes.clone();wire.extend_from_slice(b"NEXT");
        let mut input=wire.as_slice();let mut file=tokio::fs::File::create(&path).await.unwrap();
        let headers=vec![Header { disabled: false,name:"Content-Length".into(),value:size.to_string()}];
        assert_eq!(copy_body(&mut input,&mut file,&headers,false,size as u64,false).await.unwrap(),(size as u64,false));
        file.flush().await.unwrap();assert_eq!(input,b"NEXT");assert_eq!(tokio::fs::read(&path).await.unwrap(),bytes);
    }
    #[tokio::test]
    async fn streaming_boundaries_limits_and_sink_failure(){
        let headers=vec![Header { disabled: false,name:"Transfer-Encoding".into(),value:"chunked".into()}];
        let mut input=&b"3\r\nabc\r\n0\r\nX-T: yes\r\n\r\nNEXT"[..];let mut output=Vec::new();
        assert_eq!(copy_body(&mut input,&mut output,&headers,false,3,false).await.unwrap(),(3,true));assert_eq!(input,b"NEXT");assert_eq!(output,b"abc");
        let mut input=&b"4\r\nabcd\r\n0\r\n\r\n"[..];let mut output=Vec::new();assert!(copy_body(&mut input,&mut output,&headers,false,3,false).await.is_err());assert!(output.is_empty());
        let mut input=&b"abc"[..];let mut output=Vec::new();assert_eq!(copy_body(&mut input,&mut output,&[],true,3,false).await.unwrap(),(3,false));assert_eq!(output,b"abc");
        let mut input=&b"abcd"[..];assert!(copy_body(&mut input,&mut tokio::io::sink(),&[],true,3,false).await.is_err());
        let fixed=vec![Header { disabled: false,name:"Content-Length".into(),value:"4".into()}];let mut input=&b"abc"[..];assert!(copy_body(&mut input,&mut tokio::io::sink(),&fixed,false,4,false).await.is_err());
        let (mut sink,peer)=tokio::io::duplex(1);drop(peer);let mut input=&b"abcd"[..];assert!(copy_body(&mut input,&mut sink,&fixed,false,4,false).await.is_err());
    }

}
