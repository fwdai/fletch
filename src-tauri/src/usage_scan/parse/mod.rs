//! Line parsers for the two transcript formats, plus the small helpers they
//! share. Everything here is window-independent: a line becomes a
//! [`UsageRecord`] or nothing at all, and the caller decides later which
//! records a given window wants.
//!
//! One submodule per provider ([`claude`], [`codex`]) because the two formats
//! have nothing in common and their rules drift independently — these parsers
//! are the Rust half of a pair, the other being `src/adapters/{claude,codex}/
//! usage.ts`, and the two are pinned to the same answers by the shared corpus
//! under `tests/fixtures/usage` (see `corpus_totals_match_the_shared_fixture`).

mod claude;
mod codex;

pub(crate) use claude::{parse_claude_line, ClaudeState};
pub(crate) use codex::{parse_codex_line, CodexState};

use chrono::{Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Provider, TokenCounts};

/// One parsed usage record, before windowing/dedupe/bucketing. Deliberately
/// window-independent so a cached record answers any later window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UsageRecord {
    pub(crate) ts_ms: i64,
    pub(crate) provider: Provider,
    /// Empty when the transcript never named one — the TS adapters keep such a
    /// call and key it under `""`, so this does too rather than dropping spend
    /// that really happened.
    pub(crate) model: String,
    /// `None` for Codex, whose session id belongs to the file (see
    /// [`FileEntry::codex_session_id`](super::cache::FileEntry::codex_session_id)),
    /// and for Claude records with no `sessionId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_id: Option<String>,
    pub(crate) tokens: TokenCounts,
    /// `{message.id}:{requestId}` for Claude, deduped globally at aggregation
    /// time. `None` when the record has no `message.id` (an unidentifiable
    /// record stands alone rather than colliding with every other one), or for
    /// Codex, whose duplicate suppression is positional and happens while
    /// parsing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dedupe_key: Option<String>,
}

// ── parsing helpers ─────────────────────────────────────────────────────────

fn parse_ts_ms(v: &Value) -> Option<i64> {
    let s = v.get("timestamp")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

/// Epoch ms of the *local* hour containing an instant. Bucket boundaries sit on
/// the user's wall-clock hour, so summing whole hours into a day reproduces
/// their midnight-to-midnight "today". Falls back to the UTC hour if the local
/// time is ambiguous or nonexistent (the one DST-transition hour per year).
pub(crate) fn local_hour_start_ms(ms: i64) -> i64 {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .and_then(|d| {
            d.with_minute(0)
                .and_then(|d| d.with_second(0))
                .and_then(|d| d.with_nanosecond(0))
        })
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(|| ms - ms.rem_euclid(3_600_000))
}

/// A token field, clamped at 0 — absent, null, non-numeric and negative all
/// mean "nothing to count".
fn num(v: &Value, key: &str) -> u64 {
    v.get(key)
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0)
        .try_into()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_scan::test_support::ms;

    #[test]
    fn hour_starts_are_aligned_to_the_local_wall_clock() {
        // Whatever the local offset, the bucket start must be an exact hour
        // boundary in local time — i.e. zero minutes/seconds/millis.
        let t = ms("2026-01-02T10:37:42.123Z");
        let start = local_hour_start_ms(t);
        assert!(start <= t && t - start < 3_600_000);
        let local = Local.timestamp_millis_opt(start).single().unwrap();
        assert_eq!(
            (local.minute(), local.second(), local.nanosecond()),
            (0, 0, 0)
        );
    }
}
