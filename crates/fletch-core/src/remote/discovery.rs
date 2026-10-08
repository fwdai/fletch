//! Announcing this host on the local network (docs/remote-protocol.md,
//! "Discovery"), so a phone can list it by name and reach it at a name that
//! survives the Mac's IP address changing.
//!
//! Two records, both through one `mdns-sd` daemon:
//!
//! - the service, `_fletch._tcp`, whose TXT says who this is (`name`, `id`)
//!   and where it listens (`port`) — what a phone's "Macs nearby" list reads;
//! - the host name `fletch-<label>.local`, answering with every interface
//!   address and following them as they change. The label is derived from the
//!   host key, which a paired device already holds, so it can dial this name
//!   without browsing at all.
//!
//! Nothing here is a credential. Anyone on the LAN can announce any name and
//! any id; the Noise handshake is what authenticates, exactly as on a typed
//! address.

use std::collections::HashMap;

use mdns_sd::{ServiceDaemon, ServiceInfo};

/// The service type, as mDNS spells it.
pub const SERVICE_TYPE: &str = "_fletch._tcp.local.";

/// The TXT keys a browser reads. Short by mDNS custom: every byte of a TXT
/// record is in every answer.
pub const TXT_NAME: &str = "name";
pub const TXT_ID: &str = "id";
pub const TXT_PORT: &str = "port";

/// How many bytes of the host key the label carries: 16 hex digits, 64 bits —
/// unique on any LAN, and short enough to read in a log.
const LABEL_BYTES: usize = 8;

/// `fletch-<first 8 bytes of the host key, lowercase hex>`. Both the service
/// instance and the host name use it; a client derives the same string from the
/// key it pinned (`src/remote/pairing.ts`, `localHostname`).
pub fn label(host_key: &[u8; 32]) -> String {
    let hex: String = host_key[..LABEL_BYTES]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("fletch-{hex}")
}

/// One TXT string is a length byte and at most 255 bytes of `key=value`;
/// `mdns-sd` refuses a record with a longer one outright.
const TXT_STRING_MAX: usize = 255;

/// `value` cut at a character boundary so `key=value` fits one TXT string. A
/// Mac's Computer Name always fits; a `fletch-host --name` need not.
fn fit(key: &str, value: &str) -> String {
    let room = TXT_STRING_MAX - key.len() - 1;
    if value.len() <= room {
        return value.to_string();
    }
    let mut end = room;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

/// The TXT properties for one announcement.
fn properties(name: &str, host_id: &str, port: u16) -> HashMap<String, String> {
    HashMap::from([
        (TXT_NAME.to_string(), fit(TXT_NAME, name)),
        (TXT_ID.to_string(), host_id.to_string()),
        (TXT_PORT.to_string(), port.to_string()),
    ])
}

/// A live announcement. Dropping it withdraws the records (a goodbye packet,
/// so browsers drop this host at once rather than when the TTL lapses) and
/// stops the daemon.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertiser {
    /// Announce `name` at `port` under the label of `host_key`. An error here is
    /// the caller's to log, never to fail on: discovery is a convenience, and a
    /// phone can still dial the address it saved.
    pub fn start(
        host_key: &[u8; 32],
        host_id: &str,
        name: &str,
        port: u16,
    ) -> Result<Self, String> {
        let label = label(host_key);
        let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
        // No address given: `enable_addr_auto` publishes every interface's and
        // keeps them current, which is the point of the name.
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &label,
            &format!("{label}.local."),
            "",
            port,
            properties(name, host_id, port),
        )
        .map_err(|e| e.to_string())?
        .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon.register(info).map_err(|e| e.to_string())?;
        Ok(Self { daemon, fullname })
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        // Both are fire-and-forget: each hands back a channel for the outcome,
        // and there is nobody left to tell.
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_is_the_first_eight_key_bytes_in_hex() {
        let mut key = [0u8; 32];
        key[..8].copy_from_slice(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]);
        key[8] = 0xff; // past the label: must not show up
        assert_eq!(label(&key), "fletch-0123456789abcdef");
    }

    #[test]
    fn the_txt_record_says_who_and_where() {
        let txt = properties("Alex's MacBook Pro", "AAAA", 47285);
        assert_eq!(txt.len(), 3);
        assert_eq!(txt[TXT_NAME], "Alex's MacBook Pro");
        assert_eq!(txt[TXT_ID], "AAAA");
        assert_eq!(txt[TXT_PORT], "47285");
    }

    #[test]
    fn a_name_too_long_for_one_txt_string_is_cut_at_a_character() {
        let long = "é".repeat(200); // 400 bytes
        let cut = fit(TXT_NAME, &long);
        assert!(TXT_NAME.len() + 1 + cut.len() <= TXT_STRING_MAX);
        assert!(long.starts_with(&cut));
        assert_eq!(fit(TXT_NAME, "Studio Mac"), "Studio Mac");
    }

    /// The whole announcement, as a phone meets it: a browser finds the service
    /// with its TXT record, and the system resolver — `getaddrinfo`, which on
    /// a Mac or iPhone asks mDNSResponder, as the dialer does — answers for the
    /// `.local` name. Ignored by default because it announces on whatever
    /// network the machine is on: `cargo test -- --ignored announces`.
    #[test]
    #[ignore = "announces on the real network"]
    fn announces_and_resolves_on_this_network() {
        use std::time::{Duration, Instant};

        let key = [0x5a; 32];
        let host = format!("{}.local", label(&key));
        let _advertiser =
            Advertiser::start(&key, "test-id", "Fletch discovery test", 47999).expect("announce");

        let browser = ServiceDaemon::new().unwrap();
        let events = browser.browse(SERVICE_TYPE).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let found = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match events.recv_timeout(left) {
                Ok(mdns_sd::ServiceEvent::ServiceResolved(service))
                    if service.get_property_val_str(TXT_ID) == Some("test-id") =>
                {
                    break service
                }
                Ok(_) => continue,
                Err(_) => panic!("the service was not seen within 10 s"),
            }
        };
        assert_eq!(
            found.get_property_val_str(TXT_NAME),
            Some("Fletch discovery test")
        );
        assert_eq!(found.get_property_val_str(TXT_PORT), Some("47999"));
        assert_eq!(found.get_port(), 47999);
        let _ = browser.shutdown();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let addrs: Vec<_> = rt
            .block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(10),
                    tokio::net::lookup_host((host.as_str(), 47999)),
                )
                .await
            })
            .expect("resolved within 10 s")
            .unwrap_or_else(|e| panic!("{host} did not resolve: {e}"))
            .collect();
        assert!(!addrs.is_empty(), "{host} resolved to nothing");
    }
}
