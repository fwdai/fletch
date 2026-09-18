//! Where the engine's events go.
//!
//! Engine code emits through an [`EventSink`] instead of holding a
//! `tauri::AppHandle`, so the same emitters serve the desktop webview today and
//! a headless host with remote subscribers later. Event names and payloads are
//! the sink's input, not its concern: it only has to deliver them.

use serde_json::Value;

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

/// The desktop's sink: events reach the webview exactly as they did when the
/// engine called `app.emit` itself. Implemented on `AppHandle` directly (rather
/// than a newtype) so every existing `&app` argument coerces to
/// `&dyn EventSink` without touching the call site; it becomes a `TauriSink`
/// newtype once the trait lives in a crate that cannot depend on Tauri.
impl EventSink for tauri::AppHandle {
    fn emit_value(&self, event: &str, payload: Value) -> Result<(), String> {
        tauri::Emitter::emit(self, event, payload).map_err(|e| e.to_string())
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
}
