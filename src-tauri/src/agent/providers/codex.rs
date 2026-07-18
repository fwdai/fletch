//! Codex arg builders + transcript reader.
//!
//! Codex writes `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`.
//! Lines are `{timestamp,type,payload}` dual-channel with no stable per-line id,
//! so records key positionally. The codex frontend adapter already normalizes.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::args::{model_args, push_opt};
use crate::agent::transcript::{records_with_id, RawRecord, ReadDiagnostics};
use crate::agent::TurnArgs;
use crate::instructions;

use super::gated_session_id;

pub(crate) fn codex_locate(
    session_id: &str,
    _cwd: &Path,
    diag: &mut ReadDiagnostics,
) -> Vec<PathBuf> {
    crate::transcripts::find_codex_rollouts(session_id, diag)
}

pub(crate) fn codex_read(paths: &[PathBuf], diag: &mut ReadDiagnostics) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::transcripts::read_jsonl_values(p, diag))
        .collect();
    records_with_id(values, None)
}

/// Codex: `codex exec [resume <id>] --json …`. Approvals off and codex's own
/// sandbox set to `danger-full-access` via `-c` (works on both `exec` and
/// `exec resume`, unlike the `-s`/`-a` flags). Fletch now runs codex under
/// sandbox-exec like every other agent, so codex's own confinement is disabled
/// to leave a single boundary — and so codex can reach its RPC mailbox, which
/// lives outside the checkout that `workspace-write` would have confined it to.
pub(crate) fn codex_build_args(turn: &TurnArgs) -> Vec<String> {
    let &TurnArgs {
        prompt,
        session_id,
        thinking,
        model,
        extra,
        mcp_args,
    } = turn;
    let mut args: Vec<String> = vec!["exec".into()];
    push_opt(&mut args, "resume", session_id);
    args.push("--json".into());
    args.push("--skip-git-repo-check".into());
    args.push("-c".into());
    args.push("approval_policy=\"never\"".into());
    args.push("-c".into());
    args.push("sandbox_mode=\"danger-full-access\"".into());
    if let Some(effort) = thinking {
        args.push("-c".into());
        args.push(format!("reasoning_effort=\"{effort}\""));
    }
    args.extend(model_args(model));
    args.extend(instructions::codex_config_args(extra));
    // The session's MCP servers as `-c mcp_servers.*` overrides (see
    // `agent_profile::codex_mcp_args`), re-passed every turn like the rest of
    // the config since `codex exec` reads config per invocation.
    args.extend_from_slice(mcp_args);
    args.push(prompt.to_string());
    args
}

/// Codex assigns its thread id on the first turn via `thread.started`.
pub(crate) fn codex_session_id(event: &Value) -> Option<String> {
    gated_session_id(event, Some("thread.started"), None, "thread_id")
}

/// Codex: bare `codex` launches the interactive TUI;
/// `--dangerously-bypass-approvals-and-sandbox` runs it unattended (Fletch
/// already isolates the checkout). `resume <id>` continues a prior session.
pub(crate) fn codex_pty_args(
    session_id: Option<&str>,
    model: Option<&str>,
    extra: Option<&str>,
    mcp_args: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = vec!["--dangerously-bypass-approvals-and-sandbox".into()];
    args.extend(model_args(model));
    args.extend(instructions::codex_config_args(extra));
    args.extend_from_slice(mcp_args);
    push_opt(&mut args, "resume", session_id);
    args
}
