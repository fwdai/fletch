//! Line parsers for the two transcript formats, plus the small helpers they
//! share. Everything here is window-independent: a line becomes a
//! [`UsageRecord`] or nothing at all, and the caller decides later which
//! records a given window wants.

use chrono::{Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Provider, TokenCounts};

/// One parsed usage record, before windowing/dedupe/bucketing. Deliberately
/// window-independent so a cached record answers any later window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct UsageRecord {
    pub(super) ts_ms: i64,
    pub(super) provider: Provider,
    pub(super) model: String,
    /// `None` for Codex, whose session id belongs to the file (see
    /// [`FileEntry::codex_session_id`](super::cache::FileEntry::codex_session_id)),
    /// and for Claude records with no `sessionId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) session_id: Option<String>,
    pub(super) tokens: TokenCounts,
    /// `{message.id}:{requestId}` for Claude, deduped globally at aggregation
    /// time. `None` when neither id is present, or for Codex (whose duplicate
    /// suppression is positional and happens while parsing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) dedupe_key: Option<String>,
}

// ── claude ──────────────────────────────────────────────────────────────────

pub(super) fn parse_claude_line(line: &str) -> Option<UsageRecord> {
    // Cheap gate: only assistant records carry a `usage` object, and they are a
    // small minority of lines in a transcript.
    if !line.contains("\"usage\"") {
        return None;
    }
    let v = serde_json::from_str::<Value>(line).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    if v.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let message = v.get("message")?;
    let usage = message.get("usage").filter(|u| u.is_object())?;
    let model = message
        .get("model")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())?;
    let ts_ms = parse_ts_ms(&v)?;

    // Claude emits the *same* usage object once per content block of a reply,
    // and copies prior records verbatim into the transcript of a resumed or
    // forked session — so the dedupe key has to be global, not per-file. Either
    // half of the key may be absent on odd records; only dedupe when at least
    // one is present.
    let msg_id = message.get("id").and_then(Value::as_str).unwrap_or("");
    let req_id = v.get("requestId").and_then(Value::as_str).unwrap_or("");
    let dedupe_key =
        (!(msg_id.is_empty() && req_id.is_empty())).then(|| format!("{msg_id}:{req_id}"));

    Some(UsageRecord {
        ts_ms,
        provider: Provider::Claude,
        model: model.to_string(),
        session_id: v
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
        tokens: TokenCounts {
            input: num(usage, "input_tokens"),
            output: num(usage, "output_tokens"),
            cache_read: num(usage, "cache_read_input_tokens"),
            cache_write: num(usage, "cache_creation_input_tokens"),
        },
        dedupe_key,
    })
}

// ── codex ───────────────────────────────────────────────────────────────────

/// Codex rollouts are a stream, not self-describing records: the model lives in
/// a `turn_context` line that applies to every later `token_count`, so the file
/// has to be read as a small state machine. Cached between scans so appended
/// lines parse exactly as they would have in one pass over the whole file.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct CodexState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) session_id: Option<String>,
    pub(super) saw_session_meta: bool,
    /// Timestamp anchoring the burst of records a fork copied from its parent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fork_anchor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) prev_usage: Option<String>,
}

pub(super) fn parse_codex_line(line: &str, state: &mut CodexState, out: &mut Vec<UsageRecord>) {
    if !(line.contains("token_count")
        || line.contains("turn_context")
        || line.contains("session_meta"))
    {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    let payload = v.get("payload");

    if kind == "session_meta" && !state.saw_session_meta {
        state.saw_session_meta = true;
        if let Some(p) = payload {
            state.session_id = p
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| p.get("session_id").and_then(Value::as_str))
                .map(str::to_string);
            // A fork (explicit `codex resume --fork`, or a subagent thread
            // spawned off a parent) starts its rollout with a verbatim copy
            // of the parent's history, token_count events included. Those
            // are replays, not new spend: anchor here and drop the burst.
            let forked = p.get("forked_from_id").and_then(Value::as_str).is_some()
                || p.pointer("/source/subagent/thread_spawn/parent_thread_id")
                    .and_then(Value::as_str)
                    .is_some();
            if forked {
                state.fork_anchor = parse_ts_ms(&v);
            }
        }
        return;
    }

    if kind == "turn_context" {
        if let Some(m) = payload
            .and_then(|p| p.get("model"))
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
        {
            state.model = Some(m.to_string());
        }
        return;
    }

    let Some(payload) = payload else { return };
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return;
    }
    let Some(usage) = payload
        .pointer("/info/last_token_usage")
        .filter(|u| u.is_object())
    else {
        return;
    };
    let Some(model) = state.model.as_deref() else {
        return;
    };
    let Some(ts_ms) = parse_ts_ms(&v) else {
        return;
    };

    // Codex re-emits the previous turn's `last_token_usage` unchanged on
    // some events; only a *changed* payload is a new request.
    let usage_json = usage.to_string();
    if state.prev_usage.as_deref() == Some(usage_json.as_str()) {
        return;
    }
    state.prev_usage = Some(usage_json);

    if let Some(anchor) = state.fork_anchor {
        // The copied burst is written back-to-back at fork time; a real
        // turn lands at least a second later. Walk the anchor forward
        // through the burst, then stop suppressing for good.
        if ts_ms - anchor < 1000 {
            state.fork_anchor = Some(ts_ms);
            return;
        }
        state.fork_anchor = None;
    }

    let input = num(usage, "input_tokens");
    let cached = num(usage, "cached_input_tokens");
    let cache_write = num(usage, "cache_write_input_tokens");
    out.push(UsageRecord {
        ts_ms,
        provider: Provider::Codex,
        model: model.to_string(),
        // The id belongs to the file, not the record: it may be learned from a
        // `session_meta` line and falls back to the path.
        session_id: None,
        tokens: TokenCounts {
            // Codex reports total input including the cached/written parts;
            // the fresh remainder is what a non-cached input rate applies to.
            input: input.saturating_sub(cached).saturating_sub(cache_write),
            output: num(usage, "output_tokens"),
            cache_read: cached,
            cache_write,
        },
        dedupe_key: None,
    });
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
pub(super) fn local_hour_start_ms(ms: i64) -> i64 {
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
