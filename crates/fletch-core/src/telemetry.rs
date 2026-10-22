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
//! An install from before the machine id is merged onto it once with
//! [`adopt_machine_id`]. The install date lives in a file under `~/.fletch`
//! ([`installed_at_path`]) rather than in the app-data DB, so it too survives a
//! reinstall. Event properties carry only categorical values, never paths, repo
//! names, branches, or prompts.
//!
//! Besides the events callers raise, [`start_heartbeat`] sends `app_heartbeat`
//! once per UTC day the app stays running after the day it was opened, so a
//! machine that keeps Fletch open for a week still shows up on each of those
//! days.
//!
//! **One documented carve-out**, [`send_now`]: user-submitted feedback (see
//! `commands::submit_feedback`). It rides this pipeline because it's the same
//! transport, but it ignores the consent gate, awaits its result, and may carry
//! free text the user typed — including a contact email they chose to leave in
//! the field. That's an explicit Send press with a stated destination, not
//! passive analytics, so the consent flag (which governs *usage* tracking) must
//! not silence it. Because it is sent regardless of the opt-out, it carries
//! none of the install's identity: a throwaway random id per submission, no
//! `days_since_install`, and `$process_person_profile: false` so PostHog keeps
//! it a personless event. Nothing here calls `$identify`, so the email is one
//! event property and never a person profile.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tokio::time::MissedTickBehavior;

use crate::database;
use crate::DbState;

static TELEMETRY: OnceLock<Telemetry> = OnceLock::new();

/// `settings` key holding the PostHog `distinct_id`.
pub const DISTINCT_ID_KEY: &str = "telemetry_distinct_id";

/// Where the install date lives: `~/.fletch/installed_at` (release) or
/// `~/.fletch/dev/installed_at` (debug), one RFC 3339 timestamp. Under the
/// Fletch-owned root rather than the app-data dir on purpose: an uninstaller
/// that sweeps `~/Library/Application Support/<bundle>` leaves `~/.fletch`
/// alone, so the day count survives the same reinstall the machine id does.
/// `None` only when there is no home directory to put it in.
pub fn installed_at_path() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join(".fletch")
            .join(crate::build_state_subpath("installed_at")),
    )
}

/// Who this install is to PostHog: the `distinct_id` every event carries, and
/// when the install was first seen, from which every event's
/// `days_since_install` is computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub distinct_id: String,
    pub installed_at: DateTime<Utc>,
    /// Set while this install still sends as a pre-machine-id UUID on a
    /// machine that has an id: the id its person should be merged onto. See
    /// [`adopt_machine_id`].
    pub machine_id_to_adopt: Option<String>,
}

impl Identity {
    /// Read the identity, minting and persisting whatever is missing: the
    /// `distinct_id` from `settings`, the install date from
    /// `installed_at_file`.
    ///
    /// A new install takes the hashed machine id ([`machine_id`]) as its
    /// `distinct_id`, so a reinstall that wipes the app data lands on the same
    /// PostHog person instead of counting as a new user. An install that
    /// already minted a random UUID keeps sending as it — switching the whole
    /// existing base to a new id at once would count every current user as new
    /// on the day this ships — and is handed the machine id to adopt instead,
    /// which [`adopt_machine_id`] merges it onto. Where the OS exposes no
    /// machine id, a UUID is minted as before.
    ///
    /// `installed_at` falls back to the birth time of `db_path` for installs
    /// that predate the file — the DB is created on first launch, so on a
    /// filesystem that records one, that is the install date — and to now
    /// elsewhere. A file that cannot be written is not an error: the next
    /// launch derives the same value again.
    pub fn load(conn: &Connection, db_path: &Path, installed_at_file: &Path) -> Self {
        let machine = machine_id();
        let (distinct_id, machine_id_to_adopt) = match database::get_setting(conn, DISTINCT_ID_KEY)
        {
            Some(id) if !id.trim().is_empty() => {
                let adopt = machine.filter(|m| *m != id);
                (id, adopt)
            }
            _ => {
                let id = machine.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let _ = database::set_setting(conn, DISTINCT_ID_KEY, &id);
                (id, None)
            }
        };
        let installed_at = match std::fs::read_to_string(installed_at_file)
            .ok()
            .and_then(|s| DateTime::parse_from_rfc3339(s.trim()).ok())
        {
            Some(t) => t.with_timezone(&Utc),
            None => {
                let born = std::fs::metadata(db_path)
                    .and_then(|m| m.created())
                    .map(DateTime::<Utc>::from)
                    .unwrap_or_else(|_| Utc::now());
                if let Some(parent) = installed_at_file.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(installed_at_file, format!("{}\n", born.to_rfc3339()));
                born
            }
        };
        Self {
            distinct_id,
            installed_at,
            machine_id_to_adopt,
        }
    }
}

/// Merge this install's pre-machine-id person onto its machine id on PostHog,
/// then record the machine id as the `distinct_id` every later launch sends as.
/// No-op unless `identity` carries a machine id to adopt.
///
/// This process keeps sending as the UUID: PostHog attributes both ids to one
/// person once the `$create_alias` lands, so nothing is split, and the live
/// identity never has to change under running senders. The stored id flips only
/// after the endpoint accepted the alias, so a dropped request leaves the
/// install exactly where it was, to try again next launch. Consent is honoured:
/// an opted-out install sends no alias and keeps its UUID.
pub fn adopt_machine_id(db: DbState, identity: &Identity) {
    let Some(machine_id) = identity.machine_id_to_adopt.clone() else {
        return;
    };
    let Some(tel) = TELEMETRY.get() else { return };
    if !tel.enabled.load(Ordering::Relaxed) {
        return;
    }
    let body = alias_body(tel, &machine_id);
    let guard = InflightGuard::register(tel);
    crate::host::spawn(async move {
        let _guard = guard;
        match post(tel, body).await {
            Ok(()) => {
                let _ = database::set_setting(&db.lock(), DISTINCT_ID_KEY, &machine_id);
                tracing::info!("telemetry: install merged onto its machine id");
            }
            Err(e) => tracing::debug!(error = %e, "telemetry: alias not sent"),
        }
    });
}

/// PostHog's `$create_alias`: `distinct_id` is the person with the history
/// (our UUID), `alias` the id to fold into it (the machine id). The super-props
/// ride along like on any event; PostHog ignores what it does not read.
fn alias_body(tel: &Telemetry, alias: &str) -> Value {
    build_body(tel, "$create_alias", json!({ "alias": alias }))
}

/// How often the heartbeat loop looks at the calendar. Coarse on purpose: the
/// event is per day, and a laptop that slept through the boundary sends on the
/// first tick after it wakes.
const HEARTBEAT_POLL: Duration = Duration::from_secs(15 * 60);

/// Start sending `app_heartbeat` once per UTC day the app stays running, from
/// the day after the one it was started on (that day already has its
/// `app_opened`). Idempotent; a no-op in an unconfigured build.
///
/// Call it from where `app_opened` is sent, so the heartbeat sits behind the
/// same gate: on a fresh install `app_opened` waits for the data-sharing
/// disclosure, and the heartbeat must not run ahead of it.
pub fn start_heartbeat() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) || TELEMETRY.get().is_none() {
        return;
    }
    crate::host::spawn(async move {
        let mut last_sent = Utc::now().date_naive();
        let mut ticks = tokio::time::interval(HEARTBEAT_POLL);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // An `interval`'s first tick completes at once.
        ticks.tick().await;
        loop {
            ticks.tick().await;
            let today = Utc::now().date_naive();
            if heartbeat_due(last_sent, today) {
                last_sent = today;
                track("app_heartbeat", json!({}));
            }
        }
    });
}

/// A heartbeat goes out the first time the loop sees a day other than the one
/// it last sent (or started) on.
fn heartbeat_due(last_sent: NaiveDate, today: NaiveDate) -> bool {
    today != last_sent
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

/// The OS release, e.g. "15.1" on macOS — `os` alone says only "macos", and
/// which releases people install onto is what decides what the app may
/// depend on. Best-effort and categorical: a version string, never a host
/// name or anything else `sw_vers` could say.
fn os_version() -> Option<String> {
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return (out.status.success() && !v.is_empty()).then_some(v);
    }
    None
}

/// Initialize the global pipeline. No-op when no PostHog key is baked in, or if
/// already initialized. `identity` is the install's persisted [`Identity`];
/// `enabled` is the persisted opt-out consent flag.
pub fn init(identity: Identity, enabled: bool, version: String) {
    let Some(api_key) = api_key() else { return };
    let Identity {
        distinct_id,
        installed_at,
        ..
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
    if let Some(v) = os_version() {
        super_props.insert("os_version".into(), json!(v));
    }

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

/// The capture body for one usage event: the super-props plus the install's
/// identity and age, overlaid with the caller's props (so a caller can override
/// a super-prop).
///
/// `days_since_install` is computed per event rather than once at `init`: a
/// desktop app stays open for days, and an event on day 3 of a launch that
/// happened on day 0 should say 3.
fn build_body(tel: &Telemetry, event: &str, props: Value) -> Value {
    let mut properties = Map::new();
    properties.insert(
        "days_since_install".into(),
        json!(days_since(tel.installed_at, Utc::now())),
    );
    if let Some(obj) = props.as_object() {
        properties.extend(obj.clone());
    }
    body_as(tel, &tel.distinct_id, event, properties)
}

/// The capture body for a feedback submission: the super-props and the
/// caller's props under a **throwaway** identity. A fresh random id per
/// submission, never the install's machine id, and no `days_since_install`;
/// `$process_person_profile: false` tells PostHog to keep it a personless event
/// rather than mint a person for the id. Feedback is sent regardless of the
/// analytics opt-out, so it must carry nothing that could identify the install
/// or tie the message to its usage history — the modal promises the app version
/// and platform, nothing else.
fn feedback_body(tel: &Telemetry, event: &str, props: Value) -> Value {
    let mut properties = Map::new();
    properties.insert("$process_person_profile".into(), json!(false));
    if let Some(obj) = props.as_object() {
        properties.extend(obj.clone());
    }
    body_as(tel, &uuid::Uuid::new_v4().to_string(), event, properties)
}

/// One capture body: the super-props, `distinct_id`, then `properties` on top.
fn body_as(
    tel: &Telemetry,
    distinct_id: &str,
    event: &str,
    properties: Map<String, Value>,
) -> Value {
    let mut all = tel.super_props.clone();
    all.insert("distinct_id".into(), json!(distinct_id));
    all.extend(properties);
    json!({
        "api_key": tel.api_key,
        "event": event,
        "properties": Value::Object(all),
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

/// Whether [`track`] would send anything right now: a key is baked in and
/// consent is on. For callers whose event costs something to assemble (a
/// subprocess, a DB read) — skip the work when it would only be dropped.
pub fn is_enabled() -> bool {
    TELEMETRY
        .get()
        .is_some_and(|tel| tel.enabled.load(Ordering::Relaxed))
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
    post(tel, feedback_body(tel, event, props)).await
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

    /// The install-date file as the app lays it out: a sibling tree of the DB,
    /// nested the way `~/.fletch/dev/installed_at` is so the parent has to be
    /// created.
    fn stamp(dir: &Path) -> PathBuf {
        dir.join("fletch").join("dev").join("installed_at")
    }

    /// A fresh install is identified by the machine, so a reinstall that wipes
    /// the DB mints the very same id; only a machine without one gets a UUID.
    #[test]
    fn a_fresh_install_takes_the_machine_id() {
        let (dir, db) = test_db();
        let db_path = dir.path().join(database::DB_FILENAME);
        let first = Identity::load(&db.lock(), &db_path, &stamp(dir.path()));
        match machine_id() {
            Some(expected) => assert_eq!(first.distinct_id, expected),
            None => {
                uuid::Uuid::parse_str(&first.distinct_id)
                    .expect("a UUID when there is no machine id");
            }
        }
        assert_eq!(first.machine_id_to_adopt, None);
        assert_eq!(
            database::get_setting(&db.lock(), DISTINCT_ID_KEY).as_deref(),
            Some(first.distinct_id.as_str())
        );

        // Wiping the DB — the reinstall — lands on the same person.
        let (dir2, db2) = test_db();
        let again = Identity::load(
            &db2.lock(),
            &dir2.path().join(database::DB_FILENAME),
            &stamp(dir2.path()),
        );
        if machine_id().is_some() {
            assert_eq!(again.distinct_id, first.distinct_id);
        }
    }

    /// An install that already has an id keeps sending as it — nobody in the
    /// existing base becomes a new user on the day the machine id ships — and
    /// is handed the machine id to merge onto.
    #[test]
    fn an_existing_install_keeps_its_uuid_and_adopts_the_machine_id() {
        let (dir, db) = test_db();
        database::set_setting(&db.lock(), DISTINCT_ID_KEY, "legacy-uuid").unwrap();
        let db_path = dir.path().join(database::DB_FILENAME);
        let id = Identity::load(&db.lock(), &db_path, &stamp(dir.path()));
        assert_eq!(id.distinct_id, "legacy-uuid");
        assert_eq!(id.machine_id_to_adopt, machine_id());

        // Once the alias landed and the machine id is the stored id, there is
        // nothing left to adopt.
        if let Some(m) = machine_id() {
            database::set_setting(&db.lock(), DISTINCT_ID_KEY, &m).unwrap();
            let id = Identity::load(&db.lock(), &db_path, &stamp(dir.path()));
            assert_eq!(id.distinct_id, m);
            assert_eq!(id.machine_id_to_adopt, None);
        }
    }

    /// The install date is minted once, written to the file, and read back
    /// verbatim after that — including by a reinstall with a brand-new DB.
    #[test]
    fn installed_at_is_minted_once_and_survives_a_new_db() {
        let (dir, db) = test_db();
        let db_path = dir.path().join(database::DB_FILENAME);
        let file = stamp(dir.path());
        let before = Utc::now();
        let first = Identity::load(&db.lock(), &db_path, &file);
        let after = Utc::now();
        // The DB was just created, so its birth time and "now" agree to within
        // the test's own runtime (a filesystem may round to the second).
        assert!(first.installed_at >= before - TimeDelta::seconds(2));
        assert!(first.installed_at <= after);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap().trim(),
            first.installed_at.to_rfc3339()
        );

        let second = Identity::load(&db.lock(), &db_path, &file);
        assert_eq!(second.installed_at, first.installed_at);

        // A reinstall: new DB, same file.
        let (dir2, db2) = test_db();
        let third = Identity::load(&db2.lock(), &dir2.path().join(database::DB_FILENAME), &file);
        assert_eq!(third.installed_at, first.installed_at);
    }

    /// A recorded install date wins over the DB file's age.
    #[test]
    fn a_recorded_install_date_is_honoured() {
        let (dir, db) = test_db();
        let file = stamp(dir.path());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "2024-03-01T12:00:00+00:00\n").unwrap();
        let id = Identity::load(&db.lock(), &dir.path().join(database::DB_FILENAME), &file);
        assert_eq!(id.installed_at.to_rfc3339(), "2024-03-01T12:00:00+00:00");
    }

    /// A missing DB file (nothing to read a birth time from) and an unwritable
    /// install-date file still yield an install date — now — rather than
    /// failing identity.
    #[test]
    fn missing_db_and_unwritable_file_fall_back_to_now() {
        let (dir, db) = test_db();
        let before = Utc::now();
        // A path under a *file* cannot be created.
        let unwritable = dir.path().join(database::DB_FILENAME).join("installed_at");
        let id = Identity::load(&db.lock(), &dir.path().join("nope.db"), &unwritable);
        assert!(id.installed_at >= before && id.installed_at <= Utc::now());
    }

    /// The alias names the UUID as the person and the machine id as the alias
    /// to fold into it — the direction PostHog merges in.
    #[test]
    fn the_alias_folds_the_machine_id_into_the_uuid_person() {
        let tel = telemetry(Utc::now());
        let body = alias_body(&tel, "machine");
        assert_eq!(body["event"], "$create_alias");
        assert_eq!(body["properties"]["distinct_id"], "person");
        assert_eq!(body["properties"]["alias"], "machine");
    }

    /// Feedback goes out regardless of the opt-out, so it must carry nothing
    /// that identifies the install: a throwaway id that differs per
    /// submission, no install age, and no person profile — on top of the same
    /// super-props and the caller's props as any event.
    #[test]
    fn feedback_carries_a_throwaway_identity_and_no_install_age() {
        let mut tel = telemetry(Utc::now() - TimeDelta::days(7));
        tel.super_props.insert("app_version".into(), json!("1.2.3"));
        let first = feedback_body(&tel, "feedback_submitted", json!({ "message": "hi" }));
        let second = feedback_body(&tel, "feedback_submitted", json!({ "message": "hi" }));
        let props = &first["properties"];
        assert_eq!(first["event"], "feedback_submitted");
        assert_eq!(props["app_version"], "1.2.3");
        assert_eq!(props["message"], "hi");
        assert_eq!(props["$process_person_profile"], false);
        assert!(props.get("days_since_install").is_none());
        let id = props["distinct_id"].as_str().unwrap();
        assert_ne!(id, tel.distinct_id);
        uuid::Uuid::parse_str(id).expect("a random UUID");
        assert_ne!(id, second["properties"]["distinct_id"].as_str().unwrap());
    }

    #[test]
    fn a_heartbeat_is_due_once_the_day_changes() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
        assert!(!heartbeat_due(day, day));
        assert!(heartbeat_due(day, day.succ_opt().unwrap()));
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
