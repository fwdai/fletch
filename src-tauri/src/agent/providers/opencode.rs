//! OpenCode arg builders + transcript reader.
//!
//! OpenCode stores a blob store under `$XDG_DATA_HOME/opencode/storage` (defaults
//! to `~/.local/share/opencode/storage`, even on macOS): message blobs at
//! `message/<ses>/<msg>.json` (role + metadata, no content) and part blobs at
//! `part/<msg>/<part>.json` (the content). We emit each message record then its
//! parts, in id order (ids are time-sortable); the frontend reassembles. ids are
//! globally unique, so they're the native dedup key.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::args::{model_args, push_opt};
use crate::agent::transcript::{json_files_in, read_json_value, RawRecord, ReadDiagnostics};
use crate::agent::TurnArgs;
use crate::instructions;

use super::gated_session_id;

fn opencode_storage_root() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))?;
    Some(base.join("opencode").join("storage"))
}

pub(crate) fn opencode_locate(
    session_id: &str,
    _cwd: &Path,
    diag: &mut ReadDiagnostics,
) -> Vec<PathBuf> {
    let Some(root) = opencode_storage_root() else {
        return Vec::new();
    };
    diag.root_exists = root.exists();
    let out = json_files_in(&root.join("message").join(session_id));
    diag.files_matched += out.len();
    out
}

pub(crate) fn opencode_read(
    message_paths: &[PathBuf],
    diag: &mut ReadDiagnostics,
) -> Vec<RawRecord> {
    // Each blob is one JSON object (not a JSONL line): count it as one
    // `lines_seen`, bump `io_errors` when unreadable/unparseable, and only
    // `records_parsed` once we've confirmed its expected `id` shape — so an
    // OpenCode format change (readable JSON, but the id moved) reads as drift.
    let read_one = |path: &Path, diag: &mut ReadDiagnostics| -> Option<(String, Value)> {
        diag.lines_seen += 1;
        let Some(v) = read_json_value(path) else {
            diag.io_errors += 1;
            return None;
        };
        let id = v.get("id").and_then(|x| x.as_str())?.to_string();
        diag.records_parsed += 1;
        Some((id, v))
    };

    let mut out = Vec::new();
    for msg_path in message_paths {
        let Some((msg_id, msg)) = read_one(msg_path, diag) else {
            continue;
        };
        out.push(RawRecord {
            native_id: msg_id.clone(),
            body: msg,
        });
        // Parts live at `<storage>/part/<msg_id>/`; derive <storage> from the
        // message path `<storage>/message/<ses>/<msg>.json` (three parents up).
        let Some(part_dir) = msg_path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|storage| storage.join("part").join(&msg_id))
        else {
            continue;
        };
        for pf in json_files_in(&part_dir) {
            // Part blobs count toward files_matched too (locate only saw the
            // message blobs).
            diag.files_matched += 1;
            if let Some((pid, part)) = read_one(&pf, diag) {
                out.push(RawRecord {
                    native_id: pid,
                    body: part,
                });
            }
        }
    }
    out
}

/// OpenCode: `opencode run --format json --dangerously-skip-permissions [--session <id>] <prompt>`.
/// `--dangerously-skip-permissions` auto-approves tools (incl. shell + file
/// writes) so turns run unattended; verified end-to-end against opencode
/// 1.15.12. OpenCode runs in the child's cwd (no `--dir` needed) and assigns
/// its own session id on the first turn. The prompt is positional and must
/// come after the flags.
pub(crate) fn opencode_build_args(turn: &TurnArgs) -> Vec<String> {
    let &TurnArgs {
        prompt,
        session_id,
        thinking,
        model,
        extra,
        ..
    } = turn;
    let mut args: Vec<String> = vec![
        "run".into(),
        "--format".into(),
        "json".into(),
        "--dangerously-skip-permissions".into(),
        // Surface the model's reasoning as `reasoning` events (captured by the
        // opencode reducer and persisted via opencode_is_durable).
        "--thinking".into(),
    ];
    if let Some(variant) = thinking {
        args.push("--variant".into());
        args.push(variant.to_string());
    }
    args.extend(model_args(model));
    push_opt(&mut args, "--session", session_id);
    args.push(instructions::prepend_to_prompt(prompt, session_id, extra));
    args
}

/// OpenCode stamps the session id (`ses_…`) on the top-level `sessionID` field
/// of every event, so the first event of the first turn carries it (no type
/// gate); `maybe_capture_session_id` keeps the first and ignores later echoes.
pub(crate) fn opencode_session_id(event: &Value) -> Option<String> {
    gated_session_id(event, None, None, "sessionID")
}

/// OpenCode: bare `opencode` launches the interactive TUI; `--session <id>`
/// continues a prior session. Note: no auto-approve flag — that's
/// `--dangerously-skip-permissions`, which belongs to the `run` (headless)
/// subcommand and makes the *default* (TUI) command print help and exit. The
/// TUI prompts for tool permissions interactively, which the native view
/// handles like any other keystroke.
pub(crate) fn opencode_pty_args(
    session_id: Option<&str>,
    model: Option<&str>,
    _extra: Option<&str>,
    _mcp_args: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    args.extend(model_args(model));
    push_opt(&mut args, "--session", session_id);
    args
}
