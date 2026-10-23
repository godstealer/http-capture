use crate::error::Result;
use hashlink::{LruCache, linked_hash_map::RawEntryMut};
use hickory_proto::rr::RecordType;
use std::{
    collections::HashMap,
    convert::TryInto,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::{Duration, Instant},
};
use tproxy_config::IpCidr;

const MAPPING_TIMEOUT: u64 = 60; // Mapping timeout in seconds

struct NameCacheEntry {
    name: String,
    expiry: Instant,
}

/// A virtual DNS server which allocates IP addresses to clients.
/// The IP addresses are in the range of private IP addresses.
/// The DNS server is implemented as a LRU cache.
pub struct VirtualDns {
    ipv4: AddressPool,
    ipv6: Option<AddressPool>,
}

struct AddressPool {
    trailing_dot: bool,
    lru_cache: LruCache<IpAddr, NameCacheEntry>,
    name_to_ip: HashMap<String, IpAddr>,
    network_addr: IpAddr,
    broadcast_addr: IpAddr,
    next_addr: IpAddr,
}

impl VirtualDns {
    pub fn new(ipv4_pool: IpCidr, ipv6_pool: Option<IpCidr>) -> Self {
        Self {
            ipv4: AddressPool::new(ipv4_pool),
            ipv6: ipv6_pool.map(AddressPool::new),
        }
    }

    /// Returns the DNS response to send back to the client.
    pub fn generate_query(&mut self, data: &[u8]) -> Result<(Vec<u8>, String, IpAddr)> {
        use crate::dns;
        let message = dns::parse_data_to_dns_message(data, false)?;
        let qname = dns::extract_domain_from_dns_message(&message)?;
        let query_type = message
            .queries
            .first()
            .ok_or("DnsRequest no query body")?
            .query_type();
        let pool = if query_type == RecordType::AAAA {
            self.ipv6.as_mut().unwrap_or(&mut self.ipv4)
        } else {
            &mut self.ipv4
        };
        let ip = pool.find_or_allocate_ip(qname.clone())?;
        let message = dns::build_dns_response(message, &qname, ip, 5)?;
        Ok((message.to_vec()?, qname, ip))
    }

    pub fn touch_ip(&mut self, addr: &IpAddr) {
        match addr {
            IpAddr::V4(_) => self.ipv4.touch_ip(addr),
            IpAddr::V6(_) => {
                if let Some(pool) = &mut self.ipv6 {
                    pool.touch_ip(addr);
                }
            }
        }
    }

    pub fn resolve_ip(&mut self, addr: &IpAddr) -> Option<&String> {
        match addr {
            IpAddr::V4(_) => self.ipv4.resolve_ip(addr),
            IpAddr::V6(_) => self.ipv6.as_mut()?.resolve_ip(addr),
        }
    }
}

impl AddressPool {
    fn new(ip_pool: IpCidr) -> Self {
        let first = ip_pool.first_address();
        let last = ip_pool.last_address();
        let first_usable = if first == last {
            first
        } else {
            Self::increment_ip(first).expect("valid IP address")
        };
        Self {
            trailing_dot: false,
            next_addr: first_usable,
            name_to_ip: HashMap::default(),
            network_addr: first_usable,
            broadcast_addr: last,
            lru_cache: LruCache::new_unbounded(),
        }
    }

    fn increment_ip(addr: IpAddr) -> Result<IpAddr> {
        let mut ip_bytes = match addr as IpAddr {
            IpAddr::V4(ip) => Vec::<u8>::from(ip.octets()),
            IpAddr::V6(ip) => Vec::<u8>::from(ip.octets()),
        };

        // Traverse bytes from right to left and stop when we can add one.
        for j in 0..ip_bytes.len() {
            let i = ip_bytes.len() - 1 - j;
            if ip_bytes[i] != 255 {
                // We can add 1 without carry and are done.
                ip_bytes[i] += 1;
                break;
            } else {
                // Zero this byte and carry over to the next one.
                ip_bytes[i] = 0;
            }
        }
        let addr = if addr.is_ipv4() {
            let bytes: [u8; 4] = ip_bytes.as_slice().try_into()?;
            IpAddr::V4(Ipv4Addr::from(bytes))
        } else {
            let bytes: [u8; 16] = ip_bytes.as_slice().try_into()?;
            IpAddr::V6(Ipv6Addr::from(bytes))
        };
        Ok(addr)
    }

    // This is to be called whenever we receive or send a packet on the socket
    // which connects the tun interface to the client, so existing IP address to name
    // mappings to not expire as long as the connection is active.
    fn touch_ip(&mut self, addr: &IpAddr) {
        _ = self.lru_cache.get_mut(addr).map(|entry| {
            entry.expiry = Instant::now() + Duration::from_secs(MAPPING_TIMEOUT);
        });
    }

    fn resolve_ip(&mut self, addr: &IpAddr) -> Option<&String> {
        self.lru_cache.get(addr).map(|entry| &entry.name)
    }

    fn find_or_allocate_ip(&mut self, name: String) -> Result<IpAddr> {
        // This function is a search and creation function.
        // Thus, it is sufficient to canonicalize the name here.
        let insert_name = if name.ends_with('.') && !self.trailing_dot {
            String::from(name.trim_end_matches('.'))
        } else {
            name
        };

        let now = Instant::now();

        // Iterate through all entries of the LRU cache and remove those that have expired.
        loop {
            let (ip, entry) = match self.lru_cache.iter().next() {
                None => break,
                Some((ip, entry)) => (ip, entry),
            };

            // The entry has expired.
            if now > entry.expiry {
                let name = entry.name.clone();
                self.lru_cache.remove(&ip.clone());
                self.name_to_ip.remove(&name);
                continue; // There might be another expired entry after this one.
            }

            break; // The entry has not expired and all following entries are newer.
        }

        // Return the IP if it is stored inside our LRU cache.
        if let Some(ip) = self.name_to_ip.get(&insert_name) {
            let ip = *ip;
            self.touch_ip(&ip);
            return Ok(ip);
        }

        // Otherwise, store name and IP pair inside the LRU cache.
        let started_at = self.next_addr;

        loop {
            if let RawEntryMut::Vacant(vacant) =
                self.lru_cache.raw_entry_mut().from_key(&self.next_addr)
            {
                let expiry = Instant::now() + Duration::from_secs(MAPPING_TIMEOUT);
                let name0 = insert_name.clone();
                vacant.insert(
                    self.next_addr,
                    NameCacheEntry {
                        name: insert_name,
                        expiry,
                    },
                );
                self.name_to_ip.insert(name0, self.next_addr);
                return Ok(self.next_addr);
            }
            self.next_addr = Self::increment_ip(self.next_addr)?;
            if self.next_addr == self.broadcast_addr {
                // Wrap around.
                self.next_addr = self.network_addr;
            }
            if self.next_addr == started_at {
                return Err("Virtual IP space for DNS exhausted".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::{
        op::{Message, MessageType, OpCode, Query},
        rr::{Name, RData, RecordType},
    };
    use std::str::FromStr;

    fn query(kind: RecordType) -> Vec<u8> {
        let mut message = Message::new(42, MessageType::Query, OpCode::Query);
        message.add_query(Query::query(Name::from_str("example.test.").unwrap(), kind));
        message.to_vec().unwrap()
    }

    #[test]
    fn ipv4_pool_returns_nodata_for_aaaa_then_a_for_same_name() {
        let pool = IpCidr::from_str("198.18.0.0/15").unwrap();
        let mut dns = VirtualDns::new(pool, None);
        let (aaaa_bytes, _, allocated) = dns.generate_query(&query(RecordType::AAAA)).unwrap();
        let aaaa = Message::from_vec(&aaaa_bytes).unwrap();
        assert_eq!(aaaa.id, 42);
        assert_eq!(aaaa.metadata.message_type, MessageType::Response);
        assert!(aaaa.answers.is_empty());

        let (a_bytes, _, reused) = dns.generate_query(&query(RecordType::A)).unwrap();
        let a = Message::from_vec(&a_bytes).unwrap();
        assert_eq!(allocated, reused);
        assert!(matches!(a.answers[0].data, RData::A(_)));
    }

    #[test]
    fn dual_pool_answers_both_families_and_resolves_each_virtual_address() {
        let ipv4 = IpCidr::from_str("198.18.0.0/15").unwrap();
        let ipv6 = IpCidr::from_str("fd00:198:19::/96").unwrap();
        let mut dns = VirtualDns::new(ipv4, Some(ipv6));
        let (a_bytes, _, a_ip) = dns.generate_query(&query(RecordType::A)).unwrap();
        let (aaaa_bytes, _, aaaa_ip) = dns.generate_query(&query(RecordType::AAAA)).unwrap();
        assert!(matches!(
            Message::from_vec(&a_bytes).unwrap().answers[0].data,
            RData::A(_)
        ));
        assert!(matches!(
            Message::from_vec(&aaaa_bytes).unwrap().answers[0].data,
            RData::AAAA(_)
        ));
        assert!(a_ip.is_ipv4());
        assert!(aaaa_ip.is_ipv6());
        assert_eq!(
            dns.resolve_ip(&a_ip).map(String::as_str),
            Some("example.test")
        );
        assert_eq!(
            dns.resolve_ip(&aaaa_ip).map(String::as_str),
            Some("example.test")
        );
    }
}
