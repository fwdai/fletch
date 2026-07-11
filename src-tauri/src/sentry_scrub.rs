//! Scrubs Sentry payloads to a privacy-safe shape before they leave the machine.
//!
//! Crash/error reporting stays on regardless of the product-telemetry opt-out
//! (see the comment above `sentry::init` in `lib.rs`), so the burden falls on the
//! *payload*: a Sentry event must never carry a filesystem path, an argv (which
//! can embed the user's prompt text), a repo/branch name, or a raw error string
//! (an `io::Error` embeds the path it failed on). We enforce that with an
//! **allowlist, not a blocklist**.
//!
//! The invariant that makes an allowlist sufficient: every `tracing` call in this
//! codebase puts dynamic data in *structured fields* and keeps the log *message*
//! a static string literal. The `sentry-tracing` integration maps that message to
//! `event.message` / `breadcrumb.message` and the fields to a side channel:
//!
//! * events (WARN/ERROR) — fields land in `contexts["Rust Tracing Fields"]` as a
//!   `Context::Other(map)`, source location in `contexts["Rust Tracing Location"]`;
//! * breadcrumbs (below WARN) — fields land in `breadcrumb.data`.
//!
//! So we keep the static message and drop *every* dynamic field except a small set
//! of known-categorical keys. Nothing dynamic egresses unless its key is
//! consciously listed in [`ALLOWLIST`]. This is why fields you logged may be
//! absent in Sentry: to surface a new categorical field, add its key to
//! [`ALLOWLIST`] here — and only after confirming it can never carry a path,
//! prompt, or other free-form user text.
//!
//! Belt-and-braces: the message is a static literal by convention, but if one ever
//! interpolates a user-private path we redact the roots those paths live under —
//! the home directory and the OS temp roots (see [`redact_private_paths`]). The
//! static-message invariant is the real defense; this just limits the blast radius
//! of a mistake. We do not attempt general path detection — that is a blocklist,
//! and blocklists leak.

use std::collections::BTreeMap;

use sentry::protocol::{Context, Event, Value};
use sentry::Breadcrumb;

/// Field keys allowed to egress on events and breadcrumbs. Every entry must be
/// categorical (a fixed, bounded vocabulary) and provably free of paths, prompts,
/// repo/branch names, or other free-form user text:
///
/// * `agent_id` — opaque per-agent UUID;
/// * `session`  — opaque per-session id;
/// * `fresh`    — bool (fresh session vs resume);
/// * `op`       — a short static operation label.
///
/// Everything else is dropped, including `error`, `path`, `file`, `cwd`, `argv`,
/// `sandbox_root`, and `stderr`.
const ALLOWLIST: &[&str] = &["agent_id", "session", "fresh", "op"];

/// Drop every entry whose key is not in [`ALLOWLIST`].
fn retain_allowlisted(map: &mut BTreeMap<String, Value>) {
    map.retain(|key, _| ALLOWLIST.contains(&key.as_str()));
}

/// Replace the roots under which user-private paths live: the home directory
/// (`[home]`) and the OS temp roots (`[tmp]`). A best-effort guard for the
/// (by-convention impossible) case of such a path interpolated into an
/// otherwise-static message, and for panic messages, which are inherently
/// free-form.
///
/// This is deliberately a *bounded* set of well-known roots — home, `$TMPDIR`,
/// `/var/folders/`, `/private/tmp/`, `/tmp/` — not general path detection.
/// On macOS every user-writable location is under one of these, and a fixed
/// prefix list can't rot the way an open-ended "does this look like a path"
/// heuristic would. Subpaths are kept (`[home]/x/y`) so reports stay
/// debuggable. Longest-prefix-first so `$TMPDIR` (under `/var/folders/`) wins
/// over its parent.
fn redact_private_paths(text: &mut String) {
    let home = dirs::home_dir().and_then(|p| p.to_str().map(str::to_owned));
    let tmpdir = std::env::var("TMPDIR").ok();
    let mut roots: Vec<(&str, &str)> = Vec::new();
    if let Some(home) = home.as_deref().filter(|s| !s.is_empty()) {
        roots.push((home, "[home]"));
    }
    if let Some(tmpdir) = tmpdir
        .as_deref()
        .map(|s| s.trim_end_matches('/'))
        .filter(|s| !s.is_empty())
    {
        roots.push((tmpdir, "[tmp]"));
    }
    roots.extend([
        ("/private/var/folders/", "[tmp]/"),
        ("/var/folders/", "[tmp]/"),
        ("/private/tmp/", "[tmp]/"),
        ("/tmp/", "[tmp]/"),
    ]);
    roots.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
    for (prefix, placeholder) in roots {
        if text.contains(prefix) {
            *text = text.replace(prefix, placeholder);
        }
    }
}

/// Scrub a captured event (WARN/ERROR tracing events, panics, manual captures).
///
/// Keeps the static message/logentry, level, timestamp, exception + stacktrace
/// data, release, environment, and SDK info. Drops the machine hostname
/// (`server_name`, populated by the contexts integration) and every dynamic field
/// not in [`ALLOWLIST`], wherever the tracing integration stashed it (`extra` and
/// any `Context::Other` map — notably `contexts["Rust Tracing Fields"]`). Typed
/// contexts (`os`/`device`/`runtime`) are categorical and kept.
pub fn scrub_event(mut event: Event<'static>) -> Option<Event<'static>> {
    // The contexts integration copies the OS hostname into `server_name` before
    // `before_send` runs (see sentry-core `Client::prepare_event`), so clearing it
    // here is what actually keeps it off the wire.
    event.server_name = None;

    if let Some(message) = event.message.as_mut() {
        redact_private_paths(message);
    }
    if let Some(entry) = event.logentry.as_mut() {
        redact_private_paths(&mut entry.message);
    }

    // Panic messages land in exception values, and a panic payload (e.g. an
    // `expect` on an io error) can embed a user path. Redact the private path
    // roots but keep the message — it is load-bearing for debugging, and on
    // macOS every user-writable location is under home or a temp root.
    // Stacktrace frame paths are left alone: they come from the binary's debug
    // info (the *build* machine, identical for every user), and scrubbing them
    // would break symbolication.
    for exception in event.exception.values.iter_mut() {
        if let Some(value) = exception.value.as_mut() {
            redact_private_paths(value);
        }
    }

    // Empty for tracing events, but manual captures / other integrations may
    // populate `extra`; hold the allowlist invariant there too.
    retain_allowlisted(&mut event.extra);

    // Tracing fields (and source location) ride in `Context::Other` maps. Allowlist
    // each and drop the context entirely if nothing categorical survived, so we
    // don't emit an empty `"Rust Tracing Fields"` shell. Typed contexts are left
    // untouched.
    event.contexts.retain(|_, ctx| match ctx {
        Context::Other(map) => {
            retain_allowlisted(map);
            !map.is_empty()
        }
        _ => true,
    });

    Some(event)
}

/// Scrub a breadcrumb (sub-WARN tracing events, kept as context for later events).
///
/// Keeps the static message, level, category, and timestamp; drops every entry in
/// `data` not in [`ALLOWLIST`].
pub fn scrub_breadcrumb(mut breadcrumb: Breadcrumb) -> Option<Breadcrumb> {
    if let Some(message) = breadcrumb.message.as_mut() {
        redact_private_paths(message);
    }
    retain_allowlisted(&mut breadcrumb.data);
    Some(breadcrumb)
}

#[cfg(test)]
// `Event` has ~25 fields, so building fixtures field-by-field from `default()`
// is clearer than a giant struct literal.
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build the `Context::Other` map the tracing integration produces for an
    /// event's structured fields.
    fn tracing_fields() -> BTreeMap<String, Value> {
        let mut map = BTreeMap::new();
        map.insert("cwd".into(), json!("/Users/someone/code/repo"));
        map.insert("argv".into(), json!(["claude", "--prompt", "secret text"]));
        map.insert("sandbox_root".into(), json!("/private/tmp/box"));
        map.insert(
            "error".into(),
            json!("open /Users/someone/.ssh/id_rsa: denied"),
        );
        map.insert("agent_id".into(), json!("agent-123"));
        map.insert("op".into(), json!("spawn"));
        map
    }

    #[test]
    fn scrub_event_drops_sensitive_fields_keeps_allowlisted() {
        let mut event = Event::default();
        event.message = Some("spawning sandboxed pty agent".into());
        event.server_name = Some("my-laptop.local".into());
        event.contexts.insert(
            "Rust Tracing Fields".into(),
            Context::Other(tracing_fields()),
        );
        // A manual-capture style `extra` entry alongside an allowlisted one.
        event.extra.insert("path".into(), json!("/Users/someone/x"));
        event.extra.insert("session".into(), json!("sess-9"));

        let event = scrub_event(event).expect("event not dropped");

        assert_eq!(event.server_name, None, "hostname must not egress");
        assert_eq!(
            event.message.as_deref(),
            Some("spawning sandboxed pty agent"),
            "static message is preserved"
        );

        let Context::Other(fields) = &event.contexts["Rust Tracing Fields"] else {
            panic!("tracing-fields context should remain (has allowlisted keys)");
        };
        assert!(!fields.contains_key("cwd"));
        assert!(!fields.contains_key("argv"));
        assert!(!fields.contains_key("sandbox_root"));
        assert!(!fields.contains_key("error"));
        assert_eq!(fields.get("agent_id"), Some(&json!("agent-123")));
        assert_eq!(fields.get("op"), Some(&json!("spawn")));

        assert!(!event.extra.contains_key("path"));
        assert_eq!(event.extra.get("session"), Some(&json!("sess-9")));
    }

    #[test]
    fn scrub_event_drops_fully_scrubbed_context() {
        let mut event = Event::default();
        let mut loc = BTreeMap::new();
        loc.insert("file".into(), json!("src/agent.rs"));
        loc.insert("line".into(), json!(846));
        event
            .contexts
            .insert("Rust Tracing Location".into(), Context::Other(loc));

        let event = scrub_event(event).expect("event not dropped");
        assert!(
            !event.contexts.contains_key("Rust Tracing Location"),
            "a context with no allowlisted keys is removed, not emitted empty"
        );
    }

    #[test]
    fn scrub_breadcrumb_drops_sensitive_data_keeps_allowlisted() {
        let mut breadcrumb = Breadcrumb {
            message: Some("git status --porcelain=v1 failed".into()),
            ..Default::default()
        };
        breadcrumb.data = tracing_fields();

        let breadcrumb = scrub_breadcrumb(breadcrumb).expect("breadcrumb not dropped");

        assert_eq!(
            breadcrumb.message.as_deref(),
            Some("git status --porcelain=v1 failed")
        );
        assert!(!breadcrumb.data.contains_key("cwd"));
        assert!(!breadcrumb.data.contains_key("argv"));
        assert!(!breadcrumb.data.contains_key("sandbox_root"));
        assert!(!breadcrumb.data.contains_key("error"));
        assert_eq!(breadcrumb.data.get("agent_id"), Some(&json!("agent-123")));
        assert_eq!(breadcrumb.data.get("op"), Some(&json!("spawn")));
    }

    #[test]
    fn redacts_home_dir_in_exception_value() {
        let Some(home) = dirs::home_dir().and_then(|p| p.to_str().map(str::to_owned)) else {
            return; // no home dir on this platform; nothing to assert
        };
        let mut event = Event::default();
        event.exception.values.push(sentry::protocol::Exception {
            ty: "panic".into(),
            value: Some(format!("called `Result::unwrap()` on Err: {home}/x.db")),
            ..Default::default()
        });

        let event = scrub_event(event).expect("event not dropped");
        let value = event.exception.values[0].value.as_deref().unwrap();
        assert!(!value.contains(&home), "home dir must be redacted");
        assert!(value.contains("[home]"));
    }

    #[test]
    fn redacts_temp_roots_in_exception_value() {
        let mut event = Event::default();
        event.exception.values.push(sentry::protocol::Exception {
            ty: "panic".into(),
            value: Some(
                "bind /var/folders/ab/xy/T/fletch.sock failed; cleanup of /private/tmp/box left /tmp/stray".into(),
            ),
            ..Default::default()
        });

        let event = scrub_event(event).expect("event not dropped");
        let value = event.exception.values[0].value.as_deref().unwrap();
        assert!(!value.contains("/var/folders/"), "value was: {value}");
        assert!(!value.contains("/private/tmp/"), "value was: {value}");
        assert!(!value.contains("/tmp/"), "value was: {value}");
        assert_eq!(
            value,
            "bind [tmp]/ab/xy/T/fletch.sock failed; cleanup of [tmp]/box left [tmp]/stray"
        );
    }

    #[test]
    fn redacts_home_dir_in_message() {
        let Some(home) = dirs::home_dir().and_then(|p| p.to_str().map(str::to_owned)) else {
            return; // no home dir on this platform; nothing to assert
        };
        let mut event = Event::default();
        event.message = Some(format!("could not open {home}/notes.txt"));

        let event = scrub_event(event).expect("event not dropped");
        let message = event.message.unwrap();
        assert!(!message.contains(&home), "home dir must be redacted");
        assert!(message.contains("[home]"));
    }
}
