//! Taps on the Tauri event bus that mirror a whitelist of desktop events onto
//! every authenticated connection.
//!
//! `Listener::listen_any` takes an event *name* (any target), so this installs
//! one tap per whitelisted name rather than one tap for the whole bus — Tauri 2
//! has no listen-to-everything API. The effect is the whitelist the protocol
//! doc describes: nothing outside `FORWARDED_EVENTS` can reach a phone, and raw
//! PTY output (`agent:output`, `shell:output`) in particular never does.

use std::sync::Arc;

use tauri::{AppHandle, Listener};

use super::RemoteState;

/// The v1 event whitelist, verbatim from `docs/remote-protocol.md`.
pub const FORWARDED_EVENTS: &[&str] = &[
    "agent:event",
    "agent:status",
    "agent:task",
    "agent:branch",
    "agent:model",
    "agent:effort",
    "agent:repo_added",
    "agent:git-action",
    "session:records-appended",
    "turn:started",
    "workspace:changed",
    "pr:state_changed",
    "verify:report",
    "publish:approval-requested",
];

/// Install the taps. Safe to call once at launch regardless of whether the
/// listener is enabled: with no connection subscribed, forwarding short-circuits
/// before it touches the payload.
pub fn install_taps(app: &AppHandle, state: Arc<RemoteState>) {
    for name in FORWARDED_EVENTS.iter().copied() {
        let state = state.clone();
        app.listen_any(name, move |event| {
            state.forward_event(name, event.payload());
        });
    }
    // Push notifications read two of the same events for a different purpose:
    // not "mirror this to a phone" but "is this worth waking one". Separate
    // taps rather than a branch in the loop above, because the whitelist is a
    // security boundary and should stay a plain fan-out.
    super::push::install_taps(app, state);
}
