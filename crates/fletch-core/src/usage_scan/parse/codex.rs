//! Codex CLI rollout lines → [`UsageRecord`]s.
//!
//! Mirrors `src/adapters/codex/usage.ts` for what a turn's counts *mean*;
//! keep the two in step (the shared corpus under `tests/fixtures/usage` fails
//! when they drift).
//!
//! Rollouts are a stream, not self-describing records: the model lives in a
//! `turn_context` line that applies to every later `token_count`, and the
//! session id in a leading `session_meta`, so a file has to be read as a small
//! state machine. [`CodexState`] is cached between scans so appended lines
//! parse exactly as they would have in one pass over the whole file.
//!
//! The counter itself is `info.last_token_usage`, the turn's own contribution.
//! `input_tokens` INCLUDES the cached part, so fresh input is the difference —
//! cache *writes* are their own field and are never subtracted out (the TS
//! adapter's `tokenCounts`). Codex re-emits an unchanged reading on some
//! events, and a forked rollout opens with a verbatim copy of its parent's
//! history; both are handled here rather than by the aggregation, because both
//! are positional facts about the stream.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{num, parse_ts_ms, UsageRecord};
use crate::usage_scan::{Provider, TokenCounts};

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct CodexState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session_id: Option<String>,
    pub(crate) saw_session_meta: bool,
    /// Timestamp anchoring the burst of records a fork copied from its parent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fork_anchor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) prev_usage: Option<String>,
}

pub(crate) fn parse_codex_line(line: &str, state: &mut CodexState, out: &mut Vec<UsageRecord>) {
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
    // `info` is null on the event codex emits before a turn has run.
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

    let cached = num(usage, "cached_input_tokens");
    out.push(UsageRecord {
        ts_ms,
        provider: Provider::Codex,
        model: model.to_string(),
        // The id belongs to the file, not the record: it may be learned from a
        // `session_meta` line and falls back to the path.
        session_id: None,
        tokens: TokenCounts {
            // `input_tokens` counts the cached prefix too; the fresh remainder
            // is what a non-cached input rate applies to. Cache writes are
            // reported alongside, not carved out of, that remainder.
            input: num(usage, "input_tokens").saturating_sub(cached),
            output: num(usage, "output_tokens"),
            cache_read: cached,
            cache_write: num(usage, "cache_write_input_tokens"),
        },
        dedupe_key: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_input_subtracts_the_cached_prefix_but_not_the_cache_writes() {
        let mut state = CodexState {
            model: Some("gpt-5.5".into()),
            ..CodexState::default()
        };
        let mut out = Vec::new();
        parse_codex_line(
            &serde_json::json!({
                "timestamp": "2026-01-02T10:00:00Z",
                "type": "event_msg",
                "payload": { "type": "token_count", "info": { "last_token_usage": {
                    "input_tokens": 31000,
                    "cached_input_tokens": 27000,
                    "cache_write_input_tokens": 900,
                    "output_tokens": 1200,
                } } },
            })
            .to_string(),
            &mut state,
            &mut out,
        );
        assert_eq!(
            out[0].tokens,
            TokenCounts {
                input: 4000,
                output: 1200,
                cache_read: 27000,
                cache_write: 900,
            }
        );
    }
}
