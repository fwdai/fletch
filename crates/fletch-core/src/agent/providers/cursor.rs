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
//!
//! A prompt alone is not unique — the same agent can be delegated the same work
//! twice — so the link key is the prompt's hash plus the call's occurrence
//! number (see [`suffixed`]). Sub-agent files are numbered in creation
//! order and Task calls in transcript order, which line up because Cursor
//! creates a sub-agent's file when the Task is called. If that ever stops
//! holding, attribution between two identically-prompted siblings may swap, but
//! each still gets its own row — the failure the ids exist to prevent.

use std::collections::HashMap;
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

/// The base replay id of a Cursor `Task` tool call, derived from its prompt
/// because the on-disk block carries no id of its own. The frontend's Cursor
/// `normalizeTranscript` computes the same id for the block (see
/// `cursorTaskId` in `src/adapters/cursor/normalize.ts`), which is what lets
/// the reducer nest the sub-agent's records — tagged with this id by the sync —
/// under that call. FNV-1a (32-bit) over the prompt's UTF-16 code units: the
/// one encoding both sides get for free, and a hash small enough to write in
/// five lines each rather than share a dependency. Not unique on its own: see
/// [`suffixed`] for the occurrence suffix that makes it so.
pub(crate) fn cursor_task_id(prompt: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for unit in prompt.encode_utf16() {
        h ^= u32::from(unit);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("cursor-task-{h:08x}")
}

/// The replay id of the `n`-th (1-based) Task call with base id `base`: the
/// base itself for the first, `<base>-<n>` after that. Numbering by base id
/// rather than by prompt text keeps two different prompts that collide in 32
/// bits on separate rows too. Mirrored by `cursorTaskIdNth` in
/// `src/adapters/cursor/normalize.ts`, which numbers the main transcript's Task
/// blocks the same way — a drift between the two un-nests every replayed Cursor
/// sub-agent.
fn suffixed(base: String, n: usize) -> String {
    if n > 1 {
        format!("{base}-{n}")
    } else {
        base
    }
}

/// The replay id of the next Task call with this prompt, bumping `counts` (base
/// id → occurrences seen so far) as it goes. Both sides of the link — the
/// sub-agent files and the Task blocks they came from — number their sequence
/// with this, so the two agree.
fn next_task_id(counts: &mut HashMap<String, usize>, prompt: &str) -> String {
    let base = cursor_task_id(prompt);
    let n = counts.entry(base.clone()).or_insert(0);
    *n += 1;
    suffixed(base, *n)
}

/// The user's query inside Cursor's user-turn envelope — a `<timestamp>` line
/// followed by the text in `<user_query>…</user_query>` — trimmed. `None` when
/// the envelope is absent.
fn cursor_user_query(text: &str) -> Option<&str> {
    let start = text.find("<user_query>")? + "<user_query>".len();
    let end = start + text[start..].find("</user_query>")?;
    Some(text[start..end].trim())
}

/// The query of a sub-agent file's first record, which is the prompt the
/// parent's Task call passed. Empty when the file has no complete first line
/// yet (just created) or it isn't the expected user message — such a file links
/// to nothing and is retried next pass.
fn cursor_subagent_query(path: &Path) -> String {
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

/// When a sub-agent file was created — the moment Cursor called its Task, which
/// is what puts the files in the same order as the Task calls. Filesystems
/// without a birth time fall back to the mtime; a file whose metadata can't be
/// read sorts first and is disambiguated by path.
fn cursor_subagent_birth(path: &Path) -> Option<std::time::SystemTime> {
    let meta = std::fs::metadata(path).ok()?;
    meta.created().or_else(|_| meta.modified()).ok()
}

/// `<session-id>/subagents/<uuid>.jsonl` files beside the main transcript
/// `<session-id>.jsonl`, in creation order and keyed by the replay id of the
/// Task that spawned each — the base hash of its first user query plus that
/// call's occurrence number, so two sub-agents delegated the same prompt get
/// distinct keys. A file with no readable first line keeps an empty key and
/// links to nothing (retried next pass) without consuming an occurrence.
fn cursor_subagent_files(main: &Path) -> Vec<(String, PathBuf)> {
    let dir = main.with_extension("").join("subagents");
    let mut paths: Vec<(Option<std::time::SystemTime>, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .map(|path| (cursor_subagent_birth(&path), path))
        .collect();
    paths.sort();
    let mut counts: HashMap<String, usize> = HashMap::new();
    paths
        .into_iter()
        .map(|(_, path)| {
            let query = cursor_subagent_query(&path);
            let key = if query.is_empty() {
                String::new()
            } else {
                next_task_id(&mut counts, &query)
            };
            (key, path)
        })
        .collect()
}

/// Marker every stored record holding a Task call contains (serde's compact
/// serialization of the block's `"name": "Task"`), so the sync deserializes
/// only those instead of the whole conversation.
const CURSOR_TASK_NEEDLE: &str = "\"name\":\"Task\"";

/// Whether any Task call across `bodies` — the stored main-transcript records
/// holding one, in transcript order — has replay id `key`; `key` itself if so.
/// The id is the call's prompt hash plus its occurrence number, which only
/// comes out right when the whole sequence is numbered at once, hence the
/// slice. Records carrying a top-level `parent_tool_use_id` are a sub-agent's
/// own: a Task nested in one isn't part of the main transcript's numbering (nor
/// of the frontend's, which skips them the same way), so they're skipped.
fn cursor_subagent_parent(bodies: &[Value], key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    let found = bodies
        .iter()
        .filter(|body| body.get("parent_tool_use_id").is_none())
        .filter_map(|body| body.pointer("/message/content").and_then(Value::as_array))
        .flatten()
        .filter(|block| {
            block.get("type").and_then(Value::as_str) == Some("tool_use")
                && block.get("name").and_then(Value::as_str) == Some("Task")
        })
        .filter_map(|block| block.pointer("/input/prompt").and_then(Value::as_str))
        .any(|prompt| next_task_id(&mut counts, prompt.trim()) == key);
    found.then(|| key.to_string())
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
    fn repeated_task_ids_are_numbered_from_the_second_occurrence() {
        // Pinned against cursorTaskIdNth in src/adapters/cursor/normalize.ts.
        let base = cursor_task_id("look");
        assert_eq!(suffixed(base.clone(), 1), base);
        assert_eq!(suffixed(base.clone(), 2), format!("{base}-2"));
        assert_eq!(suffixed(base.clone(), 3), format!("{base}-3"));

        // The counter is keyed by base id, so identical prompts advance
        // together and different ones never share a number.
        let mut counts = HashMap::new();
        assert_eq!(next_task_id(&mut counts, "look"), base);
        assert_eq!(next_task_id(&mut counts, "other"), cursor_task_id("other"));
        assert_eq!(next_task_id(&mut counts, "look"), format!("{base}-2"));
    }

    #[test]
    fn subagent_files_are_keyed_by_the_replay_id_of_the_task_that_spawned_them() {
        let td = tempfile::tempdir().unwrap();
        let main = td.path().join("sess-1.jsonl");
        let nested = td.path().join("sess-1").join("subagents");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(&main, "{}\n").unwrap();
        // Written in path order so creation order and the path tiebreak agree
        // whatever the filesystem's timestamp resolution; ordering by creation
        // time is covered by the test below.
        std::fs::write(
            nested.join("a1.jsonl"),
            format!("{}\n", first_line("first task")),
        )
        .unwrap();
        std::fs::write(
            nested.join("b2.jsonl"),
            format!(
                "{}\n{{\"role\":\"assistant\"}}\n",
                first_line("second task")
            ),
        )
        .unwrap();
        // Just created: no complete first line yet → empty key, links nothing.
        std::fs::write(nested.join("c3.jsonl"), "{\"role\":\"us").unwrap();
        // Not a transcript.
        std::fs::write(nested.join("notes.txt"), "").unwrap();

        assert_eq!(
            cursor_subagent_files(&main),
            vec![
                (cursor_task_id("first task"), nested.join("a1.jsonl")),
                (cursor_task_id("second task"), nested.join("b2.jsonl")),
                (String::new(), nested.join("c3.jsonl")),
            ]
        );
    }

    #[test]
    fn two_subagents_with_the_same_prompt_are_keyed_in_creation_order() {
        let td = tempfile::tempdir().unwrap();
        let main = td.path().join("sess-1.jsonl");
        let nested = td.path().join("sess-1").join("subagents");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(&main, "{}\n").unwrap();
        // Named so path order is the reverse of creation order: the key must
        // follow the clock, which is what matches the Task call order.
        let second = nested.join("aaaa.jsonl");
        let first = nested.join("zzzz.jsonl");
        let line = format!("{}\n", first_line("look at a.rs"));
        std::fs::write(&first, &line).unwrap();
        // Clear any filesystem timestamp granularity between the two.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&second, &line).unwrap();

        let base = cursor_task_id("look at a.rs");
        assert_eq!(
            cursor_subagent_files(&main),
            vec![(base.clone(), first), (format!("{base}-2"), second.clone())]
        );

        // Each key resolves to its own Task call, so the two never merge.
        let bodies = [task_record("look at a.rs"), task_record("look at a.rs")];
        assert_eq!(
            cursor_subagent_parent(&bodies, &base).as_deref(),
            Some(base.as_str())
        );
        let second_key = format!("{base}-2");
        assert_eq!(
            cursor_subagent_parent(&bodies, &second_key).as_deref(),
            Some(second_key.as_str())
        );
        // Only two were called: a third sub-agent file links to nothing.
        assert_eq!(cursor_subagent_parent(&bodies, &format!("{base}-3")), None);
    }

    #[test]
    fn a_subagents_own_nested_task_does_not_shift_the_numbering() {
        let base = cursor_task_id("look at a.rs");
        let mut nested = task_record("look at a.rs");
        nested["parent_tool_use_id"] = json!("cursor-task-deadbeef");
        // The tagged record sits between the two main-transcript calls; if it
        // counted, the second would be numbered `-3`.
        let bodies = [
            task_record("look at a.rs"),
            nested,
            task_record("look at a.rs"),
        ];
        let second_key = format!("{base}-2");
        assert_eq!(
            cursor_subagent_parent(&bodies, &second_key).as_deref(),
            Some(second_key.as_str())
        );
        assert_eq!(cursor_subagent_parent(&bodies, &format!("{base}-3")), None);
    }

    #[test]
    fn subagent_files_is_empty_when_the_session_spawned_none() {
        let td = tempfile::tempdir().unwrap();
        assert!(cursor_subagent_files(&td.path().join("sess-1.jsonl")).is_empty());
    }

    #[test]
    fn subagent_parent_links_the_task_whose_replay_id_matches() {
        let prompt = "Explore src/ for seams.\n\nReport back.";
        let bodies = [task_record(prompt)];
        assert_eq!(
            cursor_subagent_parent(&bodies, &cursor_task_id(prompt)),
            Some(cursor_task_id(prompt))
        );
        // Another Task's sub-agent, a non-Task tool, a user record, no key.
        assert_eq!(
            cursor_subagent_parent(&bodies, &cursor_task_id("something else")),
            None
        );
        let other_tool = json!({ "role": "assistant", "message": { "content": [
            { "type": "tool_use", "name": "Shell", "input": { "prompt": "look" } } ] } });
        let user = json!({ "role": "user", "message": { "content": [
            { "type": "text", "text": "look" }] } });
        assert_eq!(
            cursor_subagent_parent(&[other_tool, user], &cursor_task_id("look")),
            None
        );
        assert_eq!(cursor_subagent_parent(&[task_record("look")], ""), None);
        // The file's query comes out of an envelope that pads it with
        // newlines, so the Task's prompt is trimmed before hashing — as the
        // frontend's normalizeTranscript does.
        assert_eq!(
            cursor_subagent_parent(&[task_record("look\n")], &cursor_task_id("look")),
            Some(cursor_task_id("look"))
        );
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
