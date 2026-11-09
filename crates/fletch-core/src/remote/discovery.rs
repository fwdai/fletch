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
//!
//! The other direction lives here too: [`browse_nearby`] lists the hosts
//! announcing themselves, for a desktop pairing with another machine
//! (Settings › Devices › Hosts). Only the desktop calls it; a headless host
//! announces and never browses.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;

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

/// How long dropping an announcement waits for the goodbye to go out. Bounded:
/// a network that has gone away must not hold up a quit.
const GOODBYE_WAIT: std::time::Duration = std::time::Duration::from_millis(300);

/// A live announcement. Dropping it withdraws the records (a goodbye packet,
/// so browsers drop this host at once rather than when the TTL lapses) and
/// stops the daemon. The drop waits up to [`GOODBYE_WAIT`] for the goodbye to
/// be sent, so a process about to exit does not take the daemon thread down
/// first.
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
        if let Ok(sent) = self.daemon.unregister(&self.fullname) {
            let _ = sent.recv_timeout(GOODBYE_WAIT);
        }
        // Fire-and-forget: there is nobody left to tell how it went.
        let _ = self.daemon.shutdown();
    }
}

/// One host announcing itself on the LAN, as a nearby list shows it. The same
/// shape the phone's browse plugin answers with (`NearbyHost` in
/// `src/remote/nearby.ts`), plus the addresses the announcement resolved to.
/// A claim, not a credential: the handshake is what proves `host_key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyHost {
    /// TXT `name`, or the instance label when the record carries none.
    pub name: String,
    /// TXT `id`: the host ID it claims, base64url.
    pub host_key: String,
    /// TXT `port`, or the SRV port when the TXT one is missing or garbled.
    pub port: u16,
    /// The IPv4 addresses the instance resolved to, sorted. IPv6 is left out:
    /// a link-local address needs its interface to dial, and the `.local`
    /// name a client derives from the key covers the rest.
    pub addresses: Vec<String>,
}

/// Read one resolved announcement. `None` for an instance with no `id` (not a
/// Fletch host, or a garbled record) and for `own_id`: a host hears its own
/// announcement, and pairing with itself is never what was meant.
fn nearby_host(service: &ResolvedService, own_id: &str) -> Option<NearbyHost> {
    let txt = |key| {
        service
            .get_property_val_str(key)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let host_key = txt(TXT_ID)?;
    if host_key == own_id {
        return None;
    }
    let port = txt(TXT_PORT)
        .and_then(|p| p.parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or_else(|| service.get_port());
    let name = txt(TXT_NAME).map_or_else(
        || {
            let full = service.get_fullname();
            full.strip_suffix(SERVICE_TYPE)
                .unwrap_or(full)
                .trim_end_matches('.')
                .to_string()
        },
        str::to_string,
    );
    let mut addresses: Vec<String> = service
        .get_addresses_v4()
        .into_iter()
        .map(|a| a.to_string())
        .collect();
    addresses.sort();
    Some(NearbyHost {
        name,
        host_key: host_key.to_string(),
        port,
        addresses,
    })
}

/// The list a browse answers with: one entry per host key (two instances
/// claiming one key are a spoof or a stale record, and a client keys its rows
/// on it), in name order.
fn listed(found: impl IntoIterator<Item = NearbyHost>) -> Vec<NearbyHost> {
    let mut seen = HashSet::new();
    let mut hosts: Vec<NearbyHost> = found
        .into_iter()
        .filter(|h| seen.insert(h.host_key.clone()))
        .collect();
    hosts.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.host_key.cmp(&b.host_key))
    });
    hosts
}

/// Browse `_fletch._tcp` for `duration` and answer with every host still
/// announced at the end, minus this host (`own_id`). Blocking for the whole
/// `duration`, so callers run it off the async runtime. Each call is its own
/// daemon, started and stopped here, so nothing browses between calls.
pub fn browse_nearby(duration: Duration, own_id: &str) -> Result<Vec<NearbyHost>, String> {
    let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
    let events = daemon.browse(SERVICE_TYPE).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + duration;
    // Keyed by instance, so a goodbye removes exactly what it names.
    let mut found: BTreeMap<String, NearbyHost> = BTreeMap::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match events.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(service)) => {
                if let Some(host) = nearby_host(&service, own_id) {
                    found.insert(service.get_fullname().to_string(), host);
                }
            }
            Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                found.remove(&fullname);
            }
            Ok(_) => {}
            // The deadline, or the daemon gone: either way, what was seen is
            // the answer.
            Err(_) => break,
        }
    }
    // Fire-and-forget, as the advertiser's drop: nothing waits on how it went.
    let _ = daemon.shutdown();
    Ok(listed(found.into_values()))
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

    /// An announcement as a browser resolves it: `addrs` is what the daemon
    /// would have collected from the A/AAAA records.
    fn resolved(
        instance: &str,
        addrs: &str,
        port: u16,
        txt: &[(&str, &str)],
    ) -> mdns_sd::ResolvedService {
        let txt: HashMap<String, String> = txt
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        ServiceInfo::new(
            SERVICE_TYPE,
            instance,
            &format!("{instance}.local."),
            addrs,
            port,
            txt,
        )
        .unwrap()
        .as_resolved_service()
    }

    #[test]
    fn a_resolved_announcement_reads_as_a_nearby_host() {
        let service = resolved(
            "fletch-0123456789abcdef",
            "192.168.1.24,10.0.0.7,fe80::1",
            47285,
            &[
                (TXT_NAME, "Studio Mac"),
                (TXT_ID, "HOSTKEY"),
                (TXT_PORT, "47285"),
            ],
        );
        assert_eq!(
            nearby_host(&service, "MINE"),
            Some(NearbyHost {
                name: "Studio Mac".into(),
                host_key: "HOSTKEY".into(),
                port: 47285,
                // IPv4 only, sorted.
                addresses: vec!["10.0.0.7".into(), "192.168.1.24".into()],
            })
        );
    }

    #[test]
    fn this_host_and_records_without_an_id_are_not_listed() {
        let own = resolved(
            "fletch-a",
            "10.0.0.1",
            1,
            &[(TXT_NAME, "Me"), (TXT_ID, "MINE")],
        );
        assert_eq!(nearby_host(&own, "MINE"), None);
        let anonymous = resolved("fletch-b", "10.0.0.2", 1, &[(TXT_NAME, "Who")]);
        assert_eq!(nearby_host(&anonymous, "MINE"), None);
        let blank = resolved("fletch-c", "10.0.0.3", 1, &[(TXT_ID, "  ")]);
        assert_eq!(nearby_host(&blank, "MINE"), None);
    }

    #[test]
    fn a_missing_txt_port_or_name_falls_back_to_the_srv_record() {
        let service = resolved(
            "fletch-0123456789abcdef",
            "10.0.0.9",
            47999,
            &[(TXT_ID, "KEY"), (TXT_PORT, "not a port")],
        );
        let host = nearby_host(&service, "MINE").unwrap();
        assert_eq!(host.port, 47999);
        assert_eq!(host.name, "fletch-0123456789abcdef");
    }

    #[test]
    fn the_list_has_one_row_per_key_in_name_order() {
        let host = |name: &str, key: &str| NearbyHost {
            name: name.into(),
            host_key: key.into(),
            port: 1,
            addresses: vec![],
        };
        let list = listed([
            host("studio", "B"),
            host("Attic", "A"),
            host("Impostor", "B"),
        ]);
        let names: Vec<&str> = list.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["Attic", "studio"]);
    }

    #[test]
    fn a_nearby_host_serializes_as_the_webview_reads_it() {
        let host = NearbyHost {
            name: "Studio".into(),
            host_key: "KEY".into(),
            port: 47285,
            addresses: vec!["10.0.0.7".into()],
        };
        assert_eq!(
            serde_json::to_value(&host).unwrap(),
            serde_json::json!({
                "name": "Studio",
                "hostKey": "KEY",
                "port": 47285,
                "addresses": ["10.0.0.7"],
            })
        );
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

        // And the goodbye: dropping the announcement takes it out of a
        // browser's list at once, not when the record's TTL lapses.
        let browser = ServiceDaemon::new().unwrap();
        let events = browser.browse(SERVICE_TYPE).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let fullname = loop {
            match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(mdns_sd::ServiceEvent::ServiceResolved(service))
                    if service.get_property_val_str(TXT_ID) == Some("test-id") =>
                {
                    break service.get_fullname().to_string()
                }
                Ok(_) => continue,
                Err(_) => panic!("the service was not seen again within 10 s"),
            }
        };
        drop(_advertiser);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(mdns_sd::ServiceEvent::ServiceRemoved(_, name)) if name == fullname => break,
                Ok(_) => continue,
                Err(_) => panic!("no goodbye within 5 s"),
            }
        }
        let _ = browser.shutdown();
    }
}
