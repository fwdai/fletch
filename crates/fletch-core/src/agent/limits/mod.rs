//! Per-account plan limits: the five-hour and weekly windows a provider's own
//! usage page shows, as percent used and reset time.
//!
//! Neither vendor publishes token counts against a limit, so percent is the
//! whole reading. Four sources report it, each in its own units — a 0–1
//! fraction (claude's stream), a 0–100 percent (codex's app-server and
//! rollouts, claude's OAuth usage endpoint), epoch seconds or ISO 8601 for the
//! reset — and every one is normalised here, at the boundary, to percent 0–100
//! and epoch seconds. Nothing past this module sees a source's own units.
//!
//! The last known reading per `(provider, account)` is one JSON `settings` row
//! ([`limits_setting_key`]), written through [`record_limits`] /
//! [`record_refresh`] so every write announces `settings:changed` and the
//! Settings pane follows without polling. The row also carries the manual
//! refresh's outcome and back-off, so a restart can't reset a 429 back-off.
//!
//! Mirrored in `src/api/types/providers.ts` (`AccountLimits`); keep the two in
//! step.

pub mod app_server;
pub mod oauth_usage;

use std::collections::BTreeMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::accounts;
use crate::error::Result;
use crate::host::EngineCtx;

/// `settings` key prefix of the limits rows: `provider_limits_<provider>_<account>`,
/// `account` being a managed id or [`accounts::DEFAULT_ACCOUNT`]. Neither part
/// can hold an underscore (provider ids and account slugs don't), so the key
/// splits unambiguously — `limitsKeyParts` in the store relies on that.
pub const LIMITS_SETTING_PREFIX: &str = "provider_limits_";

/// The least time between two successful manual refreshes of one account. A
/// reading this fresh is what the vendor would answer anyway, and the OAuth
/// endpoint rate-limits eagerly.
pub const REFRESH_FLOOR_SECS: i64 = 60;

/// First back-off after a 429, doubled per consecutive 429 up to
/// [`MAX_BACKOFF_SECS`].
const BASE_BACKOFF_SECS: i64 = 120;
const MAX_BACKOFF_SECS: i64 = 3600;

/// A window this long or longer is the weekly one. Sources that name their
/// windows by duration (codex: 300 and 10080 minutes) are slotted by it, so a
/// reordering of primary/secondary can't swap the meters.
const DAY_MINUTES: i64 = 24 * 60;

pub fn limits_setting_key(provider: &str, account: Option<&str>) -> String {
    let account = account
        .filter(|a| !accounts::is_default(a))
        .unwrap_or(accounts::DEFAULT_ACCOUNT);
    format!("{LIMITS_SETTING_PREFIX}{provider}_{account}")
}

/// One window: how much of it is used, and when it starts over.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LimitWindow {
    /// 0–100.
    pub percent: f64,
    /// Epoch seconds. `None` when the source reports no reset — the OAuth
    /// endpoint does that for a window with nothing used in it yet.
    pub resets_at: Option<i64>,
}

/// Where a reading came from, shown beside its age so a stale passive reading
/// isn't mistaken for a fresh query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitSource {
    /// Claude's managed stream-json `rate_limit_event`.
    Stream,
    /// Claude's status-line payload. Not produced yet; reserved so the stored
    /// shape doesn't change when it is.
    Statusline,
    /// Codex's `account/rateLimits/read` over `codex app-server`.
    AppServer,
    /// Claude's OAuth usage endpoint, on an explicit refresh.
    OauthUsage,
    /// The `rate_limits` of the last `token_count` in a codex rollout.
    Rollout,
}

/// A reading of both windows at one instant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderLimits {
    pub five_hour: Option<LimitWindow>,
    pub seven_day: Option<LimitWindow>,
    /// Epoch seconds the reading describes — the event's own time for a
    /// rollout, the time of the query otherwise.
    pub as_of: i64,
    pub source: LimitSource,
}

/// How the last manual refresh of an account ended, when it didn't produce a
/// reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshStatus {
    Ok,
    /// No login to ask with: no credential (claude) or the app-server's
    /// unauthenticated `-32603` (codex).
    SignedOut,
    /// Claude's stored access token was refused. Claude refreshes it only when
    /// it runs, so the fix is to run an agent under the account.
    Stale,
    /// The endpoint answered 429; see `next_allowed_at`.
    RateLimited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshState {
    pub status: RefreshStatus,
    /// Epoch seconds of the attempt.
    pub at: i64,
    /// Epoch seconds before which another attempt is refused (429 back-off).
    #[serde(default)]
    pub next_allowed_at: Option<i64>,
    /// Consecutive 429s, the back-off's exponent.
    #[serde(default)]
    pub failures: u32,
}

/// The settings row of one account: the last known reading and how the last
/// manual refresh went. Either may be absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountLimits {
    #[serde(default)]
    pub limits: Option<ProviderLimits>,
    #[serde(default)]
    pub refresh: Option<RefreshState>,
}

/// What a manual refresh learned.
#[derive(Debug, Clone, PartialEq)]
pub enum RefreshOutcome {
    Limits(ProviderLimits),
    SignedOut,
    Stale,
    RateLimited,
}

pub fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── normalisation ───────────────────────────────────────────────────────────

/// Clamp to 0–100; a source past its limit can report over 100.
fn clamp_percent(p: f64) -> f64 {
    if p.is_finite() {
        p.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

/// Epoch seconds from a number that may be seconds or milliseconds. Every
/// source documents seconds; one that switched to milliseconds would otherwise
/// put the reset thirty thousand years out.
fn epoch_secs(v: &Value) -> Option<i64> {
    let n = v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))?;
    Some(if n > 100_000_000_000 { n / 1000 } else { n })
}

fn iso_secs(v: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|d| d.timestamp())
}

#[derive(Clone, Copy)]
enum Slot {
    FiveHour,
    SevenDay,
}

/// Slot windows by their duration where the source states one, else by the
/// position the source gave them.
fn place(windows: impl IntoIterator<Item = (Option<i64>, Slot, LimitWindow)>) -> Windows {
    let mut out = Windows::default();
    for (minutes, fallback, window) in windows {
        let slot = match minutes {
            Some(m) if m >= DAY_MINUTES => Slot::SevenDay,
            Some(m) if m > 0 => Slot::FiveHour,
            _ => fallback,
        };
        match slot {
            Slot::FiveHour => out.five_hour = Some(window),
            Slot::SevenDay => out.seven_day = Some(window),
        }
    }
    out
}

#[derive(Default)]
struct Windows {
    five_hour: Option<LimitWindow>,
    seven_day: Option<LimitWindow>,
}

impl Windows {
    fn reading(self, as_of: i64, source: LimitSource) -> Option<ProviderLimits> {
        if self.five_hour.is_none() && self.seven_day.is_none() {
            return None;
        }
        Some(ProviderLimits {
            five_hour: self.five_hour,
            seven_day: self.seven_day,
            as_of,
            source,
        })
    }
}

/// Claude's managed stream: `rate_limit_info.unifiedWindows.{five_hour,
/// seven_day}.{utilization, resetsAt}`, utilization a 0–1 fraction. `None` for
/// any other event, or one that names no window.
pub fn from_stream_event(event: &Value, now: i64) -> Option<ProviderLimits> {
    if event.get("type").and_then(Value::as_str) != Some("rate_limit_event") {
        return None;
    }
    let unified = event.pointer("/rate_limit_info/unifiedWindows")?;
    let window = |key: &str| {
        let w = unified.get(key)?;
        Some(LimitWindow {
            percent: clamp_percent(w.get("utilization")?.as_f64()? * 100.0),
            resets_at: w.get("resetsAt").and_then(epoch_secs),
        })
    };
    Windows {
        five_hour: window("five_hour"),
        seven_day: window("seven_day"),
    }
    .reading(now, LimitSource::Stream)
}

/// Claude's OAuth usage endpoint: `five_hour` / `seven_day`, each
/// `{utilization, resets_at}` with utilization a 0–100 percent and the reset
/// ISO 8601 (null for an untouched window).
pub fn from_oauth_usage(body: &Value, now: i64) -> Option<ProviderLimits> {
    let window = |key: &str| {
        let w = body.get(key).filter(|w| w.is_object())?;
        Some(LimitWindow {
            percent: clamp_percent(w.get("utilization")?.as_f64()?),
            resets_at: w.get("resets_at").and_then(iso_secs),
        })
    };
    Windows {
        five_hour: window("five_hour"),
        seven_day: window("seven_day"),
    }
    .reading(now, LimitSource::OauthUsage)
}

/// Codex's app-server `rateLimits`: `primary` / `secondary`, each
/// `{usedPercent, windowDurationMins, resetsAt}` — a 0–100 percent and epoch
/// seconds.
pub fn from_app_server(rate_limits: &Value, now: i64) -> Option<ProviderLimits> {
    let window = |key: &str, fallback: Slot| {
        let w = rate_limits.get(key).filter(|w| w.is_object())?;
        Some((
            w.get("windowDurationMins").and_then(Value::as_i64),
            fallback,
            LimitWindow {
                percent: clamp_percent(w.get("usedPercent")?.as_f64()?),
                resets_at: w.get("resetsAt").and_then(epoch_secs),
            },
        ))
    };
    place(
        [
            window("primary", Slot::FiveHour),
            window("secondary", Slot::SevenDay),
        ]
        .into_iter()
        .flatten(),
    )
    .reading(now, LimitSource::AppServer)
}

/// A codex rollout's `token_count` `rate_limits`: `primary` / `secondary`, each
/// `{used_percent, window_minutes, resets_at}`. Older CLIs wrote
/// `resets_in_seconds` instead of an absolute reset; it counts from the event.
/// `as_of` is the event's own time, in epoch seconds.
pub fn from_rollout(rate_limits: &Value, as_of: i64) -> Option<ProviderLimits> {
    let window = |key: &str, fallback: Slot| {
        let w = rate_limits.get(key).filter(|w| w.is_object())?;
        let resets_at = w.get("resets_at").and_then(epoch_secs).or_else(|| {
            w.get("resets_in_seconds")
                .and_then(Value::as_i64)
                .map(|s| as_of + s)
        });
        Some((
            w.get("window_minutes").and_then(Value::as_i64),
            fallback,
            LimitWindow {
                percent: clamp_percent(w.get("used_percent")?.as_f64()?),
                resets_at,
            },
        ))
    };
    place(
        [
            window("primary", Slot::FiveHour),
            window("secondary", Slot::SevenDay),
        ]
        .into_iter()
        .flatten(),
    )
    .reading(as_of, LimitSource::Rollout)
}

// ── refresh gating ──────────────────────────────────────────────────────────

/// Whether a manual refresh may run now: not inside a 429 back-off, and not
/// within [`REFRESH_FLOOR_SECS`] of the last successful one. A signed-out or
/// stale outcome doesn't hold the next try back — the user is expected to fix
/// it and press Refresh again straight away.
pub fn refresh_allowed(last: Option<&RefreshState>, now: i64) -> bool {
    let Some(last) = last else { return true };
    if last.next_allowed_at.is_some_and(|t| now < t) {
        return false;
    }
    !(last.status == RefreshStatus::Ok && now - last.at < REFRESH_FLOOR_SECS)
}

/// The refresh state an outcome leaves behind. Consecutive 429s double the
/// back-off; anything else resets it.
pub fn next_refresh_state(
    prev: Option<&RefreshState>,
    outcome: &RefreshOutcome,
    now: i64,
) -> RefreshState {
    let settled = |status| RefreshState {
        status,
        at: now,
        next_allowed_at: None,
        failures: 0,
    };
    match outcome {
        RefreshOutcome::Limits(_) => settled(RefreshStatus::Ok),
        RefreshOutcome::SignedOut => settled(RefreshStatus::SignedOut),
        RefreshOutcome::Stale => settled(RefreshStatus::Stale),
        RefreshOutcome::RateLimited => {
            let failures = prev
                .filter(|p| p.status == RefreshStatus::RateLimited)
                .map_or(0, |p| p.failures)
                .saturating_add(1);
            let delay = BASE_BACKOFF_SECS
                .saturating_mul(1_i64 << (failures - 1).min(10))
                .min(MAX_BACKOFF_SECS);
            RefreshState {
                status: RefreshStatus::RateLimited,
                at: now,
                next_allowed_at: Some(now + delay),
                failures,
            }
        }
    }
}

// ── storage ─────────────────────────────────────────────────────────────────

/// The stored row of one account; absent or unreadable reads as empty, so a
/// row from a future shape degrades to "no data" rather than an error.
pub fn load(conn: &Connection, provider: &str, account: Option<&str>) -> AccountLimits {
    crate::database::get_setting(conn, &limits_setting_key(provider, account))
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Every account of `provider` — the default and each managed one on disk —
/// keyed by account id, with an empty entry for an account never read.
pub fn load_provider(conn: &Connection, provider: &str) -> BTreeMap<String, AccountLimits> {
    std::iter::once(accounts::DEFAULT_ACCOUNT.to_string())
        .chain(accounts::list_account_ids(provider))
        .map(|id| {
            let row = load(conn, provider, Some(&id));
            (id, row)
        })
        .collect()
}

/// Read-modify-write one row; `change` returns whether it changed anything.
/// The lock is held for the read and the write only, and the announcement goes
/// out after it is released.
fn update(
    ctx: &EngineCtx,
    provider: &str,
    account: Option<&str>,
    change: impl FnOnce(&mut AccountLimits) -> bool,
) -> Result<Option<AccountLimits>> {
    let key = limits_setting_key(provider, account);
    let (row, raw) = {
        let conn = ctx.db.lock();
        let mut row = load(&conn, provider, account);
        if !change(&mut row) {
            return Ok(None);
        }
        let raw = serde_json::to_string(&row)?;
        crate::database::set_setting(&conn, &key, &raw)?;
        (row, raw)
    };
    crate::commands::settings::announce(ctx, &key, Some(&raw));
    Ok(Some(row))
}

/// Store a passive reading (stream, rollout) for `account` (`None` = default)
/// when it is newer than the one stored. Returns whether it was written.
/// Leaves the manual refresh's state alone.
pub fn record_limits(
    ctx: &EngineCtx,
    provider: &str,
    account: Option<&str>,
    limits: ProviderLimits,
) -> Result<bool> {
    let written = update(ctx, provider, account, |row| {
        if row.limits.as_ref().is_some_and(|l| l.as_of >= limits.as_of) {
            return false;
        }
        row.limits = Some(limits);
        true
    })?;
    Ok(written.is_some())
}

/// Store a manual refresh's outcome: its reading when it got one (the last
/// known one stays otherwise), and its refresh state. Returns the row as
/// stored.
pub fn record_refresh(
    ctx: &EngineCtx,
    provider: &str,
    account: Option<&str>,
    outcome: RefreshOutcome,
    now: i64,
) -> Result<AccountLimits> {
    let row = update(ctx, provider, account, |row| {
        row.refresh = Some(next_refresh_state(row.refresh.as_ref(), &outcome, now));
        if let RefreshOutcome::Limits(limits) = outcome {
            row.limits = Some(limits);
        }
        true
    })?;
    Ok(row.unwrap_or_default())
}

/// The supervisor's hook on a managed claude stream: a `rate_limit_event`
/// becomes the account's reading. Best-effort — a failed write costs a meter
/// update, never the agent's turn.
pub fn observe_stream_event(ctx: &EngineCtx, account: Option<&str>, event: &Value) {
    let Some(limits) = from_stream_event(event, now_secs()) else {
        return;
    };
    if let Err(e) = record_limits(ctx, "claude", account, limits) {
        tracing::warn!(error = %e, "could not record claude rate limits");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn window(percent: f64, resets_at: i64) -> Option<LimitWindow> {
        Some(LimitWindow {
            percent,
            resets_at: Some(resets_at),
        })
    }

    #[test]
    fn the_key_names_the_default_account_when_none_is_given() {
        assert_eq!(
            limits_setting_key("claude", None),
            "provider_limits_claude_default"
        );
        assert_eq!(
            limits_setting_key("claude", Some("")),
            "provider_limits_claude_default"
        );
        assert_eq!(
            limits_setting_key("codex", Some("work")),
            "provider_limits_codex_work"
        );
    }

    #[test]
    fn limits_rows_are_host_settings_a_client_may_read() {
        use crate::commands::settings::is_host_setting_key;
        assert!(is_host_setting_key("provider_limits_codex_work"));
        assert!(!is_host_setting_key(LIMITS_SETTING_PREFIX));
    }

    #[test]
    fn a_stream_event_turns_its_fraction_into_a_percent() {
        let event = json!({
            "type": "rate_limit_event",
            "rate_limit_info": {
                "status": "allowed",
                "resetsAt": 1_788_265_323,
                "rateLimitType": "five_hour",
                "unifiedWindows": {
                    "five_hour": { "utilization": 0.06, "resetsAt": 1_788_265_323 },
                    "seven_day": { "utilization": 0.02, "resetsAt": 1_788_765_541 },
                },
            },
        });
        let limits = from_stream_event(&event, 1_788_000_000).unwrap();
        assert_eq!(limits.source, LimitSource::Stream);
        assert_eq!(limits.as_of, 1_788_000_000);
        assert!((limits.five_hour.unwrap().percent - 6.0).abs() < 1e-9);
        assert_eq!(limits.five_hour.unwrap().resets_at, Some(1_788_265_323));
        assert!((limits.seven_day.unwrap().percent - 2.0).abs() < 1e-9);
    }

    #[test]
    fn other_stream_events_carry_no_limits() {
        assert_eq!(from_stream_event(&json!({"type": "assistant"}), 1), None);
        assert_eq!(
            from_stream_event(
                &json!({"type": "rate_limit_event", "rate_limit_info": {}}),
                1
            ),
            None
        );
    }

    #[test]
    fn a_reset_in_milliseconds_is_read_as_seconds() {
        let event = json!({
            "type": "rate_limit_event",
            "rate_limit_info": { "unifiedWindows": {
                "five_hour": { "utilization": 0.5, "resetsAt": 1_788_265_323_000_i64 },
            } },
        });
        let limits = from_stream_event(&event, 1).unwrap();
        assert_eq!(limits.five_hour.unwrap().resets_at, Some(1_788_265_323));
        assert_eq!(limits.seven_day, None);
    }

    #[test]
    fn the_oauth_endpoint_reports_percent_and_iso_resets() {
        let body = json!({
            "five_hour": { "utilization": 42.0, "resets_at": "2026-10-07T14:30:00.123456+00:00" },
            "seven_day": { "utilization": 61, "resets_at": "2026-10-10T08:00:00Z" },
            "seven_day_opus": null,
            "extra_usage": { "is_enabled": false },
        });
        let limits = from_oauth_usage(&body, 1_791_000_000).unwrap();
        assert_eq!(limits.source, LimitSource::OauthUsage);
        assert_eq!(limits.five_hour, window(42.0, 1_791_383_400));
        assert_eq!(limits.seven_day, window(61.0, 1_791_619_200));
    }

    #[test]
    fn an_untouched_oauth_window_has_no_reset() {
        let body = json!({ "five_hour": { "utilization": 0, "resets_at": null } });
        let limits = from_oauth_usage(&body, 1).unwrap();
        assert_eq!(
            limits.five_hour,
            Some(LimitWindow {
                percent: 0.0,
                resets_at: None
            })
        );
    }

    #[test]
    fn the_app_server_reports_percent_and_epoch_seconds() {
        let rate_limits = json!({
            "primary": { "usedPercent": 42, "windowDurationMins": 300, "resetsAt": 1_788_265_323 },
            "secondary": { "usedPercent": 61.5, "windowDurationMins": 10080, "resetsAt": 1_788_765_541 },
            "planType": "plus",
        });
        let limits = from_app_server(&rate_limits, 7).unwrap();
        assert_eq!(limits.source, LimitSource::AppServer);
        assert_eq!(limits.as_of, 7);
        assert_eq!(limits.five_hour, window(42.0, 1_788_265_323));
        assert_eq!(limits.seven_day, window(61.5, 1_788_765_541));
    }

    #[test]
    fn windows_are_slotted_by_duration_not_position() {
        let rate_limits = json!({
            "primary": { "usedPercent": 61, "windowDurationMins": 10080, "resetsAt": 2 },
            "secondary": { "usedPercent": 42, "windowDurationMins": 300, "resetsAt": 1 },
        });
        let limits = from_app_server(&rate_limits, 0).unwrap();
        assert_eq!(limits.five_hour, window(42.0, 1));
        assert_eq!(limits.seven_day, window(61.0, 2));
    }

    #[test]
    fn a_rollout_reports_percent_and_epoch_seconds_as_of_its_event() {
        let rate_limits = json!({
            "primary": { "used_percent": 12.5, "window_minutes": 300, "resets_at": 1_788_265_323 },
            "secondary": { "used_percent": 140.0, "window_minutes": 10080, "resets_at": 1_788_765_541 },
        });
        let limits = from_rollout(&rate_limits, 1_788_200_000).unwrap();
        assert_eq!(limits.source, LimitSource::Rollout);
        assert_eq!(limits.as_of, 1_788_200_000);
        assert_eq!(limits.five_hour, window(12.5, 1_788_265_323));
        // Past the limit reads as full, not as 140%.
        assert_eq!(limits.seven_day, window(100.0, 1_788_765_541));
    }

    #[test]
    fn an_older_rollout_counts_its_reset_from_the_event() {
        let rate_limits = json!({
            "primary": { "used_percent": 5, "window_minutes": 300, "resets_in_seconds": 600 },
        });
        let limits = from_rollout(&rate_limits, 1000).unwrap();
        assert_eq!(limits.five_hour, window(5.0, 1600));
    }

    #[test]
    fn a_rollout_without_windows_is_no_reading() {
        assert_eq!(from_rollout(&json!({}), 1), None);
        assert_eq!(from_rollout(&json!(null), 1), None);
    }

    fn state(status: RefreshStatus, at: i64) -> RefreshState {
        RefreshState {
            status,
            at,
            next_allowed_at: None,
            failures: 0,
        }
    }

    #[test]
    fn a_fresh_successful_refresh_holds_the_next_one_back() {
        assert!(refresh_allowed(None, 100));
        let ok = state(RefreshStatus::Ok, 100);
        assert!(!refresh_allowed(Some(&ok), 100 + REFRESH_FLOOR_SECS - 1));
        assert!(refresh_allowed(Some(&ok), 100 + REFRESH_FLOOR_SECS));
    }

    #[test]
    fn signed_out_and_stale_outcomes_allow_an_immediate_retry() {
        assert!(refresh_allowed(
            Some(&state(RefreshStatus::SignedOut, 100)),
            101
        ));
        assert!(refresh_allowed(
            Some(&state(RefreshStatus::Stale, 100)),
            101
        ));
    }

    #[test]
    fn consecutive_429s_double_the_back_off_up_to_the_cap() {
        let first = next_refresh_state(None, &RefreshOutcome::RateLimited, 1000);
        assert_eq!(first.status, RefreshStatus::RateLimited);
        assert_eq!(first.failures, 1);
        assert_eq!(first.next_allowed_at, Some(1000 + BASE_BACKOFF_SECS));
        assert!(!refresh_allowed(Some(&first), 1000 + BASE_BACKOFF_SECS - 1));
        assert!(refresh_allowed(Some(&first), 1000 + BASE_BACKOFF_SECS));

        let second = next_refresh_state(Some(&first), &RefreshOutcome::RateLimited, 2000);
        assert_eq!(second.failures, 2);
        assert_eq!(second.next_allowed_at, Some(2000 + 2 * BASE_BACKOFF_SECS));

        let mut last = second;
        for _ in 0..20 {
            last = next_refresh_state(Some(&last), &RefreshOutcome::RateLimited, 3000);
        }
        assert_eq!(last.next_allowed_at, Some(3000 + MAX_BACKOFF_SECS));
    }

    #[test]
    fn any_answer_after_a_429_resets_the_back_off() {
        let limited = next_refresh_state(None, &RefreshOutcome::RateLimited, 1000);
        let stale = next_refresh_state(Some(&limited), &RefreshOutcome::Stale, 2000);
        assert_eq!(stale, state(RefreshStatus::Stale, 2000));
        let again = next_refresh_state(Some(&stale), &RefreshOutcome::RateLimited, 3000);
        assert_eq!(again.failures, 1);
    }

    #[test]
    fn a_row_round_trips_through_its_json() {
        let row = AccountLimits {
            limits: Some(ProviderLimits {
                five_hour: window(42.0, 10),
                seven_day: None,
                as_of: 5,
                source: LimitSource::AppServer,
            }),
            refresh: Some(state(RefreshStatus::Ok, 5)),
        };
        let raw = serde_json::to_string(&row).unwrap();
        assert!(raw.contains(r#""source":"app_server""#), "{raw}");
        assert_eq!(serde_json::from_str::<AccountLimits>(&raw).unwrap(), row);
    }

    fn reading(as_of: i64, percent: f64) -> ProviderLimits {
        ProviderLimits {
            five_hour: window(percent, 99),
            seven_day: None,
            as_of,
            source: LimitSource::Rollout,
        }
    }

    #[test]
    fn a_passive_reading_is_recorded_only_when_newer_and_announced() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        assert!(record_limits(&ctx, "codex", Some("work"), reading(10, 1.0)).unwrap());
        assert!(!record_limits(&ctx, "codex", Some("work"), reading(10, 2.0)).unwrap());
        assert!(!record_limits(&ctx, "codex", Some("work"), reading(9, 3.0)).unwrap());
        assert!(record_limits(&ctx, "codex", Some("work"), reading(11, 4.0)).unwrap());

        let row = load(&ctx.db.lock(), "codex", Some("work"));
        assert_eq!(row.limits, Some(reading(11, 4.0)));
        let announced: Vec<_> = sink
            .events()
            .into_iter()
            .filter(|(name, p)| {
                name == crate::commands::settings::SETTINGS_CHANGED
                    && p["key"] == "provider_limits_codex_work"
            })
            .collect();
        assert_eq!(announced.len(), 2);
    }

    #[test]
    fn a_refresh_without_a_reading_keeps_the_last_known_one() {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        record_limits(&ctx, "claude", None, reading(10, 1.0)).unwrap();
        let row = record_refresh(&ctx, "claude", None, RefreshOutcome::RateLimited, 20).unwrap();
        assert_eq!(row.limits, Some(reading(10, 1.0)));
        assert_eq!(
            row.refresh.as_ref().map(|r| r.status),
            Some(RefreshStatus::RateLimited)
        );
        // Persisted: a restart keeps the back-off.
        let stored = load(&ctx.db.lock(), "claude", Some(accounts::DEFAULT_ACCOUNT));
        assert_eq!(stored, row);
    }

    #[test]
    fn a_stream_event_is_recorded_under_the_streams_account() {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let event = json!({
            "type": "rate_limit_event",
            "rate_limit_info": { "unifiedWindows": {
                "five_hour": { "utilization": 0.25, "resetsAt": 1_788_265_323 },
            } },
        });
        observe_stream_event(&ctx, Some("work"), &event);
        observe_stream_event(&ctx, Some("work"), &json!({ "type": "assistant" }));

        let row = load(&ctx.db.lock(), "claude", Some("work"));
        let limits = row.limits.expect("recorded");
        assert_eq!(limits.source, LimitSource::Stream);
        assert_eq!(limits.five_hour, window(25.0, 1_788_265_323));
        assert_eq!(
            load(&ctx.db.lock(), "claude", None),
            AccountLimits::default(),
            "the default account is untouched"
        );
    }

    #[test]
    fn an_unreadable_row_reads_as_empty() {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        crate::database::set_setting(&ctx.db.lock(), "provider_limits_claude_default", "{nope")
            .unwrap();
        assert_eq!(
            load(&ctx.db.lock(), "claude", None),
            AccountLimits::default()
        );
    }
}
