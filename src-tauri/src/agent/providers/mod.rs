//! Per-provider transcript readers and CLI arg builders. Each submodule holds
//! one provider's `locate`/`read`/`build_args`/`pty_args`/`session_id` fns,
//! wired into the `PER_TURN_AGENTS` table in `super::capabilities`.

pub(crate) mod antigravity;
pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod cursor;
pub(crate) mod opencode;
pub(crate) mod pi;

use serde_json::Value;

/// Read a session id from one event: `None` unless the event's `type` matches
/// `event_type` (when set) and the `require` key/value gate passes (when set),
/// then read `id_field` as a string. The per-provider `*_session_id` extractors
/// are one-line wrappers over this — same shape, different gates.
pub(crate) fn gated_session_id(
    event: &Value,
    event_type: Option<&str>,
    require: Option<(&str, &str)>,
    id_field: &str,
) -> Option<String> {
    if let Some(ty) = event_type {
        if event.get("type").and_then(|t| t.as_str()) != Some(ty) {
            return None;
        }
    }
    if let Some((key, val)) = require {
        if event.get(key).and_then(|v| v.as_str()) != Some(val) {
            return None;
        }
    }
    event
        .get(id_field)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}
