//! Pi arg builders + transcript reader.
//!
//! Pi is the reference reader — its per-session JSONL feeds session_records.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::args::{model_args, push_opt};
use crate::agent::transcript::{jsonl_files_ending, records_with_id, RawRecord, ReadDiagnostics};
use crate::agent::TurnArgs;
use crate::instructions;

use super::gated_session_id;

/// Pi's session-dir slug: cwd with `/` → `-`, wrapped in `--…--`.
/// `/Users/alex/Code/amux` → `--Users-alex-Code-amux--`. Dots are preserved.
pub(crate) fn pi_session_slug(cwd: &Path) -> String {
    format!("-{}--", cwd.to_string_lossy().replace('/', "-"))
}

pub(crate) fn pi_locate(session_id: &str, cwd: &Path, diag: &mut ReadDiagnostics) -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let sessions = home.join(".pi/agent/sessions");
    diag.root_exists = sessions.exists();
    let dir = sessions.join(pi_session_slug(cwd));
    // Files are `<ts>_<session_id>.jsonl`.
    let out = jsonl_files_ending(&dir, &format!("_{session_id}.jsonl"));
    diag.files_matched += out.len();
    out
}

pub(crate) fn pi_read(paths: &[PathBuf], diag: &mut ReadDiagnostics) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::transcripts::read_jsonl_values(p, diag))
        .collect();
    // Pi's JSONL lines carry a stable `id`.
    records_with_id(values, Some("id"))
}

/// Pi: `pi -p --mode json [--session <id>] <prompt>`. `-p` runs one turn
/// non-interactively and exits; in that mode Pi auto-runs its tools (bash,
/// write, …) with no approval prompt. Pi assigns its own session id on the
/// first turn (captured from the `session` event), and `--session <id>`
/// resumes it. We deliberately use `--session` (not the newer `--session-id`):
/// it's the resume flag common to the versions we target — 0.74.x lacks
/// `--session-id` entirely. Verified end-to-end against pi 0.74.2. Pi runs in
/// the child's cwd; the prompt is positional and must come after the flags.
pub(crate) fn pi_build_args(turn: &TurnArgs) -> Vec<String> {
    let &TurnArgs {
        prompt,
        session_id,
        thinking,
        model,
        extra,
        ..
    } = turn;
    let mut args: Vec<String> = vec!["-p".into(), "--mode".into(), "json".into()];
    args.extend(model_args(model));
    if let Some(level) = thinking {
        args.push("--thinking".into());
        args.push(level.to_string());
    }
    args.extend(instructions::append_system_prompt_args(extra));
    push_opt(&mut args, "--session", session_id);
    args.push(prompt.to_string());
    args
}

/// Pi reports its session id on the first `{"type":"session","id":"…"}` event.
pub(crate) fn pi_session_id(event: &Value) -> Option<String> {
    gated_session_id(event, Some("session"), None, "id")
}

/// Pi: bare `pi` launches the interactive TUI (tools auto-run there).
/// `--session <id>` resumes — same flag the Custom-view runner uses, since the
/// versions we target (0.74.x) lack `--session-id`.
pub(crate) fn pi_pty_args(
    session_id: Option<&str>,
    model: Option<&str>,
    extra: Option<&str>,
    _mcp_args: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = instructions::append_system_prompt_args(extra);
    args.extend(model_args(model));
    push_opt(&mut args, "--session", session_id);
    args
}
