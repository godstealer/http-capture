//! Fixed local-only traffic used by the isolated Android LAN regression.
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use std::net::Ipv4Addr;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Ready {
    pub schema_version: u8,
    pub run_id: String,
    pub pairing_uri: String,
    pub host: Ipv4Addr,
    pub gateway_port: u16,
    pub http_port: u16,
    pub udp_port: u16,
    pub dns_wire_port: u16,
    pub expected_request_body_base64: String,
    pub expected_response_body_base64: String,
}

#[derive(Clone)]
pub(super) struct Traffic {
    pub run_id: String,
    pub host: Ipv4Addr,
    pub http_port: u16,
    pub request_body: Vec<u8>,
    pub response_body: Vec<u8>,
}
impl Traffic {
    pub fn new(run_id: String, host: Ipv4Addr, http_port: u16) -> Result<Self> {
        ensure!(
            run_id.len() == 32 && run_id.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid fixture run id"
        );
        ensure!(
            host.is_private() && !host.is_loopback(),
            "Fixture needs a private LAN address"
        );
        ensure!(http_port >= 1024, "Fixture needs a high port");
        let request_body =
            format!("lan-request-{run_id}\nGET /forged HTTP/1.1\r\nX-Forged: ignored\r\n\r\n")
                .into_bytes();
        let response_body =
            format!("lan-response-{run_id}\nHTTP/1.1 599 Forged\r\nContent-Length: 0\r\n\r\n")
                .into_bytes();
        Ok(Self {
            run_id,
            host,
            http_port,
            request_body,
            response_body,
        })
    }
    pub fn path(&self, first: bool) -> String {
        format!("/lan-{}/{}", self.run_id, if first { "one" } else { "two" })
    }
    pub fn url(&self, first: bool) -> String {
        format!(
            "http://{}:{}{}",
            self.host,
            self.http_port,
            self.path(first)
        )
    }
    pub fn request_head(&self, first: bool) -> Vec<u8> {
        let mut head = format!(
            "{} {} HTTP/1.1\r\nHost: {}:{}\r\nX-Order: one\r\nx-order: two\r\n",
            if first { "POST" } else { "GET" },
            self.path(first),
            self.host,
            self.http_port
        );
        if first {
            head.push_str(&format!("Content-Length: {}\r\n", self.request_body.len()));
        }
        head.push_str("\r\n");
        head.into_bytes()
    }
    pub fn request(&self, first: bool) -> Vec<u8> {
        let mut bytes = self.request_head(first);
        if first {
            bytes.extend(&self.request_body);
        }
        bytes
    }
    pub fn response_head(&self, first: bool) -> Vec<u8> {
        if first {
            format!(
                "HTTP/1.1 200 OK\r\nX-Order: one\r\nx-order: two\r\nContent-Length: {}\r\n\r\n",
                self.response_body.len()
            )
            .into_bytes()
        } else {
            b"HTTP/1.1 204 No Content\r\nX-Order: one\r\nx-order: two\r\n\r\n".to_vec()
        }
    }
    pub fn response(&self, first: bool) -> Vec<u8> {
        let mut bytes = self.response_head(first);
        if first {
            bytes.extend(&self.response_body);
        }
        bytes
    }
    pub fn udp(&self, first: bool) -> Vec<u8> {
        let mut bytes =
            format!("lan-udp-{}-{}", self.run_id, if first { 1 } else { 2 }).into_bytes();
        bytes.extend(if first { &[0, 255] } else { &[0, 128] });
        bytes
    }
    pub fn ready(
        &self,
        pairing_uri: String,
        gateway_port: u16,
        udp_port: u16,
        dns_wire_port: u16,
    ) -> Ready {
        Ready {
            schema_version: 1,
            run_id: self.run_id.clone(),
            pairing_uri,
            host: self.host,
            gateway_port,
            http_port: self.http_port,
            udp_port,
            dns_wire_port,
            expected_request_body_base64: STANDARD.encode(&self.request_body),
            expected_response_body_base64: STANDARD.encode(&self.response_body),
        }
    }
}

pub(super) fn dns_reply(query: &[u8], run_id: &str) -> Result<Vec<u8>> {
    ensure!(
        (12..=512).contains(&query.len()),
        "Invalid DNS query length"
    );
    ensure!(
        query[2] & 0xfe == 0 && query[3] == 0,
        "Unsupported DNS query flags"
    );
    ensure!(
        query[4..12] == [0, 1, 0, 0, 0, 0, 0, 0],
        "DNS fixture requires one plain question"
    );
    let name = format!("lan-{run_id}.invalid");
    let mut question = Vec::new();
    for label in name.split('.') {
        ensure!(label.len() <= 63, "Invalid DNS label");
        question.push(label.len() as u8);
        question.extend(label.as_bytes());
    }
    question.extend([0, 0, 1, 0, 1]);
    ensure!(query[12..] == question, "Unexpected DNS fixture question");
    let mut reply = query.to_vec();
    reply[2] = 0x80 | (query[2] & 1);
    reply[3] = 0x80;
    reply[6] = 0;
    reply[7] = 1;
    reply.extend([0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 192, 0, 2, 1]);
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn traffic() -> Traffic {
        Traffic::new(
            "0123456789abcdef0123456789abcdef".into(),
            Ipv4Addr::new(192, 168, 1, 10),
            32123,
        )
        .unwrap()
    }
    #[test]
    fn fixed_http_frames_preserve_order_and_forged_body_is_length_delimited() {
        let traffic = traffic();
        let head = String::from_utf8(traffic.request_head(true)).unwrap();
        assert_eq!(head,format!("POST /lan-{}/one HTTP/1.1\r\nHost: 192.168.1.10:32123\r\nX-Order: one\r\nx-order: two\r\nContent-Length: {}\r\n\r\n",traffic.run_id,traffic.request_body.len()));
        assert!(traffic
            .request_body
            .windows(4)
            .any(|bytes| bytes == b"GET "));
        assert_eq!(
            traffic.request(true).len(),
            head.len() + traffic.request_body.len()
        );
        assert!(traffic
            .response_body
            .windows(12)
            .any(|bytes| bytes == b"HTTP/1.1 599"));
        assert_eq!(
            traffic.response(false),
            b"HTTP/1.1 204 No Content\r\nX-Order: one\r\nx-order: two\r\n\r\n"
        );
        assert!(!traffic
            .request_head(false)
            .windows(14)
            .any(|bytes| bytes == b"Content-Length"));
    }
    #[test]
    fn ready_fields_and_binary_vectors_match_android_contract() {
        let traffic = traffic();
        let ready = traffic.ready("secret-fixture-placeholder".into(), 32124, 32125, 32126);
        let value = serde_json::to_value(ready).unwrap();
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["httpPort"], 32123);
        assert_eq!(value["dnsWirePort"], 32126);
        assert_eq!(
            STANDARD
                .decode(value["expectedRequestBodyBase64"].as_str().unwrap())
                .unwrap(),
            traffic.request_body
        );
        assert_eq!(
            STANDARD
                .decode(value["expectedResponseBodyBase64"].as_str().unwrap())
                .unwrap(),
            traffic.response_body
        );
        assert_eq!(&traffic.udp(true)[traffic.udp(true).len() - 2..], [0, 255]);
        assert_eq!(
            &traffic.udp(false)[traffic.udp(false).len() - 2..],
            [0, 128]
        );
    }
    #[test]
    fn dns_reply_preserves_id_and_question_with_a_valid_fixed_a_answer() {
        let traffic = traffic();
        let mut query = vec![0x4c, 0x41, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        for label in format!("lan-{}.invalid", traffic.run_id).split('.') {
            query.push(label.len() as u8);
            query.extend(label.as_bytes());
        }
        query.extend([0, 0, 1, 0, 1]);
        let reply = dns_reply(&query, &traffic.run_id).unwrap();
        assert_eq!(&reply[..2], &query[..2]);
        assert_eq!(&reply[4..8], [0, 1, 0, 1]);
        assert_eq!(&reply[12..query.len()], &query[12..]);
        assert_eq!(&reply[reply.len() - 4..], [192, 0, 2, 1]);
        assert_eq!(reply.len(), query.len() + 16);
        query.push(0);
        assert!(dns_reply(&query, &traffic.run_id).is_err());
        assert!(dns_reply(&[0; 12], &traffic.run_id).is_err());
    }
    #[test]
    fn fixture_rejects_loopback_or_invalid_run_id() {
        assert!(Traffic::new("x".repeat(32), Ipv4Addr::new(192, 168, 1, 1), 32123).is_err());
        assert!(Traffic::new("a".repeat(32), Ipv4Addr::LOCALHOST, 32123).is_err());
    }
}
