//! `sentinel-dns <udp address> <ip>`: the DNS server containers resolve
//! outside names with (daemon.json `dns`). A name with a dot is `<ip>` for an
//! `A` query and has no records of any other type; a single-label name does
//! not exist. Docker's own resolver answers compose service names before it
//! asks here, so traffic between services stays direct, and what reaches
//! `<ip>` on 80 and 443 init hands to `transparent-egress`.

use std::net::{Ipv4Addr, UdpSocket};

use crate::wire::Reader;
use crate::{ready, Result};

const HEADER_LEN: usize = 12;
const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;
const TTL_SECONDS: u32 = 10;
const NOERROR: u8 = 0;
const NXDOMAIN: u8 = 3;
/// An answer's name: a pointer to the question's, right after the header.
const NAME_OF_QUESTION: [u8; 2] = [0xc0, HEADER_LEN as u8];

pub fn run(args: &[String]) -> Result<()> {
    let [address, ip] = args else {
        return Err("sentinel-dns: expected <udp address> <ip>".into());
    };
    let ip: Ipv4Addr = ip
        .parse()
        .map_err(|_| format!("sentinel-dns: {ip:?} is not an IPv4 address"))?;
    let socket = UdpSocket::bind(address.as_str())
        .map_err(|e| format!("sentinel-dns: cannot listen on {address}: {e}"))?;
    ready::announce(&format!("udp {address} (names -> {ip})"));

    let mut query = [0u8; 512];
    loop {
        let (len, from) = match socket.recv_from(&mut query) {
            Ok(received) => received,
            Err(err) => {
                eprintln!("fletch-guest: sentinel-dns: {err}");
                continue;
            }
        };
        if let Some(reply) = answer(&query[..len], ip) {
            let _ = socket.send_to(&reply, from);
        }
    }
}

/// The reply to `query`, or `None` when it is not a standard query of one
/// question. The reply carries the header and the question, and the answer
/// when there is one; nothing of the query's other sections.
pub fn answer(query: &[u8], ip: Ipv4Addr) -> Option<Vec<u8>> {
    let question = question(query)?;
    let exists = question.labels > 1;
    let has_address = exists && question.qtype == TYPE_A;

    let mut reply = query[..question.end].to_vec();
    reply[2] = 0x80 | (query[2] & 0x01); // a response; recursion desired as asked
    reply[3] = 0x80 | if exists { NOERROR } else { NXDOMAIN }; // recursion available
    reply[4..HEADER_LEN].copy_from_slice(&[0, 1, 0, has_address.into(), 0, 0, 0, 0]);
    if has_address {
        reply.extend_from_slice(&NAME_OF_QUESTION);
        reply.extend_from_slice(&TYPE_A.to_be_bytes());
        reply.extend_from_slice(&CLASS_IN.to_be_bytes());
        reply.extend_from_slice(&TTL_SECONDS.to_be_bytes());
        reply.extend_from_slice(&4u16.to_be_bytes());
        reply.extend_from_slice(&ip.octets());
    }
    Some(reply)
}

struct Question {
    /// Where the question ends: the header and it are the reply's start.
    end: usize,
    labels: usize,
    qtype: u16,
}

fn question(query: &[u8]) -> Option<Question> {
    let mut r = Reader::new(query);
    let header = r.take(HEADER_LEN)?;
    let is_standard_query = header[2] & 0xf8 == 0; // QR clear, opcode QUERY
    if !is_standard_query || header[4..6] != [0, 1] {
        return None;
    }
    let mut labels = 0;
    loop {
        match r.block8()? {
            [] => break,
            label if label.len() > 63 => return None, // compression has no place here
            _ => labels += 1,
        }
    }
    let qtype = r.u16()?;
    r.u16()?; // class
    Some(Question {
        end: r.position(),
        labels,
        qtype,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTINEL: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 1);
    const TYPE_AAAA: u16 = 28;

    /// A query as resolvers send it: id 0x1234, recursion desired, one
    /// question, and an EDNS OPT record in the additional section.
    fn query(name: &str, qtype: u16) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 1];
        for label in name.split('.').filter(|l| !l.is_empty()) {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&qtype.to_be_bytes());
        q.extend_from_slice(&CLASS_IN.to_be_bytes());
        q.extend_from_slice(&[0, 0, 41, 0x04, 0xd0, 0, 0, 0, 0, 0, 0]);
        q
    }

    fn question_part(query: &[u8]) -> &[u8] {
        &query[HEADER_LEN..query.len() - 11]
    }

    #[test]
    fn a_dotted_name_is_the_sentinel() {
        let q = query("registry.npmjs.org", TYPE_A);
        let reply = answer(&q, SENTINEL).unwrap();
        assert_eq!(
            reply[..HEADER_LEN],
            [0x12, 0x34, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0]
        );
        let (echoed, record) = reply[HEADER_LEN..].split_at(question_part(&q).len());
        assert_eq!(echoed, question_part(&q));
        assert_eq!(
            record,
            [0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 10, 0, 4, 198, 18, 0, 1]
        );
    }

    #[test]
    fn other_types_of_a_dotted_name_have_no_records() {
        let q = query("deb.debian.org", TYPE_AAAA);
        let reply = answer(&q, SENTINEL).unwrap();
        assert_eq!(
            reply[..HEADER_LEN],
            [0x12, 0x34, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(&reply[HEADER_LEN..], question_part(&q));
    }

    #[test]
    fn a_single_label_name_does_not_exist() {
        for qtype in [TYPE_A, TYPE_AAAA] {
            let reply = answer(&query("api", qtype), SENTINEL).unwrap();
            assert_eq!(reply[3] & 0x0f, NXDOMAIN);
            assert_eq!(reply[6..8], [0, 0]);
        }
        let root = answer(&query("", TYPE_A), SENTINEL).unwrap();
        assert_eq!(root[3] & 0x0f, NXDOMAIN);
    }

    #[test]
    fn what_is_not_a_standard_query_of_one_question_gets_no_reply() {
        let q = query("example.com", TYPE_A);
        for cut in 0..q.len() - 11 {
            assert!(answer(&q[..cut], SENTINEL).is_none(), "cut at {cut}");
        }
        let mut response = q.clone();
        response[2] |= 0x80;
        assert!(answer(&response, SENTINEL).is_none());
        let mut two = q.clone();
        two[5] = 2;
        assert!(answer(&two, SENTINEL).is_none());
        let mut compressed = q.clone();
        compressed[HEADER_LEN] = 0xc0;
        assert!(answer(&compressed, SENTINEL).is_none());
    }
}
