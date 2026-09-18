//! Claude Code transcript lines → [`UsageRecord`].
//!
//! Mirrors `src/adapters/claude/usage.ts`, which is the canonical statement of
//! these rules; keep the two in step (the shared corpus under
//! `tests/fixtures/usage` fails when they drift).
//!
//! An `assistant` record carries its call's usage on `message.usage`, and
//! `input_tokens` is already the *fresh* input — Anthropic excludes cache
//! reads/writes from it — so the four counts are taken as reported. Three
//! record shapes are not billed calls of their own and are dropped here:
//! `isApiErrorMessage` envelopes (they replay a failed call's usage), the
//! `<synthetic>` model (the CLI talking to itself), and anything with no tokens
//! at all. A fourth shape — the same `message.id` written once per content
//! block, per streaming rewrite, and copied verbatim into a resumed session's
//! transcript — is one call spread over many lines, and is collapsed at
//! aggregation time by [`UsageRecord::dedupe_key`].
//!
//! Not every record names its model: Claude omits `message.model` on some lines
//! (a post-compaction continuation, for one). The TS fold attributes such a call
//! to the last model the session named, so the file is parsed against a
//! [`ClaudeState`] carrying that model forward — cached between scans, like
//! Codex's, so appended lines resume it instead of starting blank.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{num, parse_ts_ms, UsageRecord};
use crate::usage_scan::{Provider, TokenCounts};

/// Placeholder model on records the CLI generates itself (interrupt notices,
/// error text). Not a billed call.
const SYNTHETIC_MODEL: &str = "<synthetic>";

/// What one transcript's parse carries between lines: the model in force, for
/// the records that name none of their own (the TS fold's `currentModel`).
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ClaudeState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_model: Option<String>,
}

pub(crate) fn parse_claude_line(line: &str, state: &mut ClaudeState) -> Option<UsageRecord> {
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
    let model = message.get("model").and_then(Value::as_str);
    if model == Some(SYNTHETIC_MODEL) {
        return None;
    }
    let usage = message.get("usage").filter(|u| u.is_object())?;
    let ts_ms = parse_ts_ms(&v)?;

    let tokens = TokenCounts {
        input: num(usage, "input_tokens"),
        output: num(usage, "output_tokens"),
        cache_read: num(usage, "cache_read_input_tokens"),
        cache_write: cache_creation_tokens(usage),
    };
    // A zero-token record has no usage; it isn't a free call. Dropping it here
    // rather than at aggregation also keeps it from claiming a dedupe key that
    // a later, real record of the same message would then lose.
    if tokens.total() == 0 {
        return None;
    }

    // Everything above this point produces no event in the TS fold, so it must
    // not move `currentModel` either — only a record that counts as spend does.
    // A subagent's model is its own: it is attributed to the sidechain record
    // that named it, but never carried forward into the main conversation.
    let sidechain = v.get("isSidechain").and_then(Value::as_bool) == Some(true);
    if !sidechain {
        if let Some(named) = model.filter(|m| !m.is_empty()) {
            state.last_model = Some(named.to_string());
        }
    }

    Some(UsageRecord {
        ts_ms,
        provider: Provider::Claude,
        // `event.model ?? currentModel` in the TS fold: a record that states a
        // model uses it — a *stated* empty one included — and only a record
        // with no `model` field at all falls back to the carried one. With
        // nothing to carry it is still spend, bucketed under "".
        model: model
            .or(state.last_model.as_deref())
            .unwrap_or_default()
            .to_string(),
        session_id: v
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
        tokens,
        dedupe_key: dedupe_key(&v, message),
    })
}

/// Identity of the underlying API call, for the global dedupe. `message.id`
/// alone would merge a *retry* of the same message, so the request id joins it.
/// `None` without a message id: an unidentifiable record must stand alone
/// rather than collide with every other unidentifiable one — which is why the
/// request id never carries the key by itself.
fn dedupe_key(v: &Value, message: &Value) -> Option<String> {
    let msg_id = message.get("id").and_then(Value::as_str).unwrap_or("");
    if msg_id.is_empty() {
        return None;
    }
    let req_id = v.get("requestId").and_then(Value::as_str).unwrap_or("");
    Some(format!("{msg_id}:{req_id}"))
}

/// Cache writes. Newer Claude builds break them out by TTL under
/// `cache_creation` while keeping the flat `cache_creation_input_tokens`
/// beside it; the breakdown is authoritative when present.
fn cache_creation_tokens(usage: &Value) -> u64 {
    let split = usage
        .get("cache_creation")
        .filter(|c| c.is_object())
        .map_or(0, |c| {
            num(c, "ephemeral_5m_input_tokens") + num(c, "ephemeral_1h_input_tokens")
        });
    if split > 0 {
        split
    } else {
        num(usage, "cache_creation_input_tokens")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(message: Value) -> String {
        serde_json::json!({
            "type": "assistant",
            "sessionId": "s1",
            "requestId": "req_1",
            "timestamp": "2026-01-02T10:00:00Z",
            "message": message,
        })
        .to_string()
    }

    /// Parse one line against a throwaway state, for the rules that don't
    /// involve carry-forward.
    fn parse(line: &str) -> Option<UsageRecord> {
        parse_claude_line(line, &mut ClaudeState::default())
    }

    #[test]
    fn the_ttl_breakdown_wins_over_the_flat_cache_creation_count() {
        let record = parse(&line(serde_json::json!({
            "id": "msg_1",
            "model": "claude-opus-5",
            "usage": {
                "input_tokens": 1,
                "cache_creation_input_tokens": 9,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": 2000,
                    "ephemeral_1h_input_tokens": 500,
                },
            },
        })))
        .expect("a record");
        assert_eq!(record.tokens.cache_write, 2500);
    }

    #[test]
    fn an_absent_or_empty_breakdown_falls_back_to_the_flat_count() {
        for breakdown in [
            serde_json::Value::Null,
            serde_json::json!({ "ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 0 }),
        ] {
            let record = parse(&line(serde_json::json!({
                "id": "msg_1",
                "model": "claude-opus-5",
                "usage": {
                    "input_tokens": 1,
                    "cache_creation_input_tokens": 77,
                    "cache_creation": breakdown,
                },
            })))
            .expect("a record");
            assert_eq!(record.tokens.cache_write, 77);
        }
    }

    #[test]
    fn a_record_with_no_model_is_kept_and_buckets_under_the_empty_model() {
        let record = parse(&line(serde_json::json!({
            "id": "msg_1",
            "usage": { "input_tokens": 7, "output_tokens": 40 },
        })))
        .expect("a modelless call is still spend");
        assert_eq!(record.model, "");
        assert_eq!(record.tokens.input, 7);
    }

    #[test]
    fn the_synthetic_model_is_not_a_billed_call() {
        assert!(parse(&line(serde_json::json!({
            "id": "msg_1",
            "model": "<synthetic>",
            // Tokens, so it is the model that drops it, not the zero guard.
            "usage": { "input_tokens": 5, "output_tokens": 5 },
        })))
        .is_none());
    }

    #[test]
    fn records_without_a_message_id_are_never_deduped_against_each_other() {
        // Two lines sharing only a `requestId`: each stands alone, so a key
        // built from the request id alone would silently merge them.
        let one = parse(&line(serde_json::json!({
            "model": "claude-opus-5",
            "usage": { "input_tokens": 2, "output_tokens": 11 },
        })))
        .expect("a record");
        assert_eq!(one.dedupe_key, None);
    }

    #[test]
    fn a_zero_token_record_is_not_a_call() {
        assert!(parse(&line(serde_json::json!({
            "id": "msg_1",
            "model": "claude-opus-5",
            "usage": { "input_tokens": 0, "output_tokens": 0 },
        })))
        .is_none());
    }

    // ── the model carried between lines ─────────────────────────────────────

    #[test]
    fn a_stated_empty_model_is_used_as_stated_rather_than_carried_over() {
        // `event.model ?? currentModel`: `??` doesn't fall through an empty
        // string, so a line that states `"model": ""` buckets under "" even
        // with a model to carry — and doesn't become the carried one either.
        let mut state = ClaudeState::default();
        parse_claude_line(
            &line(serde_json::json!({
                "id": "msg_1",
                "model": "claude-opus-5",
                "usage": { "input_tokens": 5, "output_tokens": 5 },
            })),
            &mut state,
        );
        let record = parse_claude_line(
            &line(serde_json::json!({
                "id": "msg_2",
                "model": "",
                "usage": { "input_tokens": 1, "output_tokens": 1 },
            })),
            &mut state,
        )
        .expect("a record");
        assert_eq!(record.model, "");
        assert_eq!(state.last_model.as_deref(), Some("claude-opus-5"));
    }

    #[test]
    fn a_record_that_is_not_spend_never_becomes_the_carried_model() {
        // None of these produce an event in the TS fold, so none of them may
        // move `currentModel` here.
        let mut state = ClaudeState::default();
        for message in [
            serde_json::json!({
                "id": "m0", "model": "<synthetic>",
                "usage": { "input_tokens": 5, "output_tokens": 5 },
            }),
            serde_json::json!({
                "id": "m1", "model": "claude-haiku-4",
                "usage": { "input_tokens": 0, "output_tokens": 0 },
            }),
        ] {
            assert!(parse_claude_line(&line(message), &mut state).is_none());
        }
        assert_eq!(state.last_model, None);

        let mut error: Value = serde_json::from_str(&line(serde_json::json!({
            "id": "m2", "model": "claude-haiku-4",
            "usage": { "input_tokens": 5, "output_tokens": 5 },
        })))
        .unwrap();
        error["isApiErrorMessage"] = Value::Bool(true);
        assert!(parse_claude_line(&error.to_string(), &mut state).is_none());
        assert_eq!(state.last_model, None);
    }
}
