//! Antigravity (agy) arg builders + transcript reader.
//!
//! agy has no JSON event stream (its `--print` output is plaintext), so it runs
//! as a `plaintext` per-turn agent: the runner drains stdout, the turn's process
//! exit ends the turn, and history comes entirely from its on-disk transcript.
//! The conversation id (== session id) lives in agy's filesystem, not its output.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::args::push_opt;
use crate::agent::transcript::{RawRecord, ReadDiagnostics};
use crate::agent::TurnArgs;
use crate::instructions;

// `_model` is intentionally unused: agy's `--print` runner ignores model
// selection (the `--model` flag is inert in print mode), so the picker offers
// no selectable models for antigravity (see `model_catalog::discover_one`).
pub(crate) fn antigravity_build_args(turn: &TurnArgs) -> Vec<String> {
    let &TurnArgs {
        prompt,
        session_id,
        extra,
        ..
    } = turn;
    // `--print` takes the prompt as its *value* (i.e. `--print <prompt>`), so the
    // prompt must come last, directly after `--print`. Putting another flag
    // between them makes that flag the prompt (agy then "answers" the flag name).
    let mut args = vec!["--dangerously-skip-permissions".to_string()];
    push_opt(&mut args, "--conversation", session_id);
    args.push("--print".into());
    args.push(instructions::prepend_to_prompt(prompt, session_id, extra));
    args
}

// `_model` unused: agy's TUI manages its own model selection (see
// `antigravity_build_args` and `model_catalog::discover_one`). `_extra` unused:
// the standing brief rides the first `--print` turn (above), which lives in the
// resumed conversation the TUI then continues.
pub(crate) fn antigravity_pty_args(
    session_id: Option<&str>,
    _model: Option<&str>,
    _extra: Option<&str>,
    _mcp_args: &[String],
) -> Vec<String> {
    // Native view: launch agy's interactive TUI (NOT `--print`, the
    // non-interactive turn runner), resuming the conversation by id.
    let mut args = vec!["--dangerously-skip-permissions".to_string()];
    push_opt(&mut args, "--conversation", session_id);
    args
}

/// agy stores `cwd → conversationId` in
/// `~/.gemini/antigravity-cli/cache/last_conversations.json` (the checkout cwd
/// is the key). Read it at turn-end to capture the id for resume + transcript.
pub(crate) fn antigravity_session_id_from_cwd(cwd: &Path) -> Option<String> {
    let home = dirs::home_dir()?;
    let path = home.join(".gemini/antigravity-cli/cache/last_conversations.json");
    let text = std::fs::read_to_string(path).ok()?;
    antigravity_conv_id_from_map(&text, &cwd.to_string_lossy())
}

/// Pure: extract the conversation id for `cwd` from the last-conversations map.
pub(crate) fn antigravity_conv_id_from_map(json_text: &str, cwd: &str) -> Option<String> {
    let map: Value = serde_json::from_str(json_text).ok()?;
    map.get(cwd).and_then(|v| v.as_str()).map(str::to_string)
}

pub(crate) fn antigravity_locate(
    session_id: &str,
    cwd: &Path,
    diag: &mut ReadDiagnostics,
) -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    // root_exists tracks the CLI's home dir, not the conv-id indirection: a
    // missing/unparseable `last_conversations.json` or a missing cwd key leaves
    // the CLI installed (root present) but the conversation unresolved, so it
    // reads as `files_matched == 0` (ambiguous NoFiles), not NoRoot. Only the
    // whole `~/.gemini/antigravity-cli` dir vanishing is a NoRoot drift signal.
    let vendor_root = home.join(".gemini/antigravity-cli");
    diag.root_exists = vendor_root.exists();
    // Prefer the captured id; fall back to the cwd→id map (e.g. the first turn,
    // before the id has been persisted).
    let id = if session_id.is_empty() {
        match antigravity_session_id_from_cwd(cwd) {
            Some(i) => i,
            None => return Vec::new(),
        }
    } else {
        session_id.to_string()
    };
    let path = vendor_root
        .join("brain")
        .join(&id)
        .join(".system_generated/logs/transcript_full.jsonl");
    if path.exists() {
        diag.files_matched += 1;
        vec![path]
    } else {
        Vec::new()
    }
}

pub(crate) fn antigravity_read(paths: &[PathBuf], diag: &mut ReadDiagnostics) -> Vec<RawRecord> {
    paths
        .iter()
        .flat_map(|p| crate::transcripts::read_jsonl_values(p, diag))
        .enumerate()
        .map(|(i, body)| {
            // `step_index` is a stable, monotonic per-conversation key.
            let native_id = body
                .get("step_index")
                .and_then(|v| v.as_i64())
                .map(|n| format!("step:{n}"))
                .unwrap_or_else(|| format!("ln:{i}"));
            RawRecord { native_id, body }
        })
        .collect()
}
