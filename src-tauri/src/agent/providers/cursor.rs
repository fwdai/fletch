//! Cursor arg builders + transcript reader.
//!
//! cursor-agent writes `~/.cursor/projects/<slug>/agent-transcripts/<id>/<id>.jsonl`.
//! The session-id dir is unique, so glob by it (like claude) rather than
//! reverse-engineering the undocumented slug. Lines have no per-line id →
//! positional keys.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::args::{model_args, push_opt};
use crate::agent::transcript::{records_with_id, RawRecord, ReadDiagnostics};
use crate::agent::TurnArgs;
use crate::instructions;

use super::gated_session_id;

pub(crate) fn cursor_locate(
    session_id: &str,
    cwd: &Path,
    diag: &mut ReadDiagnostics,
) -> Vec<PathBuf> {
    // Cursor writes transcripts under `$HOME`, and Fletch spawns it with a
    // per-agent HOME so its MCP config is private (see
    // `agent_profile::cursor_mcp_delivery`). So look there first, derived from
    // the checkout the same way the spawn derived it. Falling back to the real
    // home keeps sessions recorded before that change readable.
    let Some(home) = crate::agent_profile::agent_home_from_checkout(cwd)
        .filter(|h| h.join(".cursor").is_dir())
        .or_else(dirs::home_dir)
    else {
        return Vec::new();
    };
    let rel = format!("agent-transcripts/{session_id}/{session_id}.jsonl");
    let projects = home.join(".cursor").join("projects");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&projects) {
        diag.root_exists = true;
        for entry in entries.flatten() {
            let path = entry.path().join(&rel);
            if path.exists() {
                out.push(path);
            }
        }
    }
    out.sort();
    diag.files_matched += out.len();
    out
}

pub(crate) fn cursor_read(paths: &[PathBuf], diag: &mut ReadDiagnostics) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::transcripts::read_jsonl_values(p, diag))
        .collect();
    records_with_id(values, None)
}

/// Cursor: `cursor-agent -p --output-format stream-json --force [--resume <id>] <prompt>`.
/// `--force` runs commands without approval prompts; `--trust` trusts the
/// workspace in headless mode. Cursor's own sandbox applies; cwd comes from
/// the child process working directory.
pub(crate) fn cursor_build_args(turn: &TurnArgs) -> Vec<String> {
    let &TurnArgs {
        prompt,
        session_id,
        model,
        extra,
        mcp_args,
        ..
    } = turn;
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--force".into(),
        "--trust".into(),
    ];
    // `--approve-mcps` from `agent_profile::cursor_mcp_delivery`. Without it the
    // servers it wrote into the per-agent HOME sit at "needs approval" and never
    // load, which a headless per-turn run can never resolve. Empty when the
    // session has no servers.
    args.extend_from_slice(mcp_args);
    push_opt(&mut args, "--resume", session_id);
    args.extend(model_args(model));
    // Prompt is positional and must come after options.
    args.push(instructions::prepend_to_prompt(prompt, session_id, extra));
    args
}

/// Cursor reports its session id on the `system`/`init` event (echoed on every
/// later event; `maybe_capture_session_id` keeps only the first).
pub(crate) fn cursor_session_id(event: &Value) -> Option<String> {
    gated_session_id(
        event,
        Some("system"),
        Some(("subtype", "init")),
        "session_id",
    )
}

/// Cursor: bare `cursor-agent` launches the TUI; `--force` auto-allows
/// commands. `--resume <id>` continues a prior chat. `_extra` unused: cursor
/// has no system-prompt slot, so the brief was prepended on the first
/// Custom-view turn and now lives in the resumed conversation.
pub(crate) fn cursor_pty_args(
    session_id: Option<&str>,
    model: Option<&str>,
    _extra: Option<&str>,
    mcp_args: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = vec!["--force".into()];
    // Same approval flag as the per-turn path, so switching into the native TUI
    // keeps the tool set the Custom-view turns had.
    args.extend_from_slice(mcp_args);
    args.extend(model_args(model));
    push_opt(&mut args, "--resume", session_id);
    args
}
