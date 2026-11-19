//! Read-only, bounded HTTP/1 session framing for an authenticated TCP tunnel.
//! Bytes are inspected but never rewritten or retained as a body. Unsupported or
//! ambiguous framing terminates observation, never the underlying connection.
use super::mobile_http_observer::{
    parse_request, parse_response, Collected, Direction, HeadCollector, RequestHead, ResponseHead,
};
use crate::model::Header;
use std::collections::VecDeque;

pub(crate) const MAX_PENDING: usize = 8;
const MAX_INTERIM: usize = 8;
const MAX_CHUNK_LINE: usize = 8 * 1024;
const MAX_CHUNK_METADATA: usize = 64 * 1024;
const MAX_TRAILER_BYTES: usize = 64 * 1024;
const MAX_TRAILER_FIELDS: usize = 256;
type StopReason = &'static str;

#[derive(Debug)]
pub(crate) enum SessionEvent {
    Request { sequence: u64, head: RequestHead },
    RequestEnd { sequence: u64 },
    Response { sequence: u64, head: ResponseHead },
    ResponseEnd { sequence: u64 },
    Stopped { reason: &'static str },
}
struct Pending {
    sequence: u64,
    head_method: String,
    request_ended: bool,
    close_after: bool,
    barrier: bool,
}
enum RequestPhase {
    Head(HeadCollector),
    Body { sequence: u64, body: BodySkipper },
    Barrier { sequence: u64 },
    Stopped,
}
enum ResponsePhase {
    Head {
        collector: HeadCollector,
        interim: usize,
    },
    Body {
        sequence: u64,
        body: BodySkipper,
        close_after: bool,
    },
    Stopped,
}
pub(crate) struct SessionObserver {
    request: RequestPhase,
    response: ResponsePhase,
    pending: VecDeque<Pending>,
    next_sequence: u64,
    stopped: bool,
}
impl Default for SessionObserver {
    fn default() -> Self {
        Self {
            request: RequestPhase::Head(HeadCollector::new(Direction::Request)),
            response: response_head(),
            pending: VecDeque::new(),
            next_sequence: 0,
            stopped: false,
        }
    }
}
fn response_head() -> ResponsePhase {
    ResponsePhase::Head {
        collector: HeadCollector::new(Direction::Response),
        interim: 0,
    }
}
impl SessionObserver {
    pub(crate) fn request_finished(&self) -> bool {
        matches!(self.request, RequestPhase::Stopped)
    }
    pub(crate) fn response_finished(&self) -> bool {
        matches!(self.response, ResponsePhase::Stopped)
    }
    fn halt(&mut self) {
        self.stopped = true;
        self.request = RequestPhase::Stopped;
        self.response = ResponsePhase::Stopped;
        self.pending.clear();
    }
    fn stop(&mut self, reason: StopReason, emit: &mut impl FnMut(SessionEvent) -> bool) {
        if self.stopped {
            return;
        }
        self.halt();
        let _ = emit(SessionEvent::Stopped { reason });
    }
    fn emit(&mut self, event: SessionEvent, emit: &mut impl FnMut(SessionEvent) -> bool) -> bool {
        if emit(event) {
            true
        } else {
            self.halt();
            false
        }
    }
    pub(crate) fn observe_request(
        &mut self,
        mut bytes: &[u8],
        emit: &mut impl FnMut(SessionEvent) -> bool,
    ) {
        while !bytes.is_empty() && !self.stopped {
            match std::mem::replace(&mut self.request, RequestPhase::Stopped) {
                RequestPhase::Head(mut collector) => {
                    if self.pending.len() >= MAX_PENDING {
                        self.stop("Pending request limit exceeded", emit);
                        return;
                    }
                    match collector.push(bytes) {
                        Collected::Partial => {
                            self.request = RequestPhase::Head(collector);
                            return;
                        }
                        Collected::Disabled => {
                            self.stop("Invalid or oversized request head", emit);
                            return;
                        }
                        Collected::Head { consumed, raw } => {
                            bytes = &bytes[consumed..];
                            let Some(head) = parse_request(raw) else {
                                self.stop("Invalid request head", emit);
                                return;
                            };
                            let Some(next) = self.next_sequence.checked_add(1) else {
                                self.stop("Request sequence limit exceeded", emit);
                                return;
                            };
                            let sequence = self.next_sequence;
                            self.next_sequence = next;
                            let framing = declared_framing(&head.headers, &head.version);
                            let persistence = persistent(&head.headers, &head.version);
                            let barrier = head.method == "CONNECT"
                                || has_header(&head.headers, "upgrade")
                                || connection_has(&head.headers, "upgrade");
                            self.pending.push_back(Pending {
                                sequence,
                                head_method: head.method.clone(),
                                request_ended: false,
                                close_after: !persistence.unwrap_or(false),
                                barrier,
                            });
                            if !self.emit(SessionEvent::Request { sequence, head }, emit) {
                                return;
                            }
                            let framing = match framing {
                                Ok(value) => value,
                                Err(reason) => {
                                    self.stop(reason, emit);
                                    return;
                                }
                            };
                            if let Err(reason) = persistence {
                                self.stop(reason, emit);
                                return;
                            }
                            match framing {
                                Framing::Absent | Framing::Length(0) => {
                                    if !self.request_end(sequence, emit) {
                                        return;
                                    }
                                }
                                Framing::Length(length) => {
                                    self.request = RequestPhase::Body {
                                        sequence,
                                        body: BodySkipper::Fixed(length),
                                    }
                                }
                                Framing::Chunked => {
                                    self.request = RequestPhase::Body {
                                        sequence,
                                        body: BodySkipper::Chunked(Chunked::default()),
                                    }
                                }
                            }
                        }
                    }
                }
                RequestPhase::Body { sequence, mut body } => match body.consume(bytes) {
                    Ok(progress) => {
                        bytes = &bytes[progress.consumed..];
                        if progress.ended {
                            if !self.request_end(sequence, emit) {
                                return;
                            }
                        } else {
                            self.request = RequestPhase::Body { sequence, body };
                            return;
                        }
                    }
                    Err(reason) => {
                        self.stop(reason, emit);
                        return;
                    }
                },
                RequestPhase::Barrier { .. } => {
                    // These bytes might be an eager CONNECT/Upgrade payload.
                    // Never buffer/reclassify them after a later error response.
                    self.request = RequestPhase::Stopped;
                    return;
                }
                RequestPhase::Stopped => return,
            }
        }
    }
    fn request_end(&mut self, sequence: u64, emit: &mut impl FnMut(SessionEvent) -> bool) -> bool {
        let Some(pending) = self
            .pending
            .iter_mut()
            .find(|entry| entry.sequence == sequence)
        else {
            self.stop("Request association was lost", emit);
            return false;
        };
        pending.request_ended = true;
        let (close_after, barrier) = (pending.close_after, pending.barrier);
        if !self.emit(SessionEvent::RequestEnd { sequence }, emit) {
            return false;
        }
        self.request = if close_after {
            RequestPhase::Stopped
        } else if barrier {
            RequestPhase::Barrier { sequence }
        } else {
            RequestPhase::Head(HeadCollector::new(Direction::Request))
        };
        true
    }
    pub(crate) fn observe_response(
        &mut self,
        mut bytes: &[u8],
        emit: &mut impl FnMut(SessionEvent) -> bool,
    ) {
        while !bytes.is_empty() && !self.stopped {
            match std::mem::replace(&mut self.response, ResponsePhase::Stopped) {
                ResponsePhase::Head {
                    mut collector,
                    mut interim,
                } => {
                    if self.pending.is_empty() {
                        self.stop("Server bytes without an observed request", emit);
                        return;
                    }
                    match collector.push(bytes) {
                        Collected::Partial => {
                            self.response = ResponsePhase::Head { collector, interim };
                            return;
                        }
                        Collected::Disabled => {
                            self.stop("Invalid or oversized response head", emit);
                            return;
                        }
                        Collected::Head { consumed, raw } => {
                            bytes = &bytes[consumed..];
                            let Some(head) = parse_response(raw) else {
                                self.stop("Invalid response head", emit);
                                return;
                            };
                            if (100..200).contains(&head.status) && head.status != 101 {
                                interim += 1;
                                if interim > MAX_INTERIM {
                                    self.stop("Interim response limit exceeded", emit);
                                    return;
                                }
                                self.response = ResponsePhase::Head { collector, interim };
                                continue;
                            }
                            let pending = self.pending.front().expect("checked pending request");
                            let (sequence, request_ended, request_close) =
                                (pending.sequence, pending.request_ended, pending.close_after);
                            let opaque = head.status == 101
                                || (pending.head_method == "CONNECT"
                                    && (200..300).contains(&head.status));
                            let no_body =
                                pending.head_method == "HEAD" || matches!(head.status, 204 | 304);
                            let framing = if opaque || no_body {
                                Ok(Framing::Length(0))
                            } else {
                                declared_framing(&head.headers, &head.version)
                            };
                            let persistence = if opaque {
                                Ok(false)
                            } else {
                                persistent(&head.headers, &head.version)
                            };
                            if !self.emit(SessionEvent::Response { sequence, head }, emit) {
                                return;
                            }
                            if !request_ended {
                                self.stop("Final response before request body completed", emit);
                                return;
                            }
                            if opaque {
                                self.response_end(sequence, true, true, emit);
                                return;
                            }
                            let framing = match framing {
                                Ok(value) => value,
                                Err(reason) => {
                                    self.stop(reason, emit);
                                    return;
                                }
                            };
                            let close_after = match persistence {
                                Ok(value) => request_close || !value,
                                Err(reason) => {
                                    self.stop(reason, emit);
                                    return;
                                }
                            };
                            match framing {
                                Framing::Length(0) => {
                                    if !self.response_end(sequence, close_after, false, emit) {
                                        return;
                                    }
                                }
                                Framing::Length(length) => {
                                    self.response = ResponsePhase::Body {
                                        sequence,
                                        body: BodySkipper::Fixed(length),
                                        close_after,
                                    }
                                }
                                Framing::Chunked => {
                                    self.response = ResponsePhase::Body {
                                        sequence,
                                        body: BodySkipper::Chunked(Chunked::default()),
                                        close_after,
                                    }
                                }
                                Framing::Absent => {
                                    // No later status-looking substring can escape this entity.
                                    self.request = RequestPhase::Stopped;
                                    self.response = ResponsePhase::Body {
                                        sequence,
                                        body: BodySkipper::Close,
                                        close_after: true,
                                    };
                                }
                            }
                        }
                    }
                }
                ResponsePhase::Body {
                    sequence,
                    mut body,
                    close_after,
                } => match body.consume(bytes) {
                    Ok(progress) => {
                        bytes = &bytes[progress.consumed..];
                        if progress.ended {
                            if !self.response_end(sequence, close_after, false, emit) {
                                return;
                            }
                        } else {
                            self.response = ResponsePhase::Body {
                                sequence,
                                body,
                                close_after,
                            };
                            return;
                        }
                    }
                    Err(reason) => {
                        self.stop(reason, emit);
                        return;
                    }
                },
                ResponsePhase::Stopped => return,
            }
        }
    }
    fn response_end(
        &mut self,
        sequence: u64,
        close_after: bool,
        opaque: bool,
        emit: &mut impl FnMut(SessionEvent) -> bool,
    ) -> bool {
        if self
            .pending
            .front()
            .is_none_or(|entry| entry.sequence != sequence)
        {
            self.stop("Response association was lost", emit);
            return false;
        }
        if !self.emit(SessionEvent::ResponseEnd { sequence }, emit) {
            return false;
        }
        self.pending.pop_front();
        if close_after || opaque {
            if self.pending.is_empty() {
                self.halt();
            } else {
                self.stop(
                    if opaque {
                        "Protocol switched with pending requests"
                    } else {
                        "Connection closed with pending requests"
                    },
                    emit,
                );
            }
            return false;
        }
        if matches!(self.request, RequestPhase::Barrier { sequence: barrier } if barrier == sequence)
        {
            self.request = RequestPhase::Head(HeadCollector::new(Direction::Request));
        }
        if self.request_finished() && self.pending.is_empty() {
            self.halt();
            return false;
        }
        self.response = response_head();
        true
    }
    pub(crate) fn eof_request(&mut self, emit: &mut impl FnMut(SessionEvent) -> bool) {
        if self.stopped {
            return;
        }
        match std::mem::replace(&mut self.request, RequestPhase::Stopped) {
            RequestPhase::Head(collector) if collector.is_empty() => {
                if self.pending.is_empty() {
                    self.halt();
                }
            }
            RequestPhase::Stopped | RequestPhase::Barrier { .. } => {
                if self.pending.is_empty() {
                    self.halt();
                }
            }
            RequestPhase::Head(_) => self.stop("Truncated request head", emit),
            RequestPhase::Body { .. } => self.stop("Truncated request body", emit),
        }
    }
    pub(crate) fn eof_response(&mut self, emit: &mut impl FnMut(SessionEvent) -> bool) {
        if self.stopped {
            return;
        }
        match std::mem::replace(&mut self.response, ResponsePhase::Stopped) {
            ResponsePhase::Body {
                sequence,
                body: BodySkipper::Close,
                ..
            } => {
                self.response_end(sequence, true, false, emit);
            }
            ResponsePhase::Head { collector, .. }
                if collector.is_empty() && self.pending.is_empty() =>
            {
                self.halt()
            }
            ResponsePhase::Stopped if self.pending.is_empty() => self.halt(),
            ResponsePhase::Head { .. } => self.stop("Truncated or missing response head", emit),
            ResponsePhase::Body { .. } => self.stop("Truncated response body", emit),
            ResponsePhase::Stopped => self.stop("Response stream stopped before completion", emit),
        }
    }
}

#[derive(Clone, Copy)]
enum Framing {
    Absent,
    Length(u64),
    Chunked,
}
fn has_header(headers: &[Header], name: &str) -> bool {
    headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case(name))
}
fn connection_has(headers: &[Header], token: &str) -> bool {
    headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("connection"))
        .flat_map(|header| header.value.split(','))
        .any(|value| value.trim_matches([' ', '\t']).eq_ignore_ascii_case(token))
}
fn persistent(headers: &[Header], version: &str) -> Result<bool, StopReason> {
    for field in headers
        .iter()
        .filter(|field| field.name.eq_ignore_ascii_case("connection"))
    {
        if field.value.split(',').any(|value| {
            let value = value.trim_matches([' ', '\t']);
            value.is_empty() || !value.bytes().all(token_byte)
        }) {
            return Err("Invalid Connection tokens");
        }
    }
    Ok(!connection_has(headers, "close")
        && (version == "HTTP/1.1" || connection_has(headers, "keep-alive")))
}
fn declared_framing(headers: &[Header], version: &str) -> Result<Framing, StopReason> {
    let mut lengths = headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("content-length"));
    let length = lengths.next();
    if lengths.next().is_some() {
        return Err("Multiple Content-Length fields are not observed");
    }
    let mut transfers = headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("transfer-encoding"));
    let transfer = transfers.next();
    if transfers.next().is_some() {
        return Err("Multiple Transfer-Encoding fields are not observed");
    }
    if length.is_some() && transfer.is_some() {
        return Err("Transfer-Encoding with Content-Length is not observed");
    }
    if let Some(transfer) = transfer {
        if version != "HTTP/1.1"
            || !transfer
                .value
                .trim_matches([' ', '\t'])
                .eq_ignore_ascii_case("chunked")
        {
            return Err("Unsupported Transfer-Encoding");
        }
        return Ok(Framing::Chunked);
    }
    if let Some(length) = length {
        let value = length.value.trim_matches([' ', '\t']);
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("Invalid Content-Length");
        }
        let count = value
            .bytes()
            .try_fold(0u64, |count, digit| {
                count
                    .checked_mul(10)
                    .and_then(|value| value.checked_add((digit - b'0') as u64))
            })
            .ok_or("Content-Length exceeds observation counter")?;
        return Ok(Framing::Length(count));
    }
    Ok(Framing::Absent)
}
struct Progress {
    consumed: usize,
    ended: bool,
}
enum BodySkipper {
    Fixed(u64),
    Chunked(Chunked),
    Close,
}
impl BodySkipper {
    fn consume(&mut self, input: &[u8]) -> Result<Progress, StopReason> {
        match self {
            Self::Fixed(remaining) => {
                let count = (*remaining).min(input.len() as u64) as usize;
                *remaining -= count as u64;
                Ok(Progress {
                    consumed: count,
                    ended: *remaining == 0,
                })
            }
            Self::Chunked(chunked) => chunked.consume(input),
            Self::Close => Ok(Progress {
                consumed: input.len(),
                ended: false,
            }),
        }
    }
}
enum ChunkPhase {
    Size(Line),
    Data(u64),
    DataCrlf(usize),
    Trailer(Line),
    Done,
}
struct Chunked {
    phase: ChunkPhase,
    metadata_bytes: usize,
    trailer_bytes: usize,
    trailer_fields: usize,
}
impl Default for Chunked {
    fn default() -> Self {
        Self {
            phase: ChunkPhase::Size(Line::default()),
            metadata_bytes: 0,
            trailer_bytes: 0,
            trailer_fields: 0,
        }
    }
}
#[derive(Default)]
struct Line {
    bytes: Vec<u8>,
}
impl Line {
    fn push(&mut self, byte: u8, limit: usize) -> Result<bool, StopReason> {
        if self.bytes.len() >= limit {
            return Err("Chunk line or trailer exceeds observation limit");
        }
        if self.bytes.last() == Some(&b'\r') && byte != b'\n'
            || byte == b'\n' && self.bytes.last() != Some(&b'\r')
        {
            return Err("Chunk framing requires CRLF");
        }
        if byte < 32 && !matches!(byte, b'\r' | b'\n' | b'\t') || byte == 127 {
            return Err("Invalid chunk metadata octet");
        }
        self.bytes.push(byte);
        Ok(self.bytes.ends_with(b"\r\n"))
    }
}
impl Chunked {
    fn metadata_byte(&mut self, trailer: bool) -> Result<(), StopReason> {
        self.metadata_bytes += 1;
        if self.metadata_bytes > MAX_CHUNK_METADATA {
            return Err("Chunk metadata budget exceeded");
        }
        if trailer {
            self.trailer_bytes += 1;
            if self.trailer_bytes > MAX_TRAILER_BYTES {
                return Err("Chunk trailer budget exceeded");
            }
        }
        Ok(())
    }
    fn consume(&mut self, mut input: &[u8]) -> Result<Progress, StopReason> {
        let total = input.len();
        loop {
            if matches!(self.phase, ChunkPhase::Done) {
                return Ok(Progress {
                    consumed: total - input.len(),
                    ended: true,
                });
            }
            if input.is_empty() {
                return Ok(Progress {
                    consumed: total,
                    ended: false,
                });
            }
            match std::mem::replace(&mut self.phase, ChunkPhase::Done) {
                ChunkPhase::Size(mut line) => {
                    self.metadata_byte(false)?;
                    let done = line.push(input[0], MAX_CHUNK_LINE)?;
                    input = &input[1..];
                    if done {
                        let value = &line.bytes[..line.bytes.len() - 2];
                        if value.contains(&b';') {
                            return Err("Chunk extensions are not observed");
                        }
                        if value.is_empty() {
                            return Err("Empty chunk size");
                        }
                        let count = value
                            .iter()
                            .try_fold(0u64, |count, byte| {
                                let digit = match byte {
                                    b'0'..=b'9' => Some(byte - b'0'),
                                    b'a'..=b'f' => Some(byte - b'a' + 10),
                                    b'A'..=b'F' => Some(byte - b'A' + 10),
                                    _ => None,
                                }?;
                                count.checked_mul(16)?.checked_add(digit as u64)
                            })
                            .ok_or("Invalid or overflowing chunk size")?;
                        self.phase = if count == 0 {
                            ChunkPhase::Trailer(Line::default())
                        } else {
                            ChunkPhase::Data(count)
                        };
                    } else {
                        self.phase = ChunkPhase::Size(line);
                    }
                }
                ChunkPhase::Data(remaining) => {
                    let count = remaining.min(input.len() as u64) as usize;
                    input = &input[count..];
                    self.phase = if count as u64 == remaining {
                        ChunkPhase::DataCrlf(0)
                    } else {
                        ChunkPhase::Data(remaining - count as u64)
                    };
                }
                ChunkPhase::DataCrlf(position) => {
                    self.metadata_byte(false)?;
                    if input[0] != b"\r\n"[position] {
                        return Err("Missing CRLF after chunk data");
                    }
                    input = &input[1..];
                    self.phase = if position == 1 {
                        ChunkPhase::Size(Line::default())
                    } else {
                        ChunkPhase::DataCrlf(1)
                    };
                }
                ChunkPhase::Trailer(mut line) => {
                    self.metadata_byte(true)?;
                    let done = line.push(input[0], MAX_TRAILER_BYTES)?;
                    input = &input[1..];
                    if done {
                        let value = &line.bytes[..line.bytes.len() - 2];
                        if value.is_empty() {
                            self.phase = ChunkPhase::Done;
                            continue;
                        }
                        self.trailer_fields += 1;
                        if self.trailer_fields > MAX_TRAILER_FIELDS {
                            return Err("Too many chunk trailer fields");
                        }
                        let colon = value
                            .iter()
                            .position(|byte| *byte == b':')
                            .ok_or("Invalid chunk trailer field")?;
                        if colon == 0 || !value[..colon].iter().copied().all(token_byte) {
                            return Err("Invalid chunk trailer name");
                        }
                        self.phase = ChunkPhase::Trailer(Line::default());
                    } else {
                        self.phase = ChunkPhase::Trailer(line);
                    }
                }
                ChunkPhase::Done => unreachable!("handled before consuming input"),
            }
        }
    }
}
fn token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(events: &mut Vec<SessionEvent>) -> impl FnMut(SessionEvent) -> bool + '_ {
        |event| {
            events.push(event);
            true
        }
    }
    fn tags(events: &[SessionEvent]) -> Vec<String> {
        events
            .iter()
            .map(|event| match event {
                SessionEvent::Request { sequence, .. } => format!("Q{sequence}"),
                SessionEvent::RequestEnd { sequence } => format!("q{sequence}"),
                SessionEvent::Response { sequence, .. } => format!("S{sequence}"),
                SessionEvent::ResponseEnd { sequence } => format!("s{sequence}"),
                SessionEvent::Stopped { .. } => "stop".into(),
            })
            .collect()
    }
    fn terminal(observer: &SessionObserver) {
        assert!(observer.request_finished());
        assert!(observer.response_finished());
    }
    fn get(observer: &mut SessionObserver, events: &mut Vec<SessionEvent>) {
        observer.observe_request(
            b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n",
            &mut collect(events),
        );
    }

    #[test]
    fn pipeline_skips_fixed_and_chunked_bodies_without_recognizing_forged_heads() {
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        let body = b"GET /forged HTTP/1.1\r\n\r\n";
        let mut requests = format!(
            "POST /fixed HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        requests.extend(body);
        requests.extend(b"POST /chunked HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n");
        requests.extend(format!("{:x}\r\n", body.len()).as_bytes());
        requests.extend(body);
        requests.extend(b"\r\n0\r\nX-Trailer: untouched\r\n\r\n");
        observer.observe_request(&requests, &mut collect(&mut events));
        let forged = b"HTTP/1.1 599 Forged\r\n\r\n";
        let mut responses = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            forged.len()
        )
        .into_bytes();
        responses.extend(forged);
        responses.extend(b"HTTP/1.1 201 Created\r\nTransfer-Encoding: CHUNKED\r\n\r\n");
        responses.extend(format!("{:X}\r\n", forged.len()).as_bytes());
        responses.extend(forged);
        responses.extend(b"\r\n0\r\nContent-Length: 999\r\n\r\n");
        observer.observe_response(&responses, &mut collect(&mut events));
        assert_eq!(
            tags(&events),
            ["Q0", "q0", "Q1", "q1", "S0", "s0", "S1", "s1"]
        );
        assert!(observer.pending.is_empty());
        assert!(!observer.request_finished());
        assert!(!observer.response_finished());
    }

    #[test]
    fn every_request_split_and_single_byte_response_preserve_boundaries() {
        let request=b"POST /one HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nGET\r\n3\r\nabc\r\n0\r\nX: y\r\n\r\nGET /two HTTP/1.1\r\n\r\n";
        let response=b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n\x00\xffa\r\n0\r\n\r\nHTTP/1.1 204 No Content\r\n\r\n";
        for split in 0..=request.len() {
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            observer.observe_request(&request[..split], &mut collect(&mut events));
            observer.observe_request(&request[split..], &mut collect(&mut events));
            for byte in response {
                observer.observe_response(std::slice::from_ref(byte), &mut collect(&mut events));
            }
            assert_eq!(
                tags(&events),
                ["Q0", "q0", "Q1", "q1", "S0", "s0", "S1", "s1"],
                "split {split}"
            );
        }
    }

    #[test]
    fn heads_preserve_raw_bytes_latin1_case_order_and_duplicates() {
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        let raw =
            b"GET / HTTP/1.0\r\nConnection: keep-alive\r\nX-Case: \xe9\r\nx-case: second\r\n\r\n";
        observer.observe_request(raw, &mut collect(&mut events));
        let SessionEvent::Request { head, .. } = &events[0] else {
            panic!("request")
        };
        assert_eq!(head.raw, raw);
        assert_eq!(head.version, "HTTP/1.0");
        assert_eq!(head.headers[1].name, "X-Case");
        assert_eq!(head.headers[1].value, "\u{e9}");
        assert_eq!(head.headers[2].name, "x-case");
        let response=b"HTTP/1.0 200 OK\r\nConnection: keep-alive\r\nContent-Length: 0\r\nX-V: \xff\r\nX-V: two\r\n\r\n";
        observer.observe_response(response, &mut collect(&mut events));
        let SessionEvent::Response { head, .. } = &events[2] else {
            panic!("response")
        };
        assert_eq!(head.raw, response);
        assert_eq!(head.headers[2].value, "\u{ff}");
        assert_eq!(head.headers[3].name, "X-V");
        assert!(!observer.request_finished());
    }

    #[test]
    fn head_interims_204_and_304_do_not_consume_following_response_bytes() {
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        observer.observe_request(
            b"HEAD / HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\n\r\n",
            &mut collect(&mut events),
        );
        observer.observe_response(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Early Hints\r\nLink: x\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\nHTTP/1.1 204 No Content\r\nContent-Length: 999\r\n\r\nHTTP/1.1 304 Not Modified\r\nContent-Length: 999\r\n\r\n",&mut collect(&mut events));
        assert_eq!(
            tags(&events),
            ["Q0", "q0", "Q1", "q1", "Q2", "q2", "S0", "s0", "S1", "s1", "S2", "s2"]
        );
    }

    #[test]
    fn status_205_uses_ordinary_framing_and_close_body_ends_only_at_eof() {
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        get(&mut observer, &mut events);
        observer.observe_response(
            b"HTTP/1.1 205 Reset Content\r\n\r\nHTTP/1.1 204 Forged\r\n\r\n",
            &mut collect(&mut events),
        );
        assert_eq!(tags(&events), ["Q0", "q0", "S0"]);
        observer.eof_response(&mut collect(&mut events));
        assert_eq!(tags(&events), ["Q0", "q0", "S0", "s0"]);
        terminal(&observer);
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        get(&mut observer, &mut events);
        get(&mut observer, &mut events);
        observer.observe_response(b"HTTP/1.1 205 Reset Content\r\nContent-Length: 3\r\n\r\nabcHTTP/1.1 204 No Content\r\n\r\n",&mut collect(&mut events));
        assert_eq!(
            tags(&events),
            ["Q0", "q0", "Q1", "q1", "S0", "s0", "S1", "s1"]
        );
    }

    #[test]
    fn upgrade_and_connect_finish_current_handshake_and_never_scan_payload() {
        for (request,response) in [
            (&b"CONNECT host:443 HTTP/1.1\r\n\r\n"[..],&b"HTTP/1.1 200 Connected\r\nContent-Length: invalid\r\nTransfer-Encoding: garbage\r\n\r\n"[..]),
            (&b"GET / HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n"[..],&b"HTTP/1.1 101 Switching Protocols\r\n\r\n"[..]),
        ] {
            let mut observer=SessionObserver::default(); let mut events=Vec::new();
            observer.observe_request(request,&mut collect(&mut events));
            observer.observe_request(b"GET /eager-payload HTTP/1.1\r\n\r\n",&mut collect(&mut events));
            assert!(observer.request_finished());assert!(!observer.response_finished());
            observer.observe_response(response,&mut collect(&mut events));
            observer.observe_response(b"HTTP/1.1 200 Forged\r\n\r\n",&mut collect(&mut events));
            assert_eq!(tags(&events),["Q0","q0","S0","s0"]);terminal(&observer);
        }
    }

    #[test]
    fn rejected_connect_resumes_only_if_no_eager_payload_was_discarded() {
        for eager in [false, true] {
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            observer.observe_request(
                b"CONNECT host:443 HTTP/1.1\r\n\r\n",
                &mut collect(&mut events),
            );
            if eager {
                observer
                    .observe_request(b"GET /opaque HTTP/1.1\r\n\r\n", &mut collect(&mut events));
            }
            observer.observe_response(
                b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n",
                &mut collect(&mut events),
            );
            get(&mut observer, &mut events);
            if eager {
                assert_eq!(tags(&events), ["Q0", "q0", "S0", "s0"]);
                terminal(&observer);
            } else {
                assert_eq!(tags(&events), ["Q0", "q0", "S0", "s0", "Q1", "q1"]);
            }
        }
    }

    #[test]
    fn early_final_retains_header_but_never_completes_the_exchange() {
        for status in [200, 204, 413] {
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            observer.observe_request(
                b"POST / HTTP/1.1\r\nContent-Length: 20\r\n\r\nabc",
                &mut collect(&mut events),
            );
            observer.observe_response(
                format!("HTTP/1.1 {status} Final\r\nContent-Length: 0\r\n\r\n").as_bytes(),
                &mut collect(&mut events),
            );
            assert_eq!(tags(&events), ["Q0", "S0", "stop"]);
            terminal(&observer);
        }
    }

    #[test]
    fn clean_request_half_close_allows_outstanding_responses() {
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        get(&mut observer, &mut events);
        observer.eof_request(&mut collect(&mut events));
        assert!(observer.request_finished());
        assert!(!observer.response_finished());
        observer.observe_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
            &mut collect(&mut events),
        );
        assert_eq!(tags(&events), ["Q0", "q0", "S0", "s0"]);
        terminal(&observer);
    }

    #[test]
    fn truncated_requests_and_responses_do_not_emit_body_end() {
        for request in [
            &b"GET / HTTP/1.1\r\nX: partial"[..],
            &b"POST / HTTP/1.1\r\nContent-Length: 2\r\n\r\nx"[..],
            &b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n"[..],
            &b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx\r"[..],
        ] {
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            observer.observe_request(request, &mut collect(&mut events));
            observer.eof_request(&mut collect(&mut events));
            assert!(tags(&events).ends_with(&["stop".into()]));
            assert!(!events
                .iter()
                .any(|event| matches!(event, SessionEvent::RequestEnd { .. })));
            terminal(&observer);
        }
        for response in [
            &b"HTTP/1.1 200 OK\r\nX: partial"[..],
            &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nx"[..],
            &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n"[..],
        ] {
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            get(&mut observer, &mut events);
            observer.observe_response(response, &mut collect(&mut events));
            observer.eof_response(&mut collect(&mut events));
            assert!(tags(&events).ends_with(&["stop".into()]));
            assert!(!events
                .iter()
                .any(|event| matches!(event, SessionEvent::ResponseEnd { .. })));
            terminal(&observer);
        }
    }

    #[test]
    fn ambiguous_or_unsupported_framing_stops_after_recording_the_head() {
        for fields in [
            "Content-Length: 0\r\nContent-Length: 0\r\n",
            "Content-Length: +1\r\n",
            "Content-Length: -1\r\n",
            "Content-Length: 0, 0\r\n",
            "Content-Length: 18446744073709551616\r\n",
            "Content-Length: 0\r\nTransfer-Encoding: chunked\r\n",
            "Transfer-Encoding: gzip, chunked\r\n",
            "Transfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n",
        ] {
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            observer.observe_request(
                format!("POST / HTTP/1.1\r\n{fields}\r\n").as_bytes(),
                &mut collect(&mut events),
            );
            assert_eq!(tags(&events), ["Q0", "stop"], "{fields}");
            terminal(&observer);
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            get(&mut observer, &mut events);
            observer.observe_response(
                format!("HTTP/1.1 200 OK\r\n{fields}\r\n").as_bytes(),
                &mut collect(&mut events),
            );
            assert_eq!(tags(&events), ["Q0", "q0", "S0", "stop"], "{fields}");
            terminal(&observer);
        }
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        observer.observe_request(
            b"POST / HTTP/1.0\r\nConnection: keep-alive\r\nTransfer-Encoding: chunked\r\n\r\n",
            &mut collect(&mut events),
        );
        assert_eq!(tags(&events), ["Q0", "stop"]);
        terminal(&observer);
    }

    #[test]
    fn chunk_syntax_and_unsupported_extensions_have_terminal_fixed_reasons() {
        for (body, reason) in [
            (&b"g\r\n"[..], "Invalid or overflowing chunk size"),
            (
                &b"10000000000000000\r\n"[..],
                "Invalid or overflowing chunk size",
            ),
            (
                &b"1;foo=bar\r\nx\r\n0\r\n\r\n"[..],
                "Chunk extensions are not observed",
            ),
            (&b"1\nx"[..], "Chunk framing requires CRLF"),
            (&b"1\r\nx\n"[..], "Missing CRLF after chunk data"),
            (
                &b"0\r\nBad trailer: x\r\n\r\n"[..],
                "Invalid chunk trailer name",
            ),
            (&b"0\r\nno-colon\r\n\r\n"[..], "Invalid chunk trailer field"),
            (&b"0\r\nX: \x00\r\n\r\n"[..], "Invalid chunk metadata octet"),
        ] {
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            observer.observe_request(
                b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
                &mut collect(&mut events),
            );
            for byte in body {
                observer.observe_request(std::slice::from_ref(byte), &mut collect(&mut events));
            }
            assert_eq!(tags(&events), ["Q0", "stop"]);
            assert!(
                matches!(events.last(),Some(SessionEvent::Stopped{reason:actual}) if *actual==reason),
                "{reason}"
            );
            terminal(&observer);
        }
    }

    #[test]
    fn chunk_metadata_limits_bound_size_lines_trailers_and_total_work() {
        let mut cases = Vec::new();
        cases.push(vec![b'0'; MAX_CHUNK_LINE + 1]);
        let mut trailers = b"0\r\n".to_vec();
        for _ in 0..=MAX_TRAILER_FIELDS {
            trailers.extend(b"X: y\r\n");
        }
        trailers.extend(b"\r\n");
        cases.push(trailers);
        let mut metadata = Vec::new();
        for _ in 0..=MAX_CHUNK_METADATA / 5 {
            metadata.extend(b"1\r\nx\r\n");
        }
        cases.push(metadata);
        let mut huge_trailer = b"0\r\nX: ".to_vec();
        huge_trailer.extend(vec![b'a'; MAX_TRAILER_BYTES]);
        cases.push(huge_trailer);
        for body in cases {
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            observer.observe_request(
                b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
                &mut collect(&mut events),
            );
            observer.observe_request(&body, &mut collect(&mut events));
            assert_eq!(tags(&events), ["Q0", "stop"]);
            terminal(&observer);
        }
    }

    #[test]
    fn at_most_eight_unfinished_exchanges_including_an_active_response() {
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        for _ in 0..MAX_PENDING {
            get(&mut observer, &mut events);
        }
        observer.observe_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nx",
            &mut collect(&mut events),
        );
        assert_eq!(observer.pending.len(), MAX_PENDING);
        get(&mut observer, &mut events);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SessionEvent::Request { .. }))
                .count(),
            MAX_PENDING
        );
        assert!(matches!(
            events.last(),
            Some(SessionEvent::Stopped {
                reason: "Pending request limit exceeded"
            })
        ));
        terminal(&observer);
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        for _ in 0..MAX_PENDING {
            get(&mut observer, &mut events);
        }
        observer.observe_response(
            b"HTTP/1.1 204 No Content\r\n\r\n",
            &mut collect(&mut events),
        );
        get(&mut observer, &mut events);
        assert_eq!(observer.pending.len(), MAX_PENDING);
        assert!(matches!(
            events[events.len() - 2],
            SessionEvent::Request { sequence: 8, .. }
        ));
    }

    #[test]
    fn a_rejected_event_permanently_halts_both_directions() {
        for rejected in 0..4 {
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            let mut emit = |event| {
                events.push(event);
                events.len() - 1 != rejected
            };
            observer.observe_request(b"GET / HTTP/1.1\r\n\r\n", &mut emit);
            observer.observe_response(b"HTTP/1.1 204 No Content\r\n\r\n", &mut emit);
            observer.observe_request(b"GET /ignored HTTP/1.1\r\n\r\n", &mut emit);
            observer.observe_response(b"HTTP/1.1 204 Ignored\r\n\r\n", &mut emit);
            observer.eof_request(&mut emit);
            observer.eof_response(&mut emit);
            terminal(&observer);
            assert_eq!(events.len(), rejected + 1);
            assert!(observer.pending.is_empty());
        }
    }

    #[test]
    fn unsolicited_server_bytes_or_tls_and_h2_never_become_http_sessions() {
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        observer.observe_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
            &mut collect(&mut events),
        );
        get(&mut observer, &mut events);
        assert_eq!(tags(&events), ["stop"]);
        terminal(&observer);
        for bytes in [
            &b"\x16\x03\x01\x00\xffHTTP/1.1 200\r\n\r\n"[..],
            &b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"[..],
            &b"\r\n"[..],
        ] {
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            observer.observe_request(bytes, &mut collect(&mut events));
            get(&mut observer, &mut events);
            assert_eq!(tags(&events), ["stop"]);
            terminal(&observer);
        }
    }

    #[test]
    fn connection_close_and_http10_default_do_not_associate_following_messages() {
        for request in [
            &b"GET / HTTP/1.0\r\n\r\n"[..],
            &b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n"[..],
        ] {
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            observer.observe_request(request, &mut collect(&mut events));
            get(&mut observer, &mut events);
            observer.observe_response(
                b"HTTP/1.1 204 No Content\r\n\r\n",
                &mut collect(&mut events),
            );
            assert_eq!(tags(&events), ["Q0", "q0", "S0", "s0"]);
            terminal(&observer);
        }
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        get(&mut observer, &mut events);
        get(&mut observer, &mut events);
        observer.observe_response(
            b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\nHTTP/1.1 204 Ignored\r\n\r\n",
            &mut collect(&mut events),
        );
        assert_eq!(tags(&events), ["Q0", "q0", "Q1", "q1", "S0", "s0", "stop"]);
        terminal(&observer);
    }

    #[test]
    fn interim_count_and_bytes_are_bounded_and_reset_between_final_responses() {
        let interim = b"HTTP/1.1 103 Early Hints\r\n\r\n";
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        get(&mut observer, &mut events);
        get(&mut observer, &mut events);
        for _ in 0..2 {
            for _ in 0..MAX_INTERIM {
                observer.observe_response(interim, &mut collect(&mut events));
            }
            observer.observe_response(
                b"HTTP/1.1 204 No Content\r\n\r\n",
                &mut collect(&mut events),
            );
        }
        assert_eq!(
            tags(&events),
            ["Q0", "q0", "Q1", "q1", "S0", "s0", "S1", "s1"]
        );
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        get(&mut observer, &mut events);
        for _ in 0..=MAX_INTERIM {
            observer.observe_response(interim, &mut collect(&mut events));
        }
        assert_eq!(tags(&events), ["Q0", "q0", "stop"]);
        terminal(&observer);
        let large = format!(
            "HTTP/1.1 103 Early Hints\r\nX: {}\r\n\r\n",
            "x".repeat(17000)
        );
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        get(&mut observer, &mut events);
        for _ in 0..4 {
            observer.observe_response(large.as_bytes(), &mut collect(&mut events));
        }
        assert_eq!(tags(&events), ["Q0", "q0", "stop"]);
        terminal(&observer);
    }

    #[test]
    fn each_head_has_64k_bytes_and_256_field_limits() {
        for fields in [256, 257] {
            let mut request = b"GET / HTTP/1.1\r\n".to_vec();
            for _ in 0..fields {
                request.extend(b"X: y\r\n");
            }
            request.extend(b"\r\n");
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            observer.observe_request(&request, &mut collect(&mut events));
            if fields == 256 {
                assert_eq!(tags(&events), ["Q0", "q0"]);
            } else {
                assert_eq!(tags(&events), ["stop"]);
                terminal(&observer);
            }
        }
        let prefix = b"GET / HTTP/1.1\r\nX: ";
        let suffix = b"\r\n\r\n";
        for bytes in [65536, 65537] {
            let mut request = prefix.to_vec();
            request.extend(vec![b'a'; bytes - prefix.len() - suffix.len()]);
            request.extend(suffix);
            let mut observer = SessionObserver::default();
            let mut events = Vec::new();
            observer.observe_request(&request, &mut collect(&mut events));
            if bytes == 65536 {
                assert_eq!(tags(&events), ["Q0", "q0"]);
            } else {
                assert_eq!(tags(&events), ["stop"]);
                terminal(&observer);
            }
        }
    }

    #[test]
    fn zero_length_and_empty_observations_do_not_allocate_or_advance() {
        let mut observer = SessionObserver::default();
        let mut events = Vec::new();
        observer.observe_request(b"", &mut collect(&mut events));
        observer.observe_response(b"", &mut collect(&mut events));
        assert!(events.is_empty());
        assert!(!observer.request_finished());
        observer.observe_request(
            b"POST / HTTP/1.1\r\nContent-Length: 000\r\n\r\n",
            &mut collect(&mut events),
        );
        observer.observe_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
            &mut collect(&mut events),
        );
        assert_eq!(tags(&events), ["Q0", "q0", "S0", "s0"]);
        assert!(observer.pending.is_empty());
    }
}
