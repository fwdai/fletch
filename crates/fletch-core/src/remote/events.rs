//! Taps on the engine's event stream that mirror a whitelist of desktop events
//! onto every authenticated connection.
//!
//! The stream is the engine's own broadcast (`host::sink::BroadcastSink`), which
//! carries *every* event the engine emits — so this filters by name, and that
//! filter is the whitelist the protocol doc describes: nothing outside
//! `FORWARDED_EVENTS` can reach a phone, and raw PTY output (`agent:output`,
//! `shell:output`) in particular never does.
//!
//! One subscriber task for the whole stream, where the Tauri event bus needed
//! one `listen_any` per name: the name is in the event now, not in the
//! subscription.

use std::sync::Arc;

use tokio::sync::broadcast;

use super::RemoteState;
use crate::host::{EngineCtx, Event, Sink};

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
    "turn:sent",
    "turn:started",
    "workspace:changed",
    "pr:state_changed",
    "verify:report",
    "publish:approval-requested",
    "publish:approval-resolved",
];

/// Install the taps and hand back the push-alert one.
///
/// Safe to call once at launch regardless of whether the listener is enabled:
/// with no connection subscribed, forwarding short-circuits before it touches
/// the payload.
///
/// Two taps, two mechanisms, because they need different timing. Forwarding is
/// the subscriber task below — a frame the phone will read whenever it reads.
/// Push alerts must run *inside* the emit that raised them (see `push`), so
/// they come back as a [`Sink`] for the host to add to its fanout rather than
/// being driven from this task.
pub fn install_taps(
    ctx: &Arc<EngineCtx>,
    state: Arc<RemoteState>,
    mut events: broadcast::Receiver<Event>,
) -> Sink {
    let forward_to = state.clone();
    crate::host::spawn(async move {
        loop {
            match events.recv().await {
                Ok((name, payload)) => {
                    if FORWARDED_EVENTS.contains(&name.as_ref()) {
                        forward_to.forward_event(&name, &payload);
                    }
                }
                // Best-effort delivery by contract, same as the per-connection
                // fan-out this feeds (see `server::spawn_event_forwarder`): the
                // phone refetches on reconnect and on returning to the
                // foreground.
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!(dropped = n, "remote: event tap lagged")
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    super::push::tap(ctx, state)
}
