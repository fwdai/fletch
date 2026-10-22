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
//! send nothing. Identity (see [`Identity`]) is a salted hash of the OS machine
//! id, so a reinstall that wipes the app data stays the same PostHog person —
//! never the account email, and a random UUID where the OS has no machine id.
//! Event properties carry only categorical values, never paths, repo names,
//! branches, or prompts.
//!
//! **One documented carve-out**, [`send_now`]: user-submitted feedback (see
//! `commands::submit_feedback`). It rides this pipeline because it's the same
//! transport, but it ignores the consent gate, awaits its result, and may carry
//! free text the user typed — including a contact email they chose to leave in
//! the field. That's an explicit Send press with a stated destination, not
//! passive analytics, so the consent flag (which governs *usage* tracking) must
//! not silence it. Identity is still the anonymous UUID: nothing here calls
//! `$identify`, so the email is one event property and never a person profile.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::database;

static TELEMETRY: OnceLock<Telemetry> = OnceLock::new();

/// `settings` key holding the PostHog `distinct_id`.
pub const DISTINCT_ID_KEY: &str = "telemetry_distinct_id";
/// `settings` key holding when this install was first seen, RFC 3339.
pub const INSTALLED_AT_KEY: &str = "telemetry_installed_at";

/// Who this install is to PostHog: the `distinct_id` every event carries, and
/// when the install was first seen, from which every event's
/// `days_since_install` is computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub distinct_id: String,
    pub installed_at: DateTime<Utc>,
}

impl Identity {
    /// Read the identity from `settings`, minting and persisting whatever is
    /// missing.
    ///
    /// A new install takes the hashed machine id ([`machine_id`]) as its
    /// `distinct_id`, so a reinstall that wipes the app data lands on the same
    /// PostHog person instead of counting as a new user. An install that
    /// already minted a random UUID keeps it: switching the whole existing base
    /// to a new id at once would count every current user as new on the day
    /// this ships, which is the very distortion the machine id is here to
    /// remove. Where the OS exposes no machine id, a UUID is minted as before.
    ///
    /// `installed_at` falls back to the birth time of `db_path` for installs
    /// that predate the key — the DB is created on first launch, so on a
    /// filesystem that records one, that is the install date — and to now
    /// elsewhere.
    pub fn load(conn: &Connection, db_path: &Path) -> Self {
        let distinct_id = match database::get_setting(conn, DISTINCT_ID_KEY) {
            Some(id) if !id.trim().is_empty() => id,
            _ => {
                let id = machine_id().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let _ = database::set_setting(conn, DISTINCT_ID_KEY, &id);
                id
            }
        };
        let installed_at = match database::get_setting(conn, INSTALLED_AT_KEY)
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        {
            Some(t) => t.with_timezone(&Utc),
            None => {
                let born = std::fs::metadata(db_path)
                    .and_then(|m| m.created())
                    .map(DateTime::<Utc>::from)
                    .unwrap_or_else(|_| Utc::now());
                let _ = database::set_setting(conn, INSTALLED_AT_KEY, &born.to_rfc3339());
                born
            }
        };
        Self {
            distinct_id,
            installed_at,
        }
    }
}

/// A stable, pseudonymous id for this machine: SHA-256 of the OS machine id
/// (`IOPlatformUUID` on macOS, `/etc/machine-id` on Linux, `MachineGuid` on
/// Windows) under a Fletch-only prefix, so the raw hardware id never leaves the
/// machine and the value cannot be joined with another product's. `None` where
/// the OS exposes no machine id (a minimal container, say).
pub fn machine_id() -> Option<String> {
    let raw = machine_uid::get().ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(b"fletch-telemetry:");
    hasher.update(raw.as_bytes());
    Some(format!("{:x}", hasher.finalize()))
}

/// Whole days between the install and `now`, never negative (a clock set back
/// past the install date reads as day 0 rather than as nonsense).
fn days_since(installed_at: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
    (now - installed_at).num_days().max(0)
}

struct Telemetry {
    api_key: &'static str,
    capture_url: String,
    distinct_id: String,
    installed_at: DateTime<Utc>,
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
/// already initialized. `identity` is the install's persisted [`Identity`];
/// `enabled` is the persisted opt-out consent flag.
pub fn init(identity: Identity, enabled: bool, version: String) {
    let Some(api_key) = api_key() else { return };
    let Identity {
        distinct_id,
        installed_at,
    } = identity;

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
        installed_at,
        super_props,
        enabled: AtomicBool::new(enabled),
        client: reqwest::Client::new(),
        inflight: AtomicUsize::new(0),
        idle: Notify::new(),
    });
}

/// The capture body for one event: the super-props plus the anonymous identity
/// and the install's age, overlaid with the caller's props (so a caller can
/// override a super-prop).
///
/// `days_since_install` is computed per event rather than once at `init`: a
/// desktop app stays open for days, and an event on day 3 of a launch that
/// happened on day 0 should say 3.
fn build_body(tel: &Telemetry, event: &str, props: Value) -> Value {
    let mut properties = tel.super_props.clone();
    properties.insert("distinct_id".into(), json!(tel.distinct_id));
    properties.insert(
        "days_since_install".into(),
        json!(days_since(tel.installed_at, Utc::now())),
    );
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
    crate::host::spawn(async move {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    fn test_db() -> (
        tempfile::TempDir,
        std::sync::Arc<parking_lot::Mutex<Connection>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let db = database::init(dir.path()).unwrap();
        (dir, db)
    }

    fn telemetry(installed_at: DateTime<Utc>) -> Telemetry {
        Telemetry {
            api_key: "key",
            capture_url: "http://localhost/capture/".into(),
            distinct_id: "person".into(),
            installed_at,
            super_props: Map::new(),
            enabled: AtomicBool::new(true),
            client: reqwest::Client::new(),
            inflight: AtomicUsize::new(0),
            idle: Notify::new(),
        }
    }

    /// A fresh install is identified by the machine, so a reinstall that wipes
    /// the DB mints the very same id; only a machine without one gets a UUID.
    #[test]
    fn a_fresh_install_takes_the_machine_id() {
        let (dir, db) = test_db();
        let db_path = dir.path().join(database::DB_FILENAME);
        let first = Identity::load(&db.lock(), &db_path);
        match machine_id() {
            Some(expected) => assert_eq!(first.distinct_id, expected),
            None => {
                uuid::Uuid::parse_str(&first.distinct_id)
                    .expect("a UUID when there is no machine id");
            }
        }
        assert_eq!(
            database::get_setting(&db.lock(), DISTINCT_ID_KEY).as_deref(),
            Some(first.distinct_id.as_str())
        );

        // Wiping the DB — the reinstall — lands on the same person.
        let (dir2, db2) = test_db();
        let again = Identity::load(&db2.lock(), &dir2.path().join(database::DB_FILENAME));
        if machine_id().is_some() {
            assert_eq!(again.distinct_id, first.distinct_id);
        }
    }

    /// An install that already has an id keeps it: nobody in the existing base
    /// becomes a new user on the day the machine id ships.
    #[test]
    fn an_existing_install_keeps_its_uuid() {
        let (dir, db) = test_db();
        database::set_setting(&db.lock(), DISTINCT_ID_KEY, "legacy-uuid").unwrap();
        let id = Identity::load(&db.lock(), &dir.path().join(database::DB_FILENAME));
        assert_eq!(id.distinct_id, "legacy-uuid");
    }

    /// The install date is minted once and read back verbatim after that.
    #[test]
    fn installed_at_is_minted_once_and_persisted() {
        let (dir, db) = test_db();
        let db_path = dir.path().join(database::DB_FILENAME);
        let before = Utc::now();
        let first = Identity::load(&db.lock(), &db_path);
        let after = Utc::now();
        // The DB was just created, so its birth time and "now" agree to within
        // the test's own runtime (a filesystem may round to the second).
        assert!(first.installed_at >= before - TimeDelta::seconds(2));
        assert!(first.installed_at <= after);

        let second = Identity::load(&db.lock(), &db_path);
        assert_eq!(second, first);
    }

    /// A recorded install date wins over the DB file's age.
    #[test]
    fn a_recorded_install_date_is_honoured() {
        let (dir, db) = test_db();
        database::set_setting(&db.lock(), INSTALLED_AT_KEY, "2024-03-01T12:00:00+00:00").unwrap();
        let id = Identity::load(&db.lock(), &dir.path().join(database::DB_FILENAME));
        assert_eq!(id.installed_at.to_rfc3339(), "2024-03-01T12:00:00+00:00");
    }

    /// A missing DB file (nothing to read a birth time from) still yields an
    /// install date — now — rather than failing identity.
    #[test]
    fn a_missing_db_file_falls_back_to_now() {
        let (dir, db) = test_db();
        let before = Utc::now();
        let id = Identity::load(&db.lock(), &dir.path().join("nope.db"));
        assert!(id.installed_at >= before && id.installed_at <= Utc::now());
    }

    #[test]
    fn days_since_counts_whole_days_and_never_goes_negative() {
        let t0 = Utc::now();
        assert_eq!(days_since(t0, t0), 0);
        assert_eq!(days_since(t0, t0 + TimeDelta::hours(47)), 1);
        assert_eq!(days_since(t0, t0 + TimeDelta::days(30)), 30);
        assert_eq!(days_since(t0, t0 - TimeDelta::days(3)), 0);
    }

    /// Every event carries the install's age as of the event, not of launch.
    #[test]
    fn every_event_carries_days_since_install() {
        let tel = telemetry(Utc::now() - TimeDelta::days(7) - TimeDelta::hours(1));
        let body = build_body(&tel, "app_opened", json!({ "k": "v" }));
        assert_eq!(body["event"], "app_opened");
        assert_eq!(body["properties"]["distinct_id"], "person");
        assert_eq!(body["properties"]["days_since_install"], 7);
        assert_eq!(body["properties"]["k"], "v");
    }
}
