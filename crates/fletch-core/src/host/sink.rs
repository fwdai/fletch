//! Where the engine's events go.
//!
//! Engine code emits through an [`EventSink`] instead of holding a
//! `tauri::AppHandle`, so the same emitters serve the desktop webview today and
//! a headless host with remote subscribers later. Event names and payloads are
//! the sink's input, not its concern: it only has to deliver them.

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::broadcast;

/// One event out of the engine, as [`BroadcastSink`] carries it: the name and
/// the already-serialized payload, both shared so a fan-out to several
/// subscribers copies neither.
pub type Event = (Arc<str>, Arc<Value>);

/// How many events a broadcast buffers per subscriber before the slow one is
/// told it lagged. One constant for both hops of the path — the engine's own
/// channel here, and the per-connection fan-out in `remote` — because a deeper
/// buffer on either end only postpones the same drop, and events are best
/// effort by contract (the frontend and the phone both refetch).
pub const EVENT_BUFFER: usize = 256;

/// One event out of the engine.
///
/// The error is a plain string because the only caller that reads it — the
/// publish-approval gate, which must fail closed when nobody can be asked —
/// needs "did this reach anyone" and nothing more. Everything else goes through
/// [`emit`], which logs and moves on: no event is delivery-guaranteed and the
/// frontend resyncs on focus rather than trusting delivery.
pub trait EventSink: Send + Sync + 'static {
    fn emit_value(&self, event: &str, payload: Value) -> Result<(), String>;
}

/// A shared sink, for the places that have to own one past the call that made
/// it (the git dispatcher's approval handle, the engine context in 1b).
pub type Sink = std::sync::Arc<dyn EventSink>;

/// Emit `payload` on `sink`, logging (not propagating) failure — the shared
/// tail of every typed emitter in the engine.
///
/// Serializing to [`Value`] here rather than in the sink keeps each payload's
/// own `Serialize` impl, notably `serialize_bytes_b64` on the PTY-carrying
/// events: the frontend decodes that base64, so a sink that re-serialized the
/// bytes itself would change the wire format.
pub fn emit<T: serde::Serialize + ?Sized>(sink: &dyn EventSink, event: &str, payload: &T) {
    match serde_json::to_value(payload) {
        Ok(value) => {
            if let Err(e) = sink.emit_value(event, value) {
                tracing::warn!(error = %e, event, "emit failed");
            }
        }
        Err(e) => tracing::warn!(error = %e, event, "emit failed"),
    }
}

/// Publishes every event on a broadcast channel, for the host's own
/// subscribers: today the remote event taps, tomorrow anything a headless host
/// wants to watch without a webview.
///
/// No receivers is success, not failure. A channel nobody has subscribed to is
/// the normal state of a desktop with no phone paired, and the one caller that
/// reads the error — the publish-approval gate, which denies when the question
/// could not be asked — is asking about the *user*, whose sink is the shell's
/// own `TauriSink`.
pub struct BroadcastSink(pub broadcast::Sender<Event>);

impl EventSink for BroadcastSink {
    fn emit_value(&self, event: &str, payload: Value) -> Result<(), String> {
        let _ = self.0.send((Arc::from(event), Arc::new(payload)));
        Ok(())
    }
}

/// Every event to every sink: the desktop webview *and* the host's own
/// subscribers.
///
/// Succeeds when at least one sink took the event, so the publish-approval gate
/// denies only when nothing could carry the question at all. Since
/// [`BroadcastSink`] always succeeds, that gate's fast refusal is effectively
/// retired on the desktop: a webview emit that fails now leaves the request
/// waiting for its timeout instead of being refused at once. Both outcomes are
/// a refusal, and a paired phone can answer a question the window could not
/// show.
///
/// The list can grow after the fanout is built, because boot has a cycle: the
/// push-alert tap is a sink that needs the engine ctx and the remote state,
/// both of which need the sink. See [`FanoutSink::add`].
pub struct FanoutSink(parking_lot::RwLock<Vec<Sink>>);

impl FanoutSink {
    pub fn new(sinks: Vec<Sink>) -> Self {
        Self(parking_lot::RwLock::new(sinks))
    }

    /// Add a sink once whatever it needed exists. Events emitted before this
    /// lands do not reach it — the same gap a `listen_any` tap installed later
    /// in `setup` had.
    pub fn add(&self, sink: Sink) {
        self.0.write().push(sink);
    }
}

impl EventSink for FanoutSink {
    fn emit_value(&self, event: &str, payload: Value) -> Result<(), String> {
        let mut delivered = false;
        let mut last_error = None;
        for sink in self.0.read().iter() {
            match sink.emit_value(event, payload.clone()) {
                Ok(()) => delivered = true,
                Err(e) => last_error = Some(e),
            }
        }
        match last_error {
            Some(e) if !delivered => Err(e),
            _ => Ok(()),
        }
    }
}

/// Drops every event. The sink for a host nobody is watching.
// Wired up with the headless host; the desktop always has a real sink.
#[allow(dead_code)]
pub struct NullSink;

impl EventSink for NullSink {
    fn emit_value(&self, _event: &str, _payload: Value) -> Result<(), String> {
        Ok(())
    }
}

/// Keeps every event, so a test can assert on the exact names and payloads an
/// emitter produces instead of on a mock's expectations.
#[cfg(test)]
pub(crate) struct RecordingSink {
    events: parking_lot::Mutex<Vec<(String, Value)>>,
}

#[cfg(test)]
impl RecordingSink {
    pub(crate) fn new() -> Self {
        Self {
            events: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// Everything emitted so far, in order.
    pub(crate) fn events(&self) -> Vec<(String, Value)> {
        self.events.lock().clone()
    }
}

#[cfg(test)]
impl EventSink for RecordingSink {
    fn emit_value(&self, event: &str, payload: Value) -> Result<(), String> {
        self.events.lock().push((event.to_string(), payload));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn emit_records_the_event_name_and_payload() {
        let sink = RecordingSink::new();
        emit(&sink, "agent:status", &json!({ "agent_id": "fuji" }));
        emit(&sink, "roadmap:item-deleted", "item-1");
        assert_eq!(
            sink.events(),
            vec![
                ("agent:status".to_string(), json!({ "agent_id": "fuji" })),
                ("roadmap:item-deleted".to_string(), json!("item-1")),
            ]
        );
    }

    struct Unserializable;

    impl serde::Serialize for Unserializable {
        fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(
                "this payload cannot be serialized",
            ))
        }
    }

    /// A payload that will not serialize is logged and dropped: an emitter is
    /// fire-and-forget, and every caller is on a path that must not unwind.
    #[test]
    fn a_failing_serializer_emits_nothing_and_does_not_panic() {
        let sink = RecordingSink::new();
        emit(&sink, "agent:status", &Unserializable);
        assert!(sink.events().is_empty());
    }

    struct FailingSink;

    impl EventSink for FailingSink {
        fn emit_value(&self, _event: &str, _payload: Value) -> Result<(), String> {
            Err("no listener".to_string())
        }
    }

    #[test]
    fn a_failing_sink_is_logged_not_propagated() {
        emit(&FailingSink, "agent:status", &json!({}));
    }

    /// The seam the remote taps hang off: a subscriber sees the event name and
    /// the payload the emitter built, with no Tauri event bus in between.
    #[tokio::test]
    async fn a_broadcast_subscriber_receives_the_event() {
        let (tx, mut rx) = broadcast::channel(EVENT_BUFFER);
        let sink = BroadcastSink(tx);

        // Nobody listening yet is not an error: this is a desktop with no
        // phone paired, which is the common case.
        assert!(BroadcastSink(broadcast::channel(EVENT_BUFFER).0)
            .emit_value("agent:status", json!({}))
            .is_ok());

        emit(&sink, "agent:status", &json!({ "agent_id": "fuji" }));
        let (name, payload) = rx.recv().await.unwrap();
        assert_eq!(name.as_ref(), "agent:status");
        assert_eq!(*payload, json!({ "agent_id": "fuji" }));
    }

    #[test]
    fn a_fanout_delivers_to_every_sink_and_survives_one_failing() {
        let recorder = Arc::new(RecordingSink::new());
        let fanout = FanoutSink::new(vec![Arc::new(FailingSink), recorder.clone()]);

        assert!(fanout.emit_value("agent:status", json!({})).is_ok());
        assert_eq!(recorder.events().len(), 1, "the working sink was skipped");

        // Every sink failing is the only case the caller hears about — that is
        // what the publish-approval gate reads as "nobody could be asked".
        let dead = FanoutSink::new(vec![Arc::new(FailingSink), Arc::new(FailingSink)]);
        assert_eq!(
            dead.emit_value("agent:status", json!({})),
            Err("no listener".to_string())
        );

        // A sink added after the fanout was built gets what comes next.
        let late = Arc::new(RecordingSink::new());
        fanout.add(late.clone());
        emit(&fanout, "roadmap:item-deleted", "item-1");
        assert_eq!(late.events().len(), 1);
        assert_eq!(recorder.events().len(), 2);
    }
}
