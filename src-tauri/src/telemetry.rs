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

/// Record an event. No-op when telemetry is uninitialized or consent is off.
/// Sends fire-and-forget so the caller never blocks on the network.
pub fn track(event: &str, props: Value) {
    let Some(tel) = TELEMETRY.get() else { return };
    if !tel.enabled.load(Ordering::Relaxed) {
        return;
    }

    let mut properties = tel.super_props.clone();
    properties.insert("distinct_id".into(), json!(tel.distinct_id));
    if let Some(obj) = props.as_object() {
        for (k, v) in obj {
            properties.insert(k.clone(), v.clone());
        }
    }

    let body = json!({
        "api_key": tel.api_key,
        "event": event,
        "properties": Value::Object(properties),
    });

    // `Client` is internally ref-counted, so cloning just shares the pool.
    let client = tel.client.clone();
    let url = tel.capture_url.clone();
    // Register the task before spawning so a `flush` racing this call always
    // sees it. `tel` is a `&'static`, so the task can decrement on completion.
    tel.inflight.fetch_add(1, Ordering::SeqCst);
    tauri::async_runtime::spawn(async move {
        if let Err(e) = client
            .post(&url)
            .json(&body)
            .timeout(Duration::from_secs(10))
            .send()
            .await
        {
            tracing::debug!(error = %e, "telemetry: send failed");
        }
        if tel.inflight.fetch_sub(1, Ordering::SeqCst) == 1 {
            // `notify_one`, not `notify_waiters`: it stores a permit when no
            // waiter is registered yet, so a `flush` that completes its counter
            // check just before this fires still wakes on its next `.await`
            // instead of stalling until the timeout. Safe because there is at
            // most one flush caller (app exit).
            tel.idle.notify_one();
        }
    });
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
