//! Paired-device remote access: a phone acts as a control panel for the agents
//! running on this Mac.
//!
//! The wire contract is `docs/remote-protocol.md` — every op mirrors an existing
//! Tauri command by name, argument keys and result DTO, so the dispatcher calls
//! the same supervisor/service functions the commands call and the phone can
//! reuse the desktop's TypeScript DTOs unchanged.
//!
//! Layout: `auth` owns the two credentials, `server` the WebSocket listener,
//! `dispatch` the op allowlist, `events` the Tauri event taps. This module owns
//! the state those four share and the listener's lifecycle.

mod auth;
mod dispatch;
mod events;
mod server;
#[cfg(test)]
mod tests;

pub use auth::{DeviceStore, PairingTokens};
pub use dispatch::{Dispatch, DispatchResult, SupervisorDispatch};
pub use events::install_taps;

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::error::{Error, Result};

/// `settings` key mirroring whether the listener should run. Read once at
/// launch and rewritten by `remote_set_enabled`.
pub const ENABLED_SETTING: &str = "remote.enabled";
/// `settings` key holding a TCP port override.
pub const PORT_SETTING: &str = "remote.port";
/// Default listen port (protocol doc).
pub const DEFAULT_PORT: u16 = 47285;
/// The only path the listener serves.
pub const WS_PATH: &str = "/ws";

/// How many event frames the fan-out buffers per connection. A phone that
/// stalls past this is told it lagged and refetches, exactly as the desktop
/// frontend does on focus — events are best effort by contract.
const EVENT_BUFFER: usize = 256;

/// Same shape as the other `settings`-mirrored booleans in the crate
/// (`rpc::approval`, `codegraph`): anything but the literal `"true"` is off.
pub fn parse_enabled(raw: Option<&str>) -> bool {
    raw == Some("true")
}

/// A blank, absent or unparseable port setting falls back to the default rather
/// than refusing to listen.
pub fn parse_port(raw: Option<&str>) -> u16 {
    raw.and_then(|s| s.trim().parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or(DEFAULT_PORT)
}

/// Who the phone is talking to, sent in the `pair` and `hello` results.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub name: String,
    pub app_version: String,
    pub os: String,
}

/// A paired device as Settings renders it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDevice {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub connected: bool,
}

/// `remote_status`' reply.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub enabled: bool,
    pub listening: bool,
    pub port: u16,
    /// Every address a phone could reach this host on, best candidate first.
    pub addresses: Vec<String>,
    pub devices: Vec<RemoteDevice>,
}

/// `remote_begin_pairing`' reply: the code to read out and the deep link to
/// encode as a QR.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingInvite {
    pub token: String,
    pub url: String,
    pub expires_at: String,
}

struct Inner {
    /// The user's intent, independent of whether a bind currently succeeds.
    enabled: bool,
    /// The configured port; `listening` reports the bound one.
    port: u16,
    server: Option<ServerHandle>,
}

struct ServerHandle {
    port: u16,
    /// Fires once to stop the accept loop and close live connections with 4004.
    shutdown: broadcast::Sender<()>,
}

/// Everything the remote surface shares: credentials, the event fan-out, the
/// live-session census and the listener handle. Held in Tauri managed state.
pub struct RemoteState {
    dispatch: Arc<dyn Dispatch>,
    devices: Arc<DeviceStore>,
    pairing: PairingTokens,
    events: broadcast::Sender<Arc<str>>,
    /// device id → live connection count, so `RemoteDevice::connected` is a
    /// fact about sockets rather than about the last `hello`.
    connected: Mutex<HashMap<String, usize>>,
    inner: Mutex<Inner>,
}

impl RemoteState {
    /// Build the state, loading `devices.json` from `<dir>` (which is
    /// `<app_data_dir>/remote`). Does not start the listener.
    pub fn new(dir: &std::path::Path, dispatch: Arc<dyn Dispatch>) -> Result<Arc<Self>> {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Ok(Arc::new(Self {
            dispatch,
            devices: Arc::new(DeviceStore::load(dir)?),
            pairing: PairingTokens::new(),
            events,
            connected: Mutex::new(HashMap::new()),
            inner: Mutex::new(Inner {
                enabled: false,
                port: DEFAULT_PORT,
                server: None,
            }),
        }))
    }

    pub fn devices(&self) -> &Arc<DeviceStore> {
        &self.devices
    }

    pub fn pairing(&self) -> &PairingTokens {
        &self.pairing
    }

    /// Start listening on `port` (0 binds an ephemeral one, which the socket
    /// tests use). Returns the bound port. Idempotent: an already-running
    /// listener is left alone.
    ///
    /// Must be called from inside the async runtime — the bind itself is
    /// synchronous so that "port already in use" reaches the settings UI as an
    /// error rather than a log line.
    pub fn start(self: &Arc<Self>, port: u16) -> Result<u16> {
        let mut inner = self.inner.lock();
        if let Some(port) = inner.server.as_ref().map(|h| h.port) {
            inner.enabled = true;
            return Ok(port);
        }

        // Bind before recording the intent, so a port that cannot be opened
        // leaves the toggle off and the error on screen rather than a switch
        // that claims to be on.
        let std_listener = std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
            .map_err(|e| Error::Other(format!("remote: cannot listen on port {port}: {e}")))?;
        std_listener.set_nonblocking(true)?;
        let listener = tokio::net::TcpListener::from_std(std_listener)?;
        let bound = listener.local_addr()?.port();

        let (shutdown, stop) = broadcast::channel(1);
        tokio::spawn(server::accept_loop(self.clone(), listener, stop));
        inner.enabled = true;
        inner.port = port;
        inner.server = Some(ServerHandle {
            port: bound,
            shutdown,
        });
        tracing::info!(port = bound, "remote: listening");
        Ok(bound)
    }

    /// Stop listening and close every live connection with 4004.
    pub fn stop(&self) {
        let mut inner = self.inner.lock();
        inner.enabled = false;
        if let Some(handle) = inner.server.take() {
            let _ = handle.shutdown.send(());
            tracing::info!(port = handle.port, "remote: stopped");
        }
    }

    pub fn status(&self) -> RemoteStatus {
        let inner = self.inner.lock();
        let connected = self.connected.lock();
        RemoteStatus {
            enabled: inner.enabled,
            listening: inner.server.is_some(),
            port: inner.server.as_ref().map_or(inner.port, |h| h.port),
            addresses: candidate_addresses(),
            devices: self
                .devices
                .list()
                .into_iter()
                .map(|d| RemoteDevice {
                    connected: connected.get(&d.device_id).copied().unwrap_or(0) > 0,
                    device_id: d.device_id,
                    name: d.name,
                    platform: d.platform,
                    created_at: d.created_at,
                    last_seen_at: d.last_seen_at,
                })
                .collect(),
        }
    }

    /// Mint a pairing token and the `fletch://pair` deep link that carries it.
    pub fn begin_pairing(&self) -> PairingInvite {
        let minted = self.pairing.mint();
        let host = host_info();
        let port = {
            let inner = self.inner.lock();
            inner.server.as_ref().map_or(inner.port, |h| h.port)
        };
        let addr = candidate_addresses()
            .into_iter()
            .next()
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let url = format!(
            "fletch://pair?host={}&port={}&token={}&name={}",
            urlencode(&addr),
            port,
            urlencode(&minted.token),
            urlencode(&host.name),
        );
        PairingInvite {
            token: minted.token,
            url,
            expires_at: minted.expires_at.to_rfc3339(),
        }
    }

    /// Revoke a device. Its live connection (if any) is not torn down here —
    /// the next `hello` fails with 4003, and the connection dies on its own
    /// ping timeout. Keeping revoke a pure credential operation avoids reaching
    /// into per-connection state for a case the user retries anyway.
    pub fn revoke_device(&self, device_id: &str) -> Result<bool> {
        self.devices.revoke(device_id)
    }

    pub(super) fn subscribe(&self) -> broadcast::Receiver<Arc<str>> {
        self.events.subscribe()
    }

    pub(super) async fn dispatch(&self, op: &str, args: Value) -> DispatchResult {
        self.dispatch.dispatch(op, args).await
    }

    /// Fan one Tauri event out to every authenticated connection.
    ///
    /// `payload_json` is spliced in rather than parsed and re-serialized, so the
    /// phone receives byte-identical JSON to the desktop webview.
    pub(super) fn forward_event(&self, name: &str, payload_json: &str) {
        if self.events.receiver_count() == 0 {
            return;
        }
        let payload = if payload_json.trim().is_empty() {
            "null"
        } else {
            payload_json
        };
        let name = serde_json::to_string(name).unwrap_or_else(|_| "\"\"".to_string());
        let frame = format!("{{\"event\":{name},\"payload\":{payload}}}");
        let _ = self.events.send(Arc::from(frame));
    }

    pub(super) fn mark_connected(&self, device_id: &str) {
        *self
            .connected
            .lock()
            .entry(device_id.to_string())
            .or_insert(0) += 1;
    }

    pub(super) fn mark_disconnected(&self, device_id: &str) {
        let mut connected = self.connected.lock();
        if let Some(n) = connected.get_mut(device_id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                connected.remove(device_id);
            }
        }
    }
}

/// Every IPv4 address a phone could dial this host on, best candidate first:
/// RFC1918 LAN addresses, then everything else routable (which is where a
/// Tailscale 100.64/10 address lands). Loopback and link-local are dropped —
/// neither reaches a phone.
pub fn candidate_addresses() -> Vec<String> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut addrs: Vec<Ipv4Addr> = ifaces
        .into_iter()
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(v4) => Some(v4),
            std::net::IpAddr::V6(_) => None,
        })
        .filter(|v4| !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified())
        .collect();
    addrs.sort();
    addrs.dedup();
    addrs.sort_by_key(|v4| !v4.is_private());
    addrs.into_iter().map(|v4| v4.to_string()).collect()
}

/// Host identity for the pairing URL and the `hello` result.
pub fn host_info() -> HostInfo {
    HostInfo {
        name: machine_name(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
    }
}

/// The name a user would recognize in a device list. On macOS that is the
/// Sharing pane's Computer Name ("Alex's MacBook Pro"), not the DNS hostname
/// ("Alexs-MacBook-Pro.local"), so `scutil` is asked first. Cold path — called
/// only when pairing or reading status.
fn machine_name() -> String {
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("scutil", &["--get", "ComputerName"]), ("hostname", &[])]
    } else {
        &[("hostname", &[])]
    };
    for (bin, args) in candidates {
        if let Ok(out) = std::process::Command::new(bin).args(*args).output() {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if out.status.success() && !name.is_empty() {
                return name;
            }
        }
    }
    "Fletch host".to_string()
}

/// Percent-encode a query-string value. Deliberately tiny: the only values
/// encoded here are an IPv4 literal, an 8-character token and a machine name,
/// so an allowlist of unreserved characters is the whole rule.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
