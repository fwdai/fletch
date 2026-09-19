//! Cursor arg builders + transcript reader.
//!
//! cursor-agent writes `~/.cursor/projects/<slug>/agent-transcripts/<id>/<id>.jsonl`.
//! The session-id dir is unique, so glob by it (like claude) rather than
//! reverse-engineering the undocumented slug. Lines have no per-line id →
//! positional keys.
//!
//! A sub-agent (the `Task` tool) gets its own file in that same dir,
//! `<id>/subagents/<uuid>.jsonl`, holding `{role, message}` records like the
//! main file plus a closing `{"type":"turn_ended"}` line — and nothing else: no
//! ids, no timestamps, no link to the parent. The main transcript holds the
//! spawning `tool_use` (`name: "Task"`, `input: {description, subagent_type,
//! model, prompt}`), itself without an id, and no `tool_result` for it. The
//! only thing the two files share is the prompt: the sub-agent's first record
//! is a user message whose `<user_query>` body equals the Task's `prompt`
//! (verified on real transcripts, cursor-agent 2026.06.19). The sync links on
//! that via [`CURSOR_SUBAGENTS`].

use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::args::{model_args, push_opt};
use crate::agent::transcript::{records_with_id, RawRecord, ReadDiagnostics, SubagentLayout};
use crate::agent::TurnArgs;
use crate::instructions;

use super::gated_session_id;

pub(crate) fn cursor_locate(
    session_id: &str,
    _cwd: &Path,
    diag: &mut ReadDiagnostics,
) -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
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

/// The replay id of a Cursor `Task` tool call, derived from its prompt because
/// the on-disk block carries no id of its own. The frontend's Cursor
/// `normalizeTranscript` computes the same id for the block (see
/// `cursorTaskId` in `src/adapters/cursor/normalize.ts`), which is what lets
/// the reducer nest the sub-agent's records — tagged with this id by the sync —
/// under that call. FNV-1a (32-bit) over the prompt's UTF-16 code units: the
/// one encoding both sides get for free, and a hash small enough to write in
/// five lines each rather than share a dependency. Two Tasks with identical
/// prompts collapse into one call — an ambiguity Cursor's format has anyway.
pub(crate) fn cursor_task_id(prompt: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for unit in prompt.encode_utf16() {
        h ^= u32::from(unit);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("cursor-task-{h:08x}")
}

/// The user's query inside Cursor's user-turn envelope — a `<timestamp>` line
/// followed by the text in `<user_query>…</user_query>` — trimmed. `None` when
/// the envelope is absent.
fn cursor_user_query(text: &str) -> Option<&str> {
    let start = text.find("<user_query>")? + "<user_query>".len();
    let end = start + text[start..].find("</user_query>")?;
    Some(text[start..end].trim())
}

/// The link key of one sub-agent file: the query of its first record, which is
/// the prompt the parent's Task call passed. Empty when the file has no
/// complete first line yet (just created) or it isn't the expected user
/// message — such a file links to nothing and is retried next pass.
fn cursor_subagent_key(path: &Path) -> String {
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut first = String::new();
    if std::io::BufReader::new(file).read_line(&mut first).is_err() || !first.ends_with('\n') {
        return String::new();
    }
    let Ok(record) = serde_json::from_str::<Value>(&first) else {
        return String::new();
    };
    if record.get("role").and_then(Value::as_str) != Some("user") {
        return String::new();
    }
    record
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .find_map(cursor_user_query)
        .unwrap_or_default()
        .to_string()
}

/// `<session-id>/subagents/<uuid>.jsonl` files beside the main transcript
/// `<session-id>.jsonl`, keyed by their first user query, sorted by path.
fn cursor_subagent_files(main: &Path) -> Vec<(String, PathBuf)> {
    let dir = main.with_extension("").join("subagents");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| (cursor_subagent_key(&path), path))
        .collect()
}

/// Marker every stored record holding a Task call contains (serde's compact
/// serialization of the block's `"name": "Task"`), so the sync deserializes
/// only those instead of the whole conversation.
const CURSOR_TASK_NEEDLE: &str = "\"name\":\"Task\"";

/// The replay id of the Task call in `body` whose `prompt` is `key`, if `body`
/// is the main-transcript assistant record that made it.
fn cursor_subagent_parent(body: &Value, key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    body.pointer("/message/content")?
        .as_array()?
        .iter()
        .filter(|block| {
            block.get("type").and_then(Value::as_str) == Some("tool_use")
                && block.get("name").and_then(Value::as_str) == Some("Task")
        })
        .filter_map(|block| block.pointer("/input/prompt").and_then(Value::as_str))
        .find(|prompt| prompt.trim() == key)
        .map(cursor_task_id)
}

pub(crate) const CURSOR_SUBAGENTS: SubagentLayout = SubagentLayout {
    files: cursor_subagent_files,
    needle: Some(CURSOR_TASK_NEEDLE),
    parent_tool_use: cursor_subagent_parent,
    // Records carry no id: positional, namespaced by the file's stem (the
    // sub-agent uuid).
    id_field: None,
};

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
        ..
    } = turn;
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--force".into(),
        "--trust".into(),
    ];
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
    _mcp_args: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = vec!["--force".into()];
    args.extend(model_args(model));
    push_opt(&mut args, "--resume", session_id);
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A sub-agent file's first record, in the shape real transcripts use.
    fn first_line(query: &str) -> String {
        json!({
            "role": "user",
            "message": { "content": [{ "type": "text", "text":
                format!("<timestamp>Friday, Sep 11, 2026, 11:07 AM (UTC+8)</timestamp>\n<user_query>\n{query}\n</user_query>") }] }
        })
        .to_string()
    }

    /// The main-transcript assistant record that spawned a sub-agent.
    fn task_record(prompt: &str) -> Value {
        json!({
            "role": "assistant",
            "message": { "content": [
                { "type": "text", "text": "Delegating." },
                { "type": "tool_use", "name": "Task", "input": {
                    "description": "Find seams", "subagent_type": "explore", "model": "inherit", "prompt": prompt } }
            ] }
        })
    }

    #[test]
    fn task_id_is_a_stable_fnv1a_of_the_prompt() {
        // Pinned so a drift on either side (see cursorTaskId in
        // src/adapters/cursor/normalize.ts) fails a test rather than a replay.
        assert_eq!(cursor_task_id(""), "cursor-task-811c9dc5");
        assert_eq!(cursor_task_id("look"), cursor_task_id("look"));
        assert_ne!(cursor_task_id("look"), cursor_task_id("look "));
        assert!(cursor_task_id("look").starts_with("cursor-task-"));
        assert_eq!(cursor_task_id("look").len(), "cursor-task-".len() + 8);
    }

    #[test]
    fn subagent_files_are_keyed_by_their_first_user_query() {
        let td = tempfile::tempdir().unwrap();
        let main = td.path().join("sess-1.jsonl");
        let nested = td.path().join("sess-1").join("subagents");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(&main, "{}\n").unwrap();
        std::fs::write(
            nested.join("b2.jsonl"),
            format!(
                "{}\n{{\"role\":\"assistant\"}}\n",
                first_line("second task")
            ),
        )
        .unwrap();
        std::fs::write(
            nested.join("a1.jsonl"),
            format!("{}\n", first_line("first task")),
        )
        .unwrap();
        // Just created: no complete first line yet → empty key, links nothing.
        std::fs::write(nested.join("c3.jsonl"), "{\"role\":\"us").unwrap();
        // Not a transcript.
        std::fs::write(nested.join("notes.txt"), "").unwrap();

        assert_eq!(
            cursor_subagent_files(&main),
            vec![
                ("first task".to_string(), nested.join("a1.jsonl")),
                ("second task".to_string(), nested.join("b2.jsonl")),
                (String::new(), nested.join("c3.jsonl")),
            ]
        );
    }

    #[test]
    fn subagent_files_is_empty_when_the_session_spawned_none() {
        let td = tempfile::tempdir().unwrap();
        assert!(cursor_subagent_files(&td.path().join("sess-1.jsonl")).is_empty());
    }

    #[test]
    fn subagent_parent_links_the_task_whose_prompt_matches() {
        let prompt = "Explore src/ for seams.\n\nReport back.";
        let body = task_record(prompt);
        assert_eq!(
            cursor_subagent_parent(&body, prompt),
            Some(cursor_task_id(prompt))
        );
        // Another Task's sub-agent, a non-Task tool, a user record, no key.
        assert_eq!(cursor_subagent_parent(&body, "something else"), None);
        let other_tool = json!({ "role": "assistant", "message": { "content": [
            { "type": "tool_use", "name": "Shell", "input": { "prompt": "look" } } ] } });
        assert_eq!(cursor_subagent_parent(&other_tool, "look"), None);
        let user = json!({ "role": "user", "message": { "content": [
            { "type": "text", "text": "look" }] } });
        assert_eq!(cursor_subagent_parent(&user, "look"), None);
        assert_eq!(cursor_subagent_parent(&task_record("look"), ""), None);
    }

    #[test]
    fn the_needle_matches_a_stored_task_record() {
        let stored = serde_json::to_string(&task_record("look")).unwrap();
        assert!(stored.contains(CURSOR_TASK_NEEDLE));
        let plain = serde_json::to_string(&json!({ "role": "assistant", "message": {
            "content": [{ "type": "text", "text": "the Task tool" }] } }))
        .unwrap();
        assert!(!plain.contains(CURSOR_TASK_NEEDLE));
    }
}
