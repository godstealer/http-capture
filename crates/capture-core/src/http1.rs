//! Bounded HTTP/1 framing. Header order is never represented by a map.
use crate::model::{Header, RequestDraft};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};
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
    let headers = parsed.headers.iter().map(|h| Header { name: h.name.into(), value: latin1(h.value) }).collect();
    Ok((parsed.method.context("Missing method")?.into(), parsed.path.context("Missing target")?.into(), headers))
}
pub fn parse_response(head: &[u8]) -> Result<(u16, String, Vec<Header>)> {
    let mut storage = [httparse::EMPTY_HEADER; 256];
    let mut parsed = httparse::Response::new(&mut storage);
    ensure!(parsed.parse(head)?.is_complete(), "Incomplete response head");
    Ok((parsed.code.context("Missing status")?, format!("HTTP/1.{}", parsed.version.unwrap_or(1)),
        parsed.headers.iter().map(|h| Header { name: h.name.into(), value: latin1(h.value) }).collect()))
}

pub fn values<'a>(headers: &'a [Header], name: &'a str) -> impl Iterator<Item = &'a str> {
    headers.iter().filter(move |h| h.name.eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

/// Returns (decoded entity bytes, trailers were present).
pub async fn read_body<R: AsyncBufRead + Unpin>(reader: &mut R, headers: &[Header], eof_body: bool) -> Result<(Vec<u8>, bool)> {
    let lengths: Vec<_> = values(headers, "content-length").collect();
    let transfers: Vec<_> = values(headers, "transfer-encoding").collect();
    ensure!(lengths.len() <= 1, "Duplicate Content-Length is not accepted");
    ensure!(lengths.is_empty() || transfers.is_empty(), "Ambiguous Transfer-Encoding + Content-Length");
    if !transfers.is_empty() {
        ensure!(transfers.len() == 1 && transfers[0].eq_ignore_ascii_case("chunked"), "Unsupported transfer coding");
        let mut body = Vec::new();
        loop {
            let chunk = line(reader, 4096).await?;
            let chunk = std::str::from_utf8(&chunk)?.trim_end_matches("\r\n");
            let size_text = chunk.split(';').next().unwrap_or("");
            ensure!(!size_text.is_empty() && size_text.bytes().all(|b| b.is_ascii_hexdigit()), "Invalid chunk size");
            let size = usize::from_str_radix(size_text, 16)?;
            ensure!(size <= MAX_BODY - body.len(), "Body exceeds 8 MiB limit");
            if size == 0 {
                let mut total = 0;
                let mut trailers = false;
                loop {
                    let trailer = line(reader, MAX_HEAD - total).await?;
                    ensure!(!trailer.is_empty(), "Unexpected EOF in chunk trailers");
                    total += trailer.len();
                    if trailer == b"\r\n" { return Ok((body, trailers)); }
                    trailers = true;
                }
            }
            let start = body.len(); body.resize(start + size, 0);
            reader.read_exact(&mut body[start..]).await?;
            let mut crlf = [0; 2]; reader.read_exact(&mut crlf).await?;
            ensure!(crlf == *b"\r\n", "Invalid chunk terminator");
        }
    }
    if let Some(length) = lengths.first() {
        ensure!(!length.is_empty() && length.bytes().all(|b| b.is_ascii_digit()), "Invalid Content-Length");
        let length: usize = length.parse()?;
        ensure!(length <= MAX_BODY, "Body exceeds 8 MiB limit");
        let mut body = vec![0; length]; reader.read_exact(&mut body).await?;
        return Ok((body, false));
    }
    if eof_body {
        let mut body = Vec::new();
        reader.take((MAX_BODY + 1) as u64).read_to_end(&mut body).await?;
        ensure!(body.len() <= MAX_BODY, "Body exceeds 8 MiB limit");
        return Ok((body, false));
    }
    Ok((Vec::new(), false))
}

/// Remove hop-by-hop headers; retain the relative order and casing of all others.
pub fn end_to_end(headers: &[Header]) -> Vec<Header> {
    let connection_tokens: Vec<_> = values(headers, "connection")
        .flat_map(|v| v.split(',')).map(|v| v.trim().to_ascii_lowercase()).collect();
    headers.iter().filter(|h| {
        let n = h.name.to_ascii_lowercase();
        !["connection", "proxy-connection", "proxy-authorization", "proxy-authenticate", "keep-alive", "transfer-encoding", "te", "trailer", "upgrade"].contains(&n.as_str())
            && !connection_tokens.contains(&n)
    }).cloned().collect()
}

pub fn prepare(draft: &RequestDraft) -> Result<(Url, Vec<Header>, Vec<u8>, Vec<String>)> {
    ensure!(token(&draft.method), "Invalid method");
    ensure!(!["CONNECT", "TRACE"].contains(&draft.method.as_str()), "CONNECT and TRACE are not supported by the editor");
    let url = validate_url(&draft.url)?;
    ensure!(draft.headers.len() <= 256, "Too many headers");
    for h in &draft.headers { ensure!(token(&h.name), "Invalid header name: {}", h.name); header_bytes(&h.value)?; }
    ensure!(values(&draft.headers, "host").count() <= 1, "Multiple Host fields are ambiguous");
    ensure!(values(&draft.headers, "content-length").count() <= 1, "Multiple Content-Length fields are ambiguous");
    ensure!(values(&draft.headers, "expect").all(|v| v.eq_ignore_ascii_case("100-continue")), "Unsupported Expect header");
    let body = STANDARD.decode(&draft.body_base64).context("Invalid body base64")?;
    ensure!(body.len() <= MAX_BODY, "Body exceeds 8 MiB limit");
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
    } else { headers.insert(0, Header { name: "Host".into(), value: host }); notes.push("自动补充 Host。".into()); }
    if let Some(h) = headers.iter_mut().find(|h| h.name.eq_ignore_ascii_case("content-length")) {
        if h.value != body.len().to_string() { notes.push("Content-Length 已按正文字节数更新。".into()); }
        h.value = body.len().to_string();
    } else if !body.is_empty() || matches!(draft.method.as_str(), "POST" | "PUT" | "PATCH") {
        headers.push(Header { name: "Content-Length".into(), value: body.len().to_string() });
        notes.push("自动补充 Content-Length。".into());
    }
    Ok((url, headers, body, notes))
}

pub fn encode_request(method: &str, url: &Url, headers: &[Header], body: &[u8]) -> Result<Vec<u8>> {
    let target = &url[url::Position::BeforePath..url::Position::AfterQuery];
    let mut out = format!("{method} {target} HTTP/1.1\r\n").into_bytes();
    for h in headers {
        out.extend_from_slice(h.name.as_bytes()); out.extend_from_slice(b": ");
        out.extend(header_bytes(&h.value)?); out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    ensure!(out.len() <= MAX_HEAD, "Encoded headers exceed limit");
    out.extend_from_slice(body); Ok(out)
}

pub fn ensure_browser_header_order(headers: &[Header]) -> Result<()> {
    // OrigHeaderMap groups duplicate names. Do not silently reorder A, B, A.
    let mut seen = std::collections::HashSet::new();
    let mut last = String::new();
    for h in headers {
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
    fn capture_keeps_interleaving_and_casing() {
        let (_, _, headers) = parse_request(b"GET / HTTP/1.1\r\nX-A: 1\r\nX-B: 2\r\nx-a: 3\r\n\r\n").unwrap();
        assert_eq!(headers.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["X-A", "X-B", "x-a"]);
        assert!(ensure_browser_header_order(&headers).is_err());
    }
    #[tokio::test]
    async fn chunked_body_and_trailers_are_framed() {
        let mut input = &b"3\r\nabc\r\n2\r\nde\r\n0\r\nX-T: yes\r\n\r\n"[..];
        let headers = vec![Header { name: "Transfer-Encoding".into(), value: "chunked".into() }];
        assert_eq!(read_body(&mut input, &headers, false).await.unwrap(), (b"abcde".to_vec(), true));
    }
    #[tokio::test]
    async fn rejects_ambiguous_framing() {
        let mut input = &b""[..];
        let headers = vec![Header { name: "Content-Length".into(), value: "0".into() }, Header { name: "Transfer-Encoding".into(), value: "chunked".into() }];
        assert!(read_body(&mut input, &headers, false).await.is_err());
    }
    #[test]
    fn prevents_crlf_injection() { assert!(header_bytes("a\r\nInjected: true").is_err()); }
    #[test]
    fn binary_header_values_round_trip() { assert_eq!(header_bytes(&latin1(&[0x80, 0xff])).unwrap(), [0x80, 0xff]); }
}
