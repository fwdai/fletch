//! Anonymous product telemetry.
//!
//! A single, process-global path for usage events (`app_opened`,
//! `agent_spawned`, `pr_opened`, …), so there is exactly one identity and one
//! consent gate. Events are sent fire-and-forget to PostHog's capture endpoint:
//! our events are low-frequency, so a per-event request is simpler than a
//! batching/queueing layer and good enough — if the network is down the event
//! is just dropped, which is acceptable for usage analytics.
//!
//! Disabled (no-op) unless a PostHog project key is baked in at build time via
//! `QUORUM_POSTHOG_KEY`, mirroring the Sentry DSN — dev and unconfigured builds
//! send nothing. Identity is a random per-install UUID (never the account
//! email); event properties carry only categorical values, never paths, repo
//! names, branches, or prompts.
//!
//! **One documented carve-out**, [`send_now`]: user-submitted feedback (see
//! `commands::submit_feedback`). It rides this pipeline because it's the same
//! transport, but it ignores the consent gate, awaits its result, and may carry
//! free text the user typed — including a contact email they chose to leave in
//! the field. That's an explicit Send press with a stated destination, not
//! passive analytics, so the consent flag (which governs *usage* tracking) must
//! not silence it. Identity is still the anonymous UUID: nothing here calls
//! `$identify`, so the email is one event property and never a person profile.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Map, Value};
use tokio::sync::Notify;

static TELEMETRY: OnceLock<Telemetry> = OnceLock::new();

struct Telemetry {
    api_key: &'static str,
    capture_url: String,
    distinct_id: String,
    /// Properties attached to every event (app version, channel, os, arch).
    super_props: Map<String, Value>,
    enabled: AtomicBool,
    client: reqwest::Client,
    /// Count of send tasks still in flight, so `flush` can wait for them on
    /// shutdown instead of letting the runtime cancel them mid-request.
    inflight: AtomicUsize,
    /// Notified whenever `inflight` drops back to zero.
    idle: Notify,
}

/// PostHog project (capture) key, baked in at build time. Empty/unset disables
/// telemetry entirely. Project keys are write-only and safe to ship.
fn api_key() -> Option<&'static str> {
    option_env!("QUORUM_POSTHOG_KEY").filter(|s| !s.is_empty())
}

/// PostHog ingestion host; overrideable for self-hosted instances.
fn host() -> &'static str {
    match option_env!("QUORUM_POSTHOG_HOST") {
        Some(h) if !h.is_empty() => h,
        _ => "https://us.i.posthog.com",
    }
}

/// Initialize the global pipeline. No-op when no PostHog key is baked in, or if
/// already initialized. `distinct_id` is the caller-supplied anonymous id;
/// `enabled` is the persisted opt-out consent flag.
pub fn init(distinct_id: String, enabled: bool, version: String) {
    let Some(api_key) = api_key() else { return };

    let mut super_props = Map::new();
    super_props.insert("app_version".into(), json!(version));
    super_props.insert(
        "app_channel".into(),
        json!(if cfg!(debug_assertions) {
            "dev"
        } else {
            "release"
        }),
    );
    super_props.insert("os".into(), json!(std::env::consts::OS));
    super_props.insert("arch".into(), json!(std::env::consts::ARCH));

    let _ = TELEMETRY.set(Telemetry {
        api_key,
        capture_url: format!("{}/capture/", host().trim_end_matches('/')),
        distinct_id,
        super_props,
        enabled: AtomicBool::new(enabled),
        client: reqwest::Client::new(),
        inflight: AtomicUsize::new(0),
        idle: Notify::new(),
    });
}

/// The capture body for one event: the super-props plus the anonymous identity,
/// overlaid with the caller's props (so a caller can override a super-prop).
fn build_body(tel: &Telemetry, event: &str, props: Value) -> Value {
    let mut properties = tel.super_props.clone();
    properties.insert("distinct_id".into(), json!(tel.distinct_id));
    if let Some(obj) = props.as_object() {
        for (k, v) in obj {
            properties.insert(k.clone(), v.clone());
        }
    }

    json!({
        "api_key": tel.api_key,
        "event": event,
        "properties": Value::Object(properties),
    })
}

/// Keeps the in-flight counter honest across both send paths. Registering
/// happens on the caller's thread — before any `spawn` — so a `flush` racing a
/// `track` always sees the send; the decrement rides `Drop`, so a task cancelled
/// during runtime teardown still releases its slot.
struct InflightGuard(&'static Telemetry);

impl InflightGuard {
    fn register(tel: &'static Telemetry) -> Self {
        tel.inflight.fetch_add(1, Ordering::SeqCst);
        Self(tel)
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        if self.0.inflight.fetch_sub(1, Ordering::SeqCst) == 1 {
            // `notify_one`, not `notify_waiters`: it stores a permit when no
            // waiter is registered yet, so a `flush` that completes its counter
            // check just before this fires still wakes on its next `.await`
            // instead of stalling until the timeout. Safe because there is at
            // most one flush caller (app exit).
            self.0.idle.notify_one();
        }
    }
}

/// POST one capture body. Non-2xx is an error, not a silent success — the only
/// caller that surfaces it is [`send_now`], but `track` logs it too.
async fn post(tel: &Telemetry, body: Value) -> Result<(), String> {
    // `Client` is internally ref-counted, so cloning just shares the pool.
    let response = tel
        .client
        .clone()
        .post(&tel.capture_url)
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("capture endpoint returned {status}"))
    }
}

/// Record an event. No-op when telemetry is uninitialized or consent is off.
/// Sends fire-and-forget so the caller never blocks on the network.
pub fn track(event: &str, props: Value) {
    let Some(tel) = TELEMETRY.get() else { return };
    if !tel.enabled.load(Ordering::Relaxed) {
        return;
    }

    let body = build_body(tel, event, props);
    let guard = InflightGuard::register(tel);
    tauri::async_runtime::spawn(async move {
        let _guard = guard;
        if let Err(e) = post(tel, body).await {
            tracing::debug!(error = %e, "telemetry: send failed");
        }
    });
}

/// Send an event the user explicitly asked to send (feedback), awaiting the
/// result so the UI can report success or failure instead of pretending.
///
/// Deliberately **ignores the consent flag** — see the carve-out in this
/// module's docs. Still hard-gated on a baked-in PostHog key: an unconfigured
/// build has nowhere to send, and says so rather than swallowing the message.
pub async fn send_now(event: &str, props: Value) -> Result<(), String> {
    let Some(tel) = TELEMETRY.get() else {
        return Err("this build has no feedback endpoint configured".into());
    };
    let _guard = InflightGuard::register(tel);
    post(tel, build_body(tel, event, props)).await
}

/// Wait up to `timeout` for in-flight sends to drain. Called on app exit so the
/// last few events (e.g. one fired right before quit) aren't cancelled when the
/// async runtime tears down — the same shutdown courtesy PostHog's own SDKs
/// extend. Best-effort: anything still outstanding past the deadline is dropped.
pub async fn flush(timeout: Duration) {
    let Some(tel) = TELEMETRY.get() else { return };
    let _ = tokio::time::timeout(timeout, async {
        loop {
            if tel.inflight.load(Ordering::SeqCst) == 0 {
                break;
            }
            // A task draining to zero between this check and the await isn't
            // lost: `notify_one` leaves a permit, so this returns immediately
            // and the next iteration sees the counter at zero.
            tel.idle.notified().await;
        }
    })
    .await;
}

/// Flip consent live (from the settings toggle). Takes effect on the next
/// `track` — nothing is buffered, so there's nothing to flush or drop.
pub fn set_enabled(enabled: bool) {
    if let Some(tel) = TELEMETRY.get() {
        tel.enabled.store(enabled, Ordering::Relaxed);
    }
}
