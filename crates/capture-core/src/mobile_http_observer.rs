//! Passive, bounded inspection of the first HTTP/1 exchange in a TCP tunnel.
//! No body bytes or subsequent requests are retained. Failure only stops
//! observation: the caller must continue forwarding the original byte stream.
use crate::model::Header;

pub(crate) const MAX_HEAD_BYTES: usize = 64 * 1024;
const MAX_HEADERS: usize = 256;
const MAX_INTERIM_RESPONSES: usize = 8;
const HTTP2_PREFACE_LINE: &[u8] = b"PRI * HTTP/2.0";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RequestHead {
    pub method: String,
    pub target: String,
    pub version: String,
    pub headers: Vec<Header>,
    pub raw: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ResponseHead {
    pub status: u16,
    pub version: String,
    pub headers: Vec<Header>,
    pub raw: Vec<u8>,
}

pub(crate) struct HeadObserver {
    request: HeadCollector,
    response: HeadCollector,
    request_recognized: bool,
    interim_responses: usize,
}

impl Default for HeadObserver {
    fn default() -> Self {
        Self {
            request: HeadCollector::new(Direction::Request),
            response: HeadCollector::new(Direction::Response),
            request_recognized: false,
            interim_responses: 0,
        }
    }
}

impl HeadObserver {
    pub(crate) fn observe_request(&mut self, bytes: &[u8]) -> Option<RequestHead> {
        if self.request.done {
            return None;
        }
        let collected = self.request.push(bytes);
        match collected {
            Collected::Head { raw, .. } => {
                self.request.stop();
                if let Some(head) = parse_request(raw) {
                    self.request_recognized = true;
                    Some(head)
                } else {
                    self.response.stop();
                    None
                }
            }
            Collected::Disabled => {
                self.response.stop();
                None
            }
            Collected::Partial => None,
        }
    }

    pub(crate) fn observe_response(&mut self, mut bytes: &[u8]) -> Option<ResponseHead> {
        // Starting later inside an already-observed server stream could mistake
        // payload for a status line. Early server bytes disable this direction.
        if !self.request_recognized {
            if !bytes.is_empty() {
                self.response.stop();
            }
            return None;
        }
        while !bytes.is_empty() && !self.response.done {
            match self.response.push(bytes) {
                Collected::Head { consumed, raw } => {
                    bytes = &bytes[consumed..];
                    let Some(head) = parse_response(raw) else {
                        self.response.stop();
                        return None;
                    };
                    if (100..200).contains(&head.status) && head.status != 101 {
                        self.interim_responses += 1;
                        if self.interim_responses > MAX_INTERIM_RESPONSES {
                            self.response.stop();
                            return None;
                        }
                    } else {
                        self.response.stop();
                        return Some(head);
                    }
                }
                Collected::Partial | Collected::Disabled => return None,
            }
        }
        None
    }

    pub(crate) fn request_finished(&self) -> bool {
        self.request.done
    }
    pub(crate) fn response_finished(&self) -> bool {
        self.response.done
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Request,
    Response,
}

struct HeadCollector {
    direction: Direction,
    buffer: Vec<u8>,
    total_bytes: usize,
    first_line_end: Option<usize>,
    done: bool,
}

enum Collected {
    Partial,
    Disabled,
    Head { consumed: usize, raw: Vec<u8> },
}

impl HeadCollector {
    fn new(direction: Direction) -> Self {
        Self {
            direction,
            buffer: Vec::new(),
            total_bytes: 0,
            first_line_end: None,
            done: false,
        }
    }

    fn stop(&mut self) {
        self.done = true;
        self.buffer = Vec::new();
    }

    fn push(&mut self, input: &[u8]) -> Collected {
        if self.done {
            return Collected::Disabled;
        }
        for (index, &byte) in input.iter().enumerate() {
            if self.total_bytes >= MAX_HEAD_BYTES || !self.valid_next(byte) {
                self.stop();
                return Collected::Disabled;
            }
            self.buffer.push(byte);
            self.total_bytes += 1;
            // The only delimiter search is this constant-size suffix comparison.
            // Feeding one byte at a time must not rescan a growing header block.
            if self.first_line_end.is_none() && self.buffer.ends_with(b"\r\n") {
                if !valid_first_line(&self.buffer[..self.buffer.len() - 2], self.direction) {
                    self.stop();
                    return Collected::Disabled;
                }
                self.first_line_end = Some(self.buffer.len());
            }
            if matches!(self.direction, Direction::Request)
                && self.buffer.len() == HTTP2_PREFACE_LINE.len()
                && self.buffer == HTTP2_PREFACE_LINE
            {
                self.stop();
                return Collected::Disabled;
            }
            if self.buffer.ends_with(b"\r\n\r\n") {
                self.first_line_end = None;
                return Collected::Head {
                    consumed: index + 1,
                    raw: std::mem::take(&mut self.buffer),
                };
            }
        }
        Collected::Partial
    }

    fn valid_next(&self, byte: u8) -> bool {
        if self.buffer.last() == Some(&b'\r') && byte != b'\n' {
            return false;
        }
        if byte == b'\n' && self.buffer.last() != Some(&b'\r') {
            return false;
        }
        if (byte < 32 && !matches!(byte, b'\t' | b'\r' | b'\n')) || byte == 127 {
            return false;
        }
        if self.buffer.is_empty() {
            return match self.direction {
                Direction::Request => token_byte(byte),
                Direction::Response => byte == b'H',
            };
        }
        if matches!(self.direction, Direction::Response) && self.buffer.len() < 7 {
            return b"HTTP/1."[self.buffer.len()] == byte;
        }
        if matches!(self.direction, Direction::Response) && self.buffer.len() == 7 {
            return matches!(byte, b'0' | b'1');
        }
        true
    }
}

fn token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn valid_first_line(line: &[u8], direction: Direction) -> bool {
    match direction {
        Direction::Request => {
            let mut fields = line.split(|byte| *byte == b' ');
            let (Some(method), Some(target), Some(version)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return false;
            };
            fields.next().is_none()
                && !method.is_empty()
                && method.iter().copied().all(token_byte)
                && !target.is_empty()
                && target.iter().all(|byte| (33..=126).contains(byte))
                && matches!(version, b"HTTP/1.0" | b"HTTP/1.1")
        }
        Direction::Response => {
            if line.len() < 13
                || !matches!(&line[..8], b"HTTP/1.0" | b"HTTP/1.1")
                || line[8] != b' '
                || line[12] != b' '
            {
                return false;
            }
            line[9..12].iter().all(u8::is_ascii_digit) && (b'1'..=b'5').contains(&line[9])
        }
    }
}

fn valid_header_lines(raw: &[u8]) -> bool {
    let Some(first) = raw.windows(2).position(|pair| pair == b"\r\n") else {
        return false;
    };
    let mut remaining = &raw[first + 2..];
    let mut count = 0;
    while !remaining.is_empty() {
        let Some(end) = remaining.windows(2).position(|pair| pair == b"\r\n") else {
            return false;
        };
        let line = &remaining[..end];
        remaining = &remaining[end + 2..];
        if line.is_empty() {
            return remaining.is_empty();
        }
        count += 1;
        if count > MAX_HEADERS {
            return false;
        }
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            return false;
        };
        if colon == 0 || !line[..colon].iter().copied().all(token_byte) {
            return false;
        }
    }
    false
}

fn parse_request(raw: Vec<u8>) -> Option<RequestHead> {
    if !valid_header_lines(&raw) {
        return None;
    }
    let mut storage = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut parsed = httparse::Request::new(&mut storage);
    if parsed.parse(&raw).ok()? != httparse::Status::Complete(raw.len()) {
        return None;
    }
    let version = match parsed.version? {
        0 => "HTTP/1.0",
        1 => "HTTP/1.1",
        _ => return None,
    };
    let head = RequestHead {
        method: parsed.method?.into(),
        target: parsed.path?.into(),
        version: version.into(),
        headers: parsed
            .headers
            .iter()
            .map(|header| Header {
                disabled: false,
                name: header.name.into(),
                value: crate::http1::latin1(header.value),
            })
            .collect(),
        raw: Vec::new(),
    };
    Some(RequestHead { raw, ..head })
}

fn parse_response(raw: Vec<u8>) -> Option<ResponseHead> {
    if !valid_header_lines(&raw) {
        return None;
    }
    let mut storage = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut parsed = httparse::Response::new(&mut storage);
    if parsed.parse(&raw).ok()? != httparse::Status::Complete(raw.len()) {
        return None;
    }
    let version = match parsed.version? {
        0 => "HTTP/1.0",
        1 => "HTTP/1.1",
        _ => return None,
    };
    let head = ResponseHead {
        status: parsed.code?,
        version: version.into(),
        headers: parsed
            .headers
            .iter()
            .map(|header| Header {
                disabled: false,
                name: header.name.into(),
                value: crate::http1::latin1(header.value),
            })
            .collect(),
        raw: Vec::new(),
    };
    Some(ResponseHead { raw, ..head })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recognized() -> HeadObserver {
        let mut observer = HeadObserver::default();
        observer
            .observe_request(b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n")
            .unwrap();
        observer
    }

    #[test]
    fn first_heads_preserve_duplicates_case_latin1_and_exact_raw_bytes() {
        let mut observer = HeadObserver::default();
        let request = b"POST /path?q=one HTTP/1.1\r\nHost: example.test\r\nX-Case: one\r\nx-case: two\r\nX-Byte: \xe9\xff\r\nContent-Length: 99\r\n\r\n";
        let mut input = request.to_vec();
        input.extend_from_slice(b"body that is not retained");
        let head = observer.observe_request(&input).unwrap();
        assert_eq!(
            (
                head.method.as_str(),
                head.target.as_str(),
                head.version.as_str()
            ),
            ("POST", "/path?q=one", "HTTP/1.1")
        );
        assert_eq!(head.raw, request);
        assert_eq!(head.headers.len(), 5);
        assert_eq!(
            (head.headers[1].name.as_str(), head.headers[2].name.as_str()),
            ("X-Case", "x-case")
        );
        assert_eq!(head.headers[3].value, "\u{e9}\u{ff}");
        let raw =
            b"HTTP/1.1 200 Fine\r\nSet-Cookie: a=1\r\nset-cookie: b=2\r\nX-Byte: \x80\r\n\r\n";
        let head = observer.observe_response(raw).unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(head.raw, raw);
        assert_eq!(head.headers[2].value, "\u{80}");
        assert_eq!(head.headers[1].name, "set-cookie");
        assert!(observer.request_finished());
        assert!(observer.response_finished());
        assert!(observer.request.buffer.is_empty());
        assert!(observer.response.buffer.is_empty());
    }

    #[test]
    fn fragmentation_at_every_boundary_and_byte_at_a_time_has_one_event() {
        let request = b"GET /slow HTTP/1.0\r\nX-One: 1\r\nX-One: 2\r\n\r\n";
        for split in 0..=request.len() {
            let mut observer = HeadObserver::default();
            let first = observer.observe_request(&request[..split]);
            let second = observer.observe_request(&request[split..]);
            assert_eq!(
                usize::from(first.is_some()) + usize::from(second.is_some()),
                1
            );
            let head = first.or(second).unwrap();
            assert_eq!(head.version, "HTTP/1.0");
            assert_eq!(head.raw, request);
        }
        let mut observer = HeadObserver::default();
        let mut events = 0;
        for byte in request {
            events += usize::from(
                observer
                    .observe_request(std::slice::from_ref(byte))
                    .is_some(),
            );
        }
        assert_eq!(events, 1);
        assert_eq!(observer.request.total_bytes, request.len());
        let response = b"HTTP/1.0 204 Empty\r\nX-One: 1\r\n\r\n";
        for split in 0..=response.len() {
            let mut observer = recognized();
            let first = observer.observe_response(&response[..split]);
            let second = observer.observe_response(&response[split..]);
            assert_eq!(
                usize::from(first.is_some()) + usize::from(second.is_some()),
                1
            );
            assert_eq!(first.or(second).unwrap().raw, response);
        }
    }

    #[test]
    fn request_and_response_body_or_followup_requests_never_become_heads() {
        let mut observer = HeadObserver::default();
        let first = b"POST /one HTTP/1.1\r\nContent-Length: 200\r\n\r\n";
        let mut bytes = first.to_vec();
        bytes.extend_from_slice(b"GET /forged HTTP/1.1\r\n\r\n");
        assert_eq!(observer.observe_request(&bytes).unwrap().target, "/one");
        assert!(observer
            .observe_request(b"GET /later HTTP/1.1\r\n\r\n")
            .is_none());
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\n";
        let mut bytes = response.to_vec();
        bytes.extend_from_slice(b"HTTP/1.1 500 Forged\r\n\r\n");
        assert_eq!(observer.observe_response(&bytes).unwrap().status, 200);
        assert!(observer
            .observe_response(b"HTTP/1.1 404 Later\r\n\r\n")
            .is_none());
        assert!(observer.request.buffer.is_empty());
        assert!(observer.response.buffer.is_empty());
        assert_eq!(observer.request.total_bytes, first.len());
        assert_eq!(observer.response.total_bytes, response.len());
    }

    #[test]
    fn interim_responses_share_a_budget_and_switching_protocols_is_final() {
        let mut observer = recognized();
        let mut bytes =
            b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Early Hints\r\nLink: </one>\r\n\r\n"
                .to_vec();
        let final_head = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n";
        bytes.extend_from_slice(final_head);
        bytes.extend_from_slice(b"OK");
        let head = observer.observe_response(&bytes).unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(head.raw, final_head);
        assert_eq!(observer.interim_responses, 2);
        let mut upgrade = recognized();
        let head=upgrade.observe_response(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\nHTTP/1.1 500 Fake\r\n\r\n").unwrap();
        assert_eq!(head.status, 101);
        assert!(upgrade.response_finished());
        assert!(upgrade
            .observe_response(b"HTTP/1.1 200 Fake\r\n\r\n")
            .is_none());
    }

    #[test]
    fn interim_responses_across_calls_cannot_flood_events_or_reset_limits() {
        let mut observer = recognized();
        for _ in 0..MAX_INTERIM_RESPONSES {
            assert!(observer
                .observe_response(b"HTTP/1.1 100 Continue\r\n\r\n")
                .is_none());
            assert!(!observer.response_finished());
        }
        assert_eq!(
            observer
                .observe_response(b"HTTP/1.1 200 OK\r\n\r\n")
                .unwrap()
                .status,
            200
        );
        let mut too_many = recognized();
        for _ in 0..=MAX_INTERIM_RESPONSES {
            assert!(too_many
                .observe_response(b"HTTP/1.1 103 Hint\r\n\r\n")
                .is_none());
        }
        assert!(too_many.response_finished());
        assert!(too_many
            .observe_response(b"HTTP/1.1 200 OK\r\n\r\n")
            .is_none());
        let mut total = recognized();
        let prefix = b"HTTP/1.1 103 Hint\r\nX-Padding: ";
        let suffix = b"\r\n\r\n";
        let mut large = prefix.to_vec();
        large.resize(40_000 - suffix.len(), b'a');
        large.extend_from_slice(suffix);
        assert!(total.observe_response(&large).is_none());
        assert!(!total.response_finished());
        assert!(total.observe_response(&large).is_none());
        assert!(total.response_finished());
        assert!(total.response.buffer.is_empty());
        assert_eq!(total.response.total_bytes, MAX_HEAD_BYTES);
    }

    #[test]
    fn exact_byte_budget_and_header_count_limits_are_enforced() {
        let prefix = b"GET / HTTP/1.1\r\nX-Pad: ";
        let suffix = b"\r\n\r\n";
        for length in [MAX_HEAD_BYTES, MAX_HEAD_BYTES + 1] {
            let mut head = prefix.to_vec();
            head.resize(length - suffix.len(), b'a');
            head.extend_from_slice(suffix);
            let mut observer = HeadObserver::default();
            let output = observer.observe_request(&head);
            assert_eq!(output.is_some(), length == MAX_HEAD_BYTES);
            assert!(observer.request_finished());
            assert!(observer.request.buffer.is_empty());
            assert!(observer.request.total_bytes <= MAX_HEAD_BYTES);
        }
        for count in [MAX_HEADERS, MAX_HEADERS + 1] {
            let mut head = b"GET / HTTP/1.1\r\n".to_vec();
            for _ in 0..count {
                head.extend_from_slice(b"X: a\r\n");
            }
            head.extend_from_slice(b"\r\n");
            let mut observer = HeadObserver::default();
            assert_eq!(
                observer.observe_request(&head).is_some(),
                count == MAX_HEADERS
            );
            assert!(observer.request_finished());
        }
    }

    #[test]
    fn malformed_requests_disable_observation_permanently_without_results() {
        for invalid in [
            &b"GET / HTTP/1.1\n\n"[..],
            &b"GET / HTTP/1.1\rX"[..],
            &b"GET / HTTP/1.1\r\n folded: one\r\n\r\n"[..],
            &b"GET / HTTP/1.1\r\nX : a\r\n\r\n"[..],
            &b"GET / HTTP/1.1\r\nMissing-Colon\r\n\r\n"[..],
            &b"GET / HTTP/1.1\r\nX: \x00\r\n\r\n"[..],
            &b"GET / HTTP/1.1\r\nX: \x7f\r\n\r\n"[..],
            &b"GET  / HTTP/1.1\r\n\r\n"[..],
            &b"GET / HTTP/1.2\r\n\r\n"[..],
            &b"GET / HTTP/1.1 extra\r\n\r\n"[..],
            &b"GET\t/ HTTP/1.1\r\n\r\n"[..],
            &b" GET / HTTP/1.1\r\n\r\n"[..],
            &b"GET /\xff HTTP/1.1\r\n\r\n"[..],
        ] {
            let mut observer = HeadObserver::default();
            assert!(observer.observe_request(invalid).is_none(), "{invalid:?}");
            assert!(observer.request_finished(), "{invalid:?}");
            assert!(observer.response_finished());
            assert!(observer
                .observe_request(b"GET /good HTTP/1.1\r\n\r\n")
                .is_none());
            assert!(observer
                .observe_response(b"HTTP/1.1 200 OK\r\n\r\n")
                .is_none());
        }
    }

    #[test]
    fn malformed_responses_and_non_http_payloads_are_never_reinterpreted() {
        for invalid in [
            &b"HTTP/2.0 200 OK\r\n\r\n"[..],
            &b"HTTP/1.1 000 Invalid\r\n\r\n"[..],
            &b"HTTP/1.1 600 Invalid\r\n\r\n"[..],
            &b"HTTP/1.1 20x Invalid\r\n\r\n"[..],
            &b"HTTP/1.1 200\r\n\r\n"[..],
            &b"HTTP/1.1 200 OK\n\n"[..],
            &b"HTTP/1.1 200 OK\r\n Folded: no\r\n\r\n"[..],
            &b"SSH-2.0-banner\r\n"[..],
            &b"\x16\x03\x03"[..],
        ] {
            let mut observer = recognized();
            assert!(observer.observe_response(invalid).is_none(), "{invalid:?}");
            assert!(observer.response_finished(), "{invalid:?}");
            assert!(observer.response.buffer.is_empty());
            assert!(observer
                .observe_response(b"HTTP/1.1 200 Later\r\n\r\n")
                .is_none());
        }
    }

    #[test]
    fn tls_and_http2_prefaces_stop_early_and_empty_input_has_no_effect() {
        let mut empty = HeadObserver::default();
        assert!(empty.observe_request(&[]).is_none());
        assert!(empty.observe_response(&[]).is_none());
        assert!(!empty.request_finished());
        assert!(!empty.response_finished());
        for bytes in [
            &b"\x16\x03\x01"[..],
            &b"PRI * HTTP/2.0"[..],
            &b"\x00\x00\x12\x04"[..],
        ] {
            let mut observer = HeadObserver::default();
            assert!(observer.observe_request(bytes).is_none());
            assert!(observer.request_finished());
            assert!(observer.response_finished());
            assert!(observer.request.buffer.is_empty());
            assert!(observer.request.total_bytes <= HTTP2_PREFACE_LINE.len());
        }
    }

    #[test]
    fn server_bytes_before_a_recognized_request_disable_that_direction() {
        let mut observer = HeadObserver::default();
        assert!(observer
            .observe_response(b"HTTP/1.1 200 Early\r\n\r\n")
            .is_none());
        assert!(observer.response_finished());
        assert!(observer
            .observe_request(b"GET / HTTP/1.1\r\n\r\n")
            .is_some());
        assert!(observer
            .observe_response(b"HTTP/1.1 200 Later\r\n\r\n")
            .is_none());
        let mut partial = HeadObserver::default();
        partial.observe_request(b"GET / HTTP/1.1\r\n");
        partial.observe_response(b"H");
        assert!(partial.response_finished());
        assert!(partial.observe_request(b"\r\n").is_some());
    }

    #[test]
    fn a_large_body_in_the_same_read_does_not_consume_header_budget() {
        let head = b"POST / HTTP/1.1\r\nContent-Length: 1048576\r\n\r\n";
        let mut bytes = head.to_vec();
        bytes.resize(head.len() + 1024 * 1024, b'x');
        let mut observer = HeadObserver::default();
        assert_eq!(observer.observe_request(&bytes).unwrap().raw, head);
        assert_eq!(observer.request.total_bytes, head.len());
        assert!(observer.request.buffer.capacity() == 0);
        let head = b"HTTP/1.1 200 OK\r\nContent-Length: 1048576\r\n\r\n";
        let mut bytes = head.to_vec();
        bytes.resize(head.len() + 1024 * 1024, b'x');
        assert_eq!(observer.observe_response(&bytes).unwrap().raw, head);
        assert_eq!(observer.response.total_bytes, head.len());
        assert!(observer.response.buffer.capacity() == 0);
    }
}
