//! Pi arg builders + transcript reader and writer.
//!
//! Pi is the reference reader — its per-session JSONL feeds session_records.
//! A session file opens with a `session` header naming the session (`id`) and
//! the directory it runs in (`cwd`); every entry after it carries an `id` of
//! its own and its `parentId`, the tree pi rebuilds the conversation from.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::args::{model_args, push_opt};
use crate::agent::transcript::{
    jsonl_files_ending, records_with_id, replace_field, write_new_jsonl, RawRecord, ReadDiagnostics,
};
use crate::agent::TurnArgs;
use crate::error::{Error, Result};
use crate::instructions;

use super::gated_session_id;

/// Pi's session-dir slug: cwd with `/` → `-`, wrapped in `--…--`.
/// `/Users/alex/Code/amux` → `--Users-alex-Code-amux--`. Dots are preserved.
pub(crate) fn pi_session_slug(cwd: &Path) -> String {
    format!("-{}--", cwd.to_string_lossy().replace('/', "-"))
}

/// Where pi keeps its sessions: `~/.pi/agent/sessions`, one dir per cwd.
fn pi_sessions_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".pi/agent/sessions"))
}

pub(crate) fn pi_locate(
    session_id: &str,
    _agent_id: &str,
    cwd: &Path,
    diag: &mut ReadDiagnostics,
) -> Vec<PathBuf> {
    match pi_sessions_dir() {
        Some(sessions) => pi_locate_in(&sessions, session_id, cwd, diag),
        None => Vec::new(),
    }
}

/// [`pi_locate`] under the `sessions` root given.
fn pi_locate_in(
    sessions: &Path,
    session_id: &str,
    cwd: &Path,
    diag: &mut ReadDiagnostics,
) -> Vec<PathBuf> {
    diag.root_exists = sessions.exists();
    let dir = sessions.join(pi_session_slug(cwd));
    // Files are `<ts>_<session_id>.jsonl`.
    let out = jsonl_files_ending(&dir, &format!("_{session_id}.jsonl"));
    diag.files_matched += out.len();
    out
}

/// Write `bodies` as pi session `session_id`, run in `cwd`: the file
/// `<sessions>/<cwd slug>/<UTC time>_<id>.jsonl`, named as pi names its own
/// (0.78.0). `pi --session <id>` finds it among the cwd's sessions by its
/// header's `id`. The `session` header names the new session (`id`, and `cwd`
/// where present); the entries, their `id` / `parentId` tree included, are
/// copied as is.
pub(crate) fn pi_write(
    session_id: &str,
    _agent_id: &str,
    cwd: &Path,
    _container: bool,
    bodies: &[Value],
) -> Result<Option<PathBuf>> {
    let sessions = pi_sessions_dir()
        .ok_or_else(|| Error::Other("pi's sessions directory can't be resolved".into()))?;
    pi_write_in(&sessions, session_id, cwd, bodies).map(Some)
}

/// [`pi_write`] under the `sessions` root given.
fn pi_write_in(sessions: &Path, session_id: &str, cwd: &Path, bodies: &[Value]) -> Result<PathBuf> {
    let cwd_text = cwd.to_string_lossy();
    let lines: Vec<Value> = bodies
        .iter()
        .map(|body| {
            let mut line = body.clone();
            if line.get("type").and_then(Value::as_str) == Some("session") {
                replace_field(&mut line, "id", session_id);
                replace_field(&mut line, "cwd", &cwd_text);
            }
            line
        })
        .collect();
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S-%3fZ");
    let path = sessions
        .join(pi_session_slug(cwd))
        .join(format!("{stamp}_{session_id}.jsonl"));
    write_new_jsonl(&path, &lines)?;
    Ok(path)
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

/// Pi as a one-shot completion (`OneShot`), per `pi --help` (0.78.0): print
/// mode with a plain-text answer, `--no-tools` (built-in and extension tools
/// alike), `--no-session` so nothing is saved, and `--no-context-files` so no
/// AGENTS.md/CLAUDE.md is pulled in. In print mode pi takes piped stdin as the
/// message (its `readPipedStdin`, not in the help).
pub(crate) fn pi_one_shot_args(model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--mode",
        "text",
        "--no-tools",
        "--no-session",
        "--no-context-files",
    ]
    .map(String::from)
    .into();
    args.extend(model_args(model));
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NEW: &str = "1c2d3e4f-5a6b-4c7d-8e9f-0a1b2c3d4e5f";

    /// A session file as pi 0.78 writes it: the header, the model and
    /// thinking-level entries, then a turn with a tool call, each entry
    /// chained to the one before it by `parentId`.
    fn session_lines() -> Vec<Value> {
        let usage = json!({"input": 2, "output": 14, "cacheRead": 0, "cacheWrite": 3159,
                           "totalTokens": 3175, "cost": {"total": 0.02}});
        vec![
            json!({"type": "session", "version": 3, "id": "019eabc7-7b9d-77d6-9350-1af83fc36925",
                   "timestamp": "2026-06-09T09:47:17.789Z", "cwd": "/Users/u/.fletch/workspaces/old/repo"}),
            json!({"type": "model_change", "id": "056c9f24", "parentId": null,
                   "timestamp": "2026-06-09T09:47:17.794Z", "provider": "anthropic",
                   "modelId": "claude-opus-4-8"}),
            json!({"type": "thinking_level_change", "id": "0067544a", "parentId": "056c9f24",
                   "timestamp": "2026-06-09T09:47:17.794Z", "thinkingLevel": "low"}),
            json!({"type": "message", "id": "15b32be5", "parentId": "0067544a",
                   "timestamp": "2026-06-09T09:47:17.804Z",
                   "message": {"role": "user", "content": [{"type": "text", "text": "ping"}],
                               "timestamp": 1_780_998_437_803_u64}}),
            json!({"type": "message", "id": "52885254", "parentId": "15b32be5",
                   "timestamp": "2026-06-09T09:47:19.549Z",
                   "message": {"role": "assistant",
                               "content": [{"type": "toolCall", "id": "toolu_01", "name": "bash",
                                            "arguments": {"command": "echo pong"}}],
                               "provider": "anthropic", "model": "claude-opus-4-8",
                               "usage": usage, "stopReason": "toolUse"}}),
            json!({"type": "message", "id": "a4e67eb1", "parentId": "52885254",
                   "timestamp": "2026-06-09T09:47:19.946Z",
                   "message": {"role": "toolResult", "toolCallId": "toolu_01", "toolName": "bash",
                               "content": [{"type": "text", "text": "pong"}], "isError": false}}),
            json!({"type": "message", "id": "92069c57", "parentId": "a4e67eb1",
                   "timestamp": "2026-06-09T09:47:21.404Z",
                   "message": {"role": "assistant", "content": [{"type": "text", "text": "pong"}],
                               "provider": "anthropic", "model": "claude-opus-4-8",
                               "usage": usage, "stopReason": "stop"}}),
        ]
    }

    #[test]
    fn a_written_session_is_where_pi_resumes_it_with_only_its_identity_changed() {
        let td = tempfile::tempdir().unwrap();
        let sessions = td.path().join("sessions");
        let cwd = Path::new("/Users/u/.fletch/workspaces/new/repo");
        let lines = session_lines();

        let path = pi_write_in(&sessions, NEW, cwd, &lines).unwrap();

        // In the new cwd's dir, named `<time>_<id>.jsonl` as pi names its own.
        assert_eq!(path.parent().unwrap(), sessions.join(pi_session_slug(cwd)));
        let mut diag = ReadDiagnostics::default();
        let located = pi_locate_in(&sessions, NEW, cwd, &mut diag);
        assert_eq!(located, [path]);
        let read = pi_read(&located, &mut diag);
        assert_eq!(read.len(), lines.len());
        let mut header = lines[0].clone();
        header["id"] = json!(NEW);
        header["cwd"] = json!(cwd.to_string_lossy());
        assert_eq!(read[0].body, header);
        for (back, line) in read[1..].iter().zip(&lines[1..]) {
            assert_eq!(&back.body, line);
            assert_eq!(back.native_id, line["id"].as_str().unwrap());
        }
        // Another cwd's sessions don't include it.
        let elsewhere = Path::new("/Users/u/.fletch/workspaces/old/repo");
        assert!(pi_locate_in(&sessions, NEW, elsewhere, &mut diag).is_empty());
        // And every turn resumes it.
        let turn = pi_build_args(&TurnArgs {
            prompt: "go on",
            session_id: Some(NEW),
            ..Default::default()
        });
        assert!(turn.ends_with(&["--session".into(), NEW.into(), "go on".into()]));
        assert!(
            pi_pty_args(Some(NEW), None, None, &[]).ends_with(&["--session".into(), NEW.into()])
        );
    }
}
