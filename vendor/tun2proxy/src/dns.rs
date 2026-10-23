use hickory_proto::{
    op::{Message, MessageType, ResponseCode},
    rr::{
        Name, RData, Record, RecordType,
        rdata::{A, AAAA},
    },
};
use std::{net::IpAddr, str::FromStr};

pub fn build_dns_response(
    mut request: Message,
    domain: &str,
    ip: IpAddr,
    ttl: u32,
) -> Result<Message, String> {
    let name = Name::from_str(domain).map_err(|e| e.to_string())?;
    let query_type = request
        .queries
        .first()
        .ok_or("DnsRequest no query body")?
        .query_type();
    let record = match (query_type, ip) {
        (RecordType::A, IpAddr::V4(ip)) => {
            Some(Record::from_rdata(name.clone(), ttl, RData::A(A(ip))))
        }
        (RecordType::AAAA, IpAddr::V6(ip)) => {
            Some(Record::from_rdata(name, ttl, RData::AAAA(AAAA(ip))))
        }
        // The Android tunnel currently uses an IPv4 virtual pool. A client
        // asking for AAAA or HTTPS must receive NODATA, not a mismatched A.
        _ => None,
    };

    // We must indicate that this message is a response. Otherwise, implementations may not
    // recognize it.
    request.metadata.message_type = MessageType::Response;

    if let Some(record) = record {
        request.add_answer(record);
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::{OpCode, Query};
    use std::net::Ipv4Addr;

    fn query(record_type: RecordType) -> Message {
        let name = Name::from_str("example.test.").unwrap();
        let mut message = Message::new(1, MessageType::Query, OpCode::Query);
        message.add_query(Query::query(name, record_type));
        message
    }

    #[test]
    fn ipv4_virtual_dns_only_answers_matching_a_queries() {
        let ip = IpAddr::V4(Ipv4Addr::new(198, 18, 0, 2));
        let a = build_dns_response(query(RecordType::A), "example.test.", ip, 5).unwrap();
        assert_eq!(a.metadata.message_type, MessageType::Response);
        assert!(matches!(a.answers[0].data, RData::A(_)));
        for kind in [RecordType::AAAA, RecordType::HTTPS] {
            let response = build_dns_response(query(kind), "example.test.", ip, 5).unwrap();
            assert_eq!(response.metadata.message_type, MessageType::Response);
            assert!(
                response.answers.is_empty(),
                "{kind:?} must not receive an A record"
            );
        }
    }
}

pub fn remove_ipv6_entries(message: &mut Message) {
    message
        .answers
        .retain(|answer| !matches!(&answer.data, RData::AAAA(_)));
}

pub fn extract_ipaddr_from_dns_message(message: &Message) -> Result<IpAddr, String> {
    if message.metadata.response_code != ResponseCode::NoError {
        return Err(format!("{:?}", message.metadata.response_code));
    }
    let mut cname = None;
    for answer in &message.answers {
        match &answer.data {
            RData::A(addr) => {
                return Ok(IpAddr::V4((*addr).into()));
            }
            RData::AAAA(addr) => {
                return Ok(IpAddr::V6((*addr).into()));
            }
            RData::CNAME(name) => {
                cname = Some(name.to_utf8());
            }
            _ => {}
        }
    }
    if let Some(cname) = cname {
        return Err(cname);
    }
    Err(format!("{:?}", message.answers))
}

pub fn extract_domain_from_dns_message(message: &Message) -> Result<String, String> {
    let query = message.queries.first().ok_or("DnsRequest no query body")?;
    let name = query.name().to_string();
    Ok(name)
}

pub fn parse_data_to_dns_message(data: &[u8], used_by_tcp: bool) -> Result<Message, String> {
    if used_by_tcp {
        if data.len() < 2 {
            return Err("invalid dns data".into());
        }
        let len = u16::from_be_bytes([data[0], data[1]]) as usize;
        let data = data.get(2..len + 2).ok_or("invalid dns data")?;
        return parse_data_to_dns_message(data, false);
    }
    let message = Message::from_vec(data).map_err(|e| e.to_string())?;
    Ok(message)
}
