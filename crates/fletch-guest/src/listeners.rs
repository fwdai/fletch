//! `listeners`: the TCP ports something in the guest listens on where a
//! forward to the guest's `localhost` reaches it (`127.0.0.1`, `::1`, or every
//! address), as one sorted JSON array per line, e.g. `[3000,5432]`: printed at
//! once and again whenever the set changes, polled once a second, until the
//! process is killed. Read from `/proc/net/tcp` and `/proc/net/tcp6`, which
//! show this process's network namespace (the guest's: a container's own
//! listeners are not there, a published port's proxy is) to any user.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::thread;
use std::time::Duration;

use crate::Result;

/// Init's egress loopback (`EGRESS_PORT`): the host's own proxy, not a server.
const EGRESS_PORT: u16 = 8899;
/// A socket's state in the tables: `TCP_LISTEN`, include/net/tcp_states.h.
const LISTEN: &str = "0A";

pub fn run(args: &[String]) -> Result<()> {
    if !args.is_empty() {
        return Err("listeners takes no arguments".into());
    }
    let mut printed = None;
    loop {
        let ports = listening_ports()?;
        if printed.as_ref() != Some(&ports) {
            writeln!(io::stdout(), "{}", serde_json::to_string(&ports)?)?;
            printed = Some(ports);
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn listening_ports() -> Result<BTreeSet<u16>> {
    let mut ports = BTreeSet::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let text = fs::read_to_string(table).map_err(|e| format!("listeners: {table}: {e}"))?;
        ports.extend(reachable_listeners(&text));
    }
    ports.remove(&EGRESS_PORT);
    Ok(ports)
}

/// The ports of a `/proc/net/tcp{,6}` table's listening sockets on a loopback
/// or wildcard address. Each row is `sl local_address rem_address st ...`,
/// the address and port in hex.
fn reachable_listeners(table: &str) -> impl Iterator<Item = u16> + '_ {
    table.lines().skip(1).filter_map(|row| {
        let mut fields = row.split_whitespace();
        let local = fields.nth(1)?;
        if fields.nth(1)? != LISTEN {
            return None;
        }
        let (addr, port) = local.split_once(':')?;
        let reachable = match address(addr)? {
            IpAddr::V4(ip) => ip == Ipv4Addr::LOCALHOST || ip.is_unspecified(),
            IpAddr::V6(ip) => ip == Ipv6Addr::LOCALHOST || ip.is_unspecified(),
        };
        if !reachable {
            return None;
        }
        u16::from_str_radix(port, 16).ok()
    })
}

/// An address as the tables print it: each 32-bit word of it in hex, as the
/// word lies in memory, so its bytes in network order are the word's own.
fn address(hex: &str) -> Option<IpAddr> {
    let mut bytes = Vec::with_capacity(16);
    for at in (0..hex.len()).step_by(8) {
        let word = u32::from_str_radix(hex.get(at..at + 8)?, 16).ok()?;
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    match bytes.len() {
        4 => Some(IpAddr::from(<[u8; 4]>::try_from(bytes).ok()?)),
        16 => Some(IpAddr::from(<[u8; 16]>::try_from(bytes).ok()?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::reachable_listeners;

    /// Rows as a little-endian guest prints them (both architectures a box
    /// runs on are).
    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1 1 0000000000000000 100 0 0 10 0
   1: 00000000:1538 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2 1 0000000000000000 100 0 0 10 0
   2: 0100007F:22C3 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 3 1 0000000000000000 100 0 0 10 0
   3: 0100007F:0BB8 0100007F:D2F0 01 00000000:00000000 00:00000000 00000000  1000        0 4 1 0000000000000000 20 4 30 10 -1
   4: 010011AC:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 5 1 0000000000000000 100 0 0 10 0
   5: 0200007F:1F91 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 6 1 0000000000000000 100 0 0 10 0
";
    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:18EB 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 7 1 0000000000000000 100 0 0 10 0
   1: 00000000000000000000000001000000:0BB9 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 8 1 0000000000000000 100 0 0 10 0
   2: 0000000000000000FFFF00000100007F:0BBA 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 9 1 0000000000000000 100 0 0 10 0
";

    #[test]
    fn listening_sockets_on_loopback_or_every_address_count() {
        // 127.0.0.1:3000, 0.0.0.0:5432 and 127.0.0.1:8899 listen; the
        // connection to 3000 does not; 172.17.0.1:8080 is the bridge's and
        // 127.0.0.2:8081 no forward to `localhost` reaches.
        let ports: Vec<u16> = reachable_listeners(TCP).collect();
        assert_eq!(ports, [3000, 5432, 8899]);
        // [::]:6379 and [::1]:3001; the IPv4-mapped 127.0.0.1 is neither.
        let ports: Vec<u16> = reachable_listeners(TCP6).collect();
        assert_eq!(ports, [6379, 3001]);
    }

    #[test]
    fn rows_that_do_not_parse_are_skipped() {
        let table = "header\n0: zz:0BB8 00000000:0000 0A\n1: 0100007F 00000000:0000 0A\n2:\n";
        assert_eq!(reachable_listeners(table).count(), 0);
    }
}
