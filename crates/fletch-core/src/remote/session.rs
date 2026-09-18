//! The live-connection registry.
//!
//! A connected phone is addressable state, not just a row in `devices.json`:
//! revoking a device or turning remote access off has to reach the socket that
//! is already authenticated. Every accepted connection registers a
//! `SessionGuard` here for its whole life, and the guard's `Drop` is the only
//! place a session leaves the registry — so no exit path out of the connection
//! task can leak one, and `RemoteDevice::connected` is derived from this map
//! rather than from a second census that could disagree with it.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::mpsc;

/// Identifies one connection for the lifetime of the process.
pub(super) type SessionId = u64;

/// The close frame the host wants written before a socket goes away.
#[derive(Debug, Clone, Copy)]
pub(super) struct CloseReason {
    /// WebSocket close code, per `docs/remote-protocol.md` → "Errors".
    pub code: u16,
    pub reason: &'static str,
}

/// What the host asks a live connection to do. `None` means "drop the socket
/// without a close frame": the only case is outbound backpressure, where the
/// queue a close frame would have to travel through is precisely what is full.
pub(super) type CloseRequest = Option<CloseReason>;

/// A device's credential was revoked under a live connection.
pub(super) const CLOSE_REVOKED: CloseReason = CloseReason {
    code: 4003,
    reason: "device revoked",
};

/// The host turned remote access off under a live connection.
pub(super) const CLOSE_DISABLED: CloseReason = CloseReason {
    code: 4004,
    reason: "remote access disabled",
};

/// The listener moved to another port under a live connection. The standard
/// "service restart" code: unlike `4004`, the phone keeps retrying on its normal
/// backoff, so a relayed device comes straight back and a LAN device finds the
/// Mac again once it learns the new port.
pub(super) const CLOSE_RESTARTING: CloseReason = CloseReason {
    code: 1012,
    reason: "listener restarting",
};

struct Session {
    /// `None` until `pair`/`hello` succeeds. Pre-auth connections are tracked
    /// too, so disabling remote access closes a socket that is mid-handshake
    /// instead of leaving it free to finish pairing against a stopped listener.
    device_id: Option<String>,
    /// Capacity 1: one close request is all a connection can act on.
    close: mpsc::Sender<CloseRequest>,
}

/// Every live connection, keyed by an id that is never reused.
pub(super) struct Sessions {
    next_id: AtomicU64,
    live: Mutex<HashMap<SessionId, Session>>,
}

impl Sessions {
    pub(super) fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            live: Mutex::new(HashMap::new()),
        }
    }

    /// Register a connection. The returned guard must be held for as long as
    /// the connection lives; dropping it deregisters the session.
    pub(super) fn register(self: &Arc<Self>, close: mpsc::Sender<CloseRequest>) -> SessionGuard {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.live.lock().insert(
            id,
            Session {
                device_id: None,
                close,
            },
        );
        SessionGuard {
            sessions: self.clone(),
            id,
        }
    }

    /// Ask every session authenticated as `device_id` to close. Non-blocking:
    /// a session that already has a close queued needs no second one.
    pub(super) fn close_device(&self, device_id: &str, reason: CloseReason) {
        for session in self.live.lock().values() {
            if session.device_id.as_deref() == Some(device_id) {
                let _ = session.close.try_send(Some(reason));
            }
        }
    }

    /// Ask every session to close, authenticated or not.
    pub(super) fn close_all(&self, reason: CloseReason) {
        for session in self.live.lock().values() {
            let _ = session.close.try_send(Some(reason));
        }
    }

    /// The devices with at least one live authenticated connection.
    pub(super) fn connected_devices(&self) -> HashSet<String> {
        self.live
            .lock()
            .values()
            .filter_map(|s| s.device_id.clone())
            .collect()
    }
}

/// A connection's handle on its own registration. Owned by the connection task.
pub(super) struct SessionGuard {
    sessions: Arc<Sessions>,
    id: SessionId,
}

impl SessionGuard {
    /// Record which device this connection authenticated as, which is what
    /// makes it reachable by `close_device` and visible as `connected`.
    pub(super) fn bind_device(&self, device_id: &str) {
        if let Some(session) = self.sessions.live.lock().get_mut(&self.id) {
            session.device_id = Some(device_id.to_string());
        }
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.sessions.live.lock().remove(&self.id);
    }
}
