//! What a failed turn's error means, as categories safe to send to telemetry.
//!
//! An agent's `last_error` is free text — a CLI's stderr, a spawn failure, a
//! sandbox launcher's copy — and may carry paths or prompt fragments, so it
//! never leaves the machine. What the `turn_completed` event carries instead is
//! the *kind* of failure ([`classify`]) and the process's exit code
//! ([`exit_code`]), which together say whether users are hitting rate limits,
//! sign-in trouble, hangs or genuine crashes, per provider and version.
//!
//! Matching is substring-based and case-insensitive, ordered so the most
//! specific signal wins: a launch that never ran is a spawn failure whatever
//! else its message says, and a provider's "Rate limit exceeded" on exit 1 is a
//! rate limit, not a crash.

use serde_json::{json, Value};

use crate::workspace::{ClosedTurn, TurnOutcome};

/// The categorical kind of a turn's error. Every value is a fixed label; the
/// message itself is never part of the answer.
pub fn classify(message: &str) -> &'static str {
    let m = message.trim().to_ascii_lowercase();
    if m.is_empty() {
        return "unknown";
    }
    let any = |needles: &[&str]| needles.iter().any(|n| m.contains(n));
    let code = exit_code(&m);

    // The process never ran (or the sandbox launcher could not start it).
    if any(&[
        "agent spawn:",
        "not found on path",
        "no such file or directory",
        "has no support for provider",
        "could not start the sandbox",
    ]) || matches!(code, Some(125..=127))
    {
        return "spawn_failed";
    }
    if any(&[
        "rate limit",
        "ratelimit",
        "quota",
        "usage limit",
        "resource_exhausted",
        "freeusagelimit",
        "too many requests",
    ]) || has_code(&m, "429")
    {
        return "rate_limit";
    }
    if any(&[
        "unauthorized",
        "forbidden",
        "not logged in",
        "not signed in",
        "sign in",
        "login expired",
        "login refresh",
        "revoked",
        "credential",
        "api key",
        "api_key",
        "authentication",
        "eligibility check failed",
    ]) || has_code(&m, "401")
        || has_code(&m, "403")
    {
        return "auth";
    }
    if any(&[
        "model not found",
        "unknown model",
        "no such model",
        "invalid model",
        "model_not_found",
        "providermodelnotfound",
    ]) {
        return "unknown_model";
    }
    if any(&["timeout", "timed out", "deadline"]) {
        return "timeout";
    }
    if any(&[
        "operation not permitted",
        "permission denied",
        "eperm",
        "eacces",
        "sandbox",
    ]) {
        return "sandbox_denied";
    }
    if any(&[
        "econnrefused",
        "enotfound",
        "etimedout",
        "econnreset",
        "fetch failed",
        "connection refused",
        "connection reset",
        "socket hang up",
        "network",
        "dns",
    ]) {
        return "network";
    }
    if any(&["signal:", "sigkill", "sigterm", "killed"]) {
        return "killed";
    }
    "crash"
}

/// The exit code named in an exit message, when one is: `exit status: 1`
/// (std's `ExitStatus`), `exit code: 2`, or a sandbox launcher's `Exit 125: …`.
pub fn exit_code(message: &str) -> Option<i32> {
    let m = message.to_ascii_lowercase();
    ["exit status: ", "exit code: ", "exit code ", "exit "]
        .iter()
        .find_map(|prefix| {
            m.match_indices(prefix)
                .find_map(|(at, _)| leading_int(&m[at + prefix.len()..]))
        })
}

fn leading_int(s: &str) -> Option<i32> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Whether `code` appears as its own token — `429` in "HTTP 429 (rate limit)"
/// but not in an id like `msg_1429`.
fn has_code(m: &str, code: &str) -> bool {
    m.match_indices(code).any(|(at, _)| {
        let before = m[..at].chars().next_back();
        let after = m[at + code.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

/// A per-turn process's exit message with the failure the provider reported in
/// band before exiting, when it reported one the exit message doesn't already
/// carry. The exit status alone ("exit status: 1") says nothing about why;
/// the report ("UnknownError: …", "Rate limit exceeded") is what the user
/// reads and what [`classify`] keys on.
pub(super) fn exit_message_with_report(exit: &str, reported: Option<&str>) -> String {
    match reported.map(str::trim).filter(|r| !r.is_empty()) {
        Some(report) if !exit.contains(report) => format!("{exit}: {report}"),
        _ => exit.to_string(),
    }
}

/// The `turn_completed` event's properties. All categorical or numeric: the
/// provider, its probed version, the model, how long the turn ran and how much
/// it produced, how it ended, and — for a failed turn — the error's kind and
/// exit code. A turn failed whether the process crashed or the provider
/// reported the failure in band and exited cleanly; `error` is whichever
/// message there is. `errored` stays alongside `outcome` for existing
/// dashboards and means exactly `outcome == "failed"`.
pub(super) fn turn_completed_props(
    provider: &str,
    provider_version: Option<&str>,
    model: Option<&str>,
    outcome: TurnOutcome,
    turn: &ClosedTurn,
    error: Option<&str>,
) -> Value {
    let errored = matches!(outcome, TurnOutcome::Failed);
    let error = error.filter(|_| errored);
    json!({
        "provider": provider,
        "provider_version": provider_version,
        "model": model,
        "duration_ms": turn.duration_ms,
        "record_count": turn.record_count,
        "errored": errored,
        "outcome": outcome.as_str(),
        "error_kind": error.map(classify),
        "exit_code": error.and_then(exit_code),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_is_read_from_a_real_message() {
        let cases: &[(&str, &str)] = &[
            // opencode 1.18: a `{"type":"error"}` event on stdout, nothing on
            // stderr, exit 1 — all the supervisor sees is the status.
            ("Agent process exited: exit status: 1", "crash"),
            ("Agent process exited: exit status: 1: Error: Rate limit exceeded. Please try again later.", "rate_limit"),
            ("exit status: 1: FreeUsageLimitError: free usage limit reached", "rate_limit"),
            ("exit status: 1: 429 RESOURCE_EXHAUSTED: check quota", "rate_limit"),
            ("exit status: 1: Monthly usage limit reached", "rate_limit"),
            ("exit status: 1: Eligibility check failed", "auth"),
            ("login expired", "auth"),
            ("exit status: 1: HTTP 401 Unauthorized", "auth"),
            ("exit status: 1: ProviderModelNotFoundError: no such model", "unknown_model"),
            // agy's own print-mode deadline.
            ("Agent process exited: exit status: 1: Error: timeout waiting for response", "timeout"),
            ("exit status: 1: bash: Operation not permitted", "sandbox_denied"),
            ("exit status: 1: TypeError: fetch failed (ECONNREFUSED)", "network"),
            ("agent spawn: No such file or directory (os error 2)", "spawn_failed"),
            ("installer finished but the binary was not found on PATH", "spawn_failed"),
            ("Exit 127: no agent binary on the sandbox image's PATH (or the agent itself exited 127).", "spawn_failed"),
            ("Agent process exited: signal: 9 (SIGKILL)", "killed"),
            ("Agent process exited: exit status: 139", "crash"),
            ("", "unknown"),
        ];
        for (message, kind) in cases {
            assert_eq!(classify(message), *kind, "{message:?}");
        }
    }

    #[test]
    fn a_status_code_only_counts_as_its_own_token() {
        assert_eq!(classify("exit status: 1: request msg_1429 failed"), "crash");
        assert_eq!(
            classify("exit status: 1: server answered 429"),
            "rate_limit"
        );
    }

    #[test]
    fn the_exit_code_is_read_from_every_exit_message_shape() {
        assert_eq!(
            exit_code("Agent process exited: exit status: 1: stderr"),
            Some(1)
        );
        assert_eq!(exit_code("exit code: 2"), Some(2));
        assert_eq!(
            exit_code("Exit 125: Docker could not start the sandbox container"),
            Some(125)
        );
        assert_eq!(exit_code("Agent process exited: signal: 9 (SIGKILL)"), None);
        assert_eq!(exit_code("login expired"), None);
    }

    /// opencode's exit-1-with-empty-stderr: the `error` event it emitted first
    /// is what turns a bare exit status into a classifiable failure.
    #[test]
    fn a_reported_failure_joins_the_exit_message_once() {
        let joined = exit_message_with_report(
            "exit status: 1",
            Some("UnknownError: Rate limit exceeded. Please try again later."),
        );
        assert_eq!(
            joined,
            "exit status: 1: UnknownError: Rate limit exceeded. Please try again later."
        );
        assert_eq!(classify(&joined), "rate_limit");
        assert_eq!(exit_code(&joined), Some(1));
        // Already in the stderr the exit carried: not repeated.
        assert_eq!(
            exit_message_with_report("exit status: 1: boom", Some("boom")),
            "exit status: 1: boom"
        );
        assert_eq!(
            exit_message_with_report("exit status: 1", None),
            "exit status: 1"
        );
        assert_eq!(
            exit_message_with_report("exit status: 1", Some("  ")),
            "exit status: 1"
        );
    }

    #[test]
    fn a_failed_turn_carries_its_kind_and_code_and_a_clean_one_does_not() {
        let turn = ClosedTurn {
            duration_ms: 1200,
            record_count: 3,
        };
        let failed = turn_completed_props(
            "opencode",
            Some("1.18.31"),
            None,
            TurnOutcome::Failed,
            &turn,
            Some("Agent process exited: exit status: 1: Error: Rate limit exceeded"),
        );
        assert_eq!(failed["errored"], true);
        assert_eq!(failed["outcome"], "failed");
        assert_eq!(failed["error_kind"], "rate_limit");
        assert_eq!(failed["exit_code"], 1);
        assert_eq!(failed["provider_version"], "1.18.31");
        // The raw message is never a property.
        assert!(!failed.to_string().contains("Rate limit exceeded"));

        // codex's `turn.failed` on a spent quota: the turn lands on Idle and
        // the process exit is expected, so the only error is the in-band one.
        let in_band = turn_completed_props(
            "codex",
            None,
            Some("gpt-5"),
            TurnOutcome::Failed,
            &turn,
            Some("You've hit your usage limit. Try again at 3pm."),
        );
        assert_eq!(in_band["errored"], true);
        assert_eq!(in_band["error_kind"], "rate_limit");
        assert_eq!(in_band["exit_code"], Value::Null);

        // A user Stop on a hung turn: not an error, but visibly interrupted.
        let stopped = turn_completed_props(
            "antigravity",
            None,
            None,
            TurnOutcome::Interrupted,
            &turn,
            Some("stale error from an earlier turn"),
        );
        assert_eq!(stopped["errored"], false);
        assert_eq!(stopped["outcome"], "interrupted");
        assert_eq!(stopped["error_kind"], Value::Null);
        assert_eq!(stopped["exit_code"], Value::Null);
        assert_eq!(stopped["provider_version"], Value::Null);
    }
}
