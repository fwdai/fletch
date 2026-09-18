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
//! block, and copied verbatim into a resumed session's transcript — is one call
//! spread over many lines, and is collapsed at aggregation time by
//! [`UsageRecord::dedupe_key`].

use serde_json::Value;

use super::{num, parse_ts_ms, UsageRecord};
use crate::usage_scan::{Provider, TokenCounts};

/// Placeholder model on records the CLI generates itself (interrupt notices,
/// error text). Not a billed call.
const SYNTHETIC_MODEL: &str = "<synthetic>";

pub(crate) fn parse_claude_line(line: &str) -> Option<UsageRecord> {
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

    Some(UsageRecord {
        ts_ms,
        provider: Provider::Claude,
        // A record with no `model` is still spend; it buckets under "".
        model: model.unwrap_or_default().to_string(),
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

    #[test]
    fn the_ttl_breakdown_wins_over_the_flat_cache_creation_count() {
        let record = parse_claude_line(&line(serde_json::json!({
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
            let record = parse_claude_line(&line(serde_json::json!({
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
        let record = parse_claude_line(&line(serde_json::json!({
            "id": "msg_1",
            "usage": { "input_tokens": 7, "output_tokens": 40 },
        })))
        .expect("a modelless call is still spend");
        assert_eq!(record.model, "");
        assert_eq!(record.tokens.input, 7);
    }

    #[test]
    fn the_synthetic_model_is_not_a_billed_call() {
        assert!(parse_claude_line(&line(serde_json::json!({
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
        let one = parse_claude_line(&line(serde_json::json!({
            "model": "claude-opus-5",
            "usage": { "input_tokens": 2, "output_tokens": 11 },
        })))
        .expect("a record");
        assert_eq!(one.dedupe_key, None);
    }

    #[test]
    fn a_zero_token_record_is_not_a_call() {
        assert!(parse_claude_line(&line(serde_json::json!({
            "id": "msg_1",
            "model": "claude-opus-5",
            "usage": { "input_tokens": 0, "output_tokens": 0 },
        })))
        .is_none());
    }
}
