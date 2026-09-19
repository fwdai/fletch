//! Codex arg builders + transcript reader.
//!
//! Codex writes `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`.
//! Lines are `{timestamp,type,payload}` dual-channel with no stable per-line id,
//! so records key positionally. The codex frontend adapter already normalizes.
//!
//! A sub-agent (the `collaboration` tools: `spawn_agent` & co.) gets a rollout
//! of its own in that same tree, named by its *own* thread id — nothing in the
//! path says whose child it is. The link is in the two files' contents,
//! verified on real rollouts (codex-cli 0.153.4):
//!
//! - the child's first line is a `session_meta` whose
//!   `payload.source.subagent.thread_spawn.parent_thread_id` is the parent's
//!   thread id (`payload.id` is the child's; its `payload.session_id` names the
//!   parent too, but `forked_from_id` does as well for a plain fork, so only
//!   the `subagent` source is trusted);
//! - the parent records `event_msg` / `item_completed` with an item
//!   `{type: "SubAgentActivity", kind: "started", id, agent_thread_id}` right
//!   after the `spawn_agent` function_call, and that item's `id` IS the spawn
//!   call's `call_id` (the later `completed`/`interrupted` activities carry
//!   ids of their own).
//!
//! A sub-agent can spawn sub-agents of its own: the grandchild's `session_meta`
//! names its *immediate* spawner as `parent_thread_id` (with `depth: 2`), and
//! its `SubAgentActivity` / `started` record lives in the child's rollout, not
//! the top-level one. So the child rollouts of a session are the transitive
//! closure, not just the threads naming the top-level id.
//!
//! The sync ingests child rollouts via [`CODEX_SUBAGENTS`], tagged with that
//! call id, so the frontend nests them under the spawn call on replay — at any
//! depth, since the reducer routes a record to a child thread recursively.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::args::{model_args, push_opt};
use crate::agent::transcript::{records_with_id, RawRecord, ReadDiagnostics, SubagentLayout};
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

/// A rollout's first line, unparsed. Every rollout (parent or child, all 236
/// on the reference machine) opens with its `session_meta`, so one line is all
/// that is ever read of a file this module only needs to identify.
fn rollout_first_line(path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let mut line = String::new();
    BufReader::new(std::fs::File::open(path).ok()?)
        .read_line(&mut line)
        .ok()?;
    Some(line)
}

/// The `session_meta` record on a rollout's first line, if that is what it is.
fn rollout_session_meta(path: &Path) -> Option<Value> {
    let v: Value = serde_json::from_str(&rollout_first_line(path)?).ok()?;
    (v.get("type").and_then(Value::as_str) == Some("session_meta")).then_some(v)
}

/// `(this thread's id, the id of the thread that spawned it)` from a rollout's
/// first line, if that line is a sub-agent `session_meta`. Only the first line
/// identifies a file: a child forked with `fork_turns` replays its parent's
/// early records, second `session_meta` line and all.
fn rollout_spawned_by(path: &Path) -> Option<(String, String)> {
    let line = rollout_first_line(path)?;
    // Rejected on the substring before parsing what is a ~20 KB record (it
    // carries the base instructions).
    if !line.contains("\"subagent\"") {
        return None;
    }
    let meta: Value = serde_json::from_str(&line).ok()?;
    let spawned_by = meta
        .pointer("/payload/source/subagent/thread_spawn/parent_thread_id")?
        .as_str()?
        .to_string();
    let id = meta.pointer("/payload/id")?.as_str()?.to_string();
    Some((id, spawned_by))
}

/// Descendant rollouts of the session whose rollout is `main` — the transitive
/// closure, so a sub-agent's own sub-agents are included — as
/// `(thread id, path)` in creation order.
///
/// One ordered pass suffices: a thread is spawned only after its spawner's
/// rollout exists, and paths sort chronologically, so every rollout is reached
/// *after* the one that spawned it. Carrying the set of ids admitted so far is
/// therefore enough to decide each file on sight, with no second sweep. That
/// order is also what the sync needs — a spawner's records (which carry the
/// `SubAgentActivity` linking its children) are ingested before its children's.
///
/// Cost: one listing of the `YYYY/MM/DD` tree plus one first-line read per
/// rollout created after `main` — everything up to it is skipped without being
/// opened.
fn codex_subagent_files(main: &Path) -> Vec<(String, PathBuf)> {
    let Some(top_id) = rollout_session_meta(main)
        .and_then(|meta| meta.pointer("/payload/id")?.as_str().map(str::to_string))
    else {
        return Vec::new();
    };
    // `<sessions>/YYYY/MM/DD/<file>`: the root is four levels up.
    let Some(sessions) = main.ancestors().nth(4) else {
        return Vec::new();
    };
    let mut included: HashSet<String> = HashSet::from([top_id]);
    let mut out = Vec::new();
    for path in crate::transcripts::codex_rollout_files(sessions) {
        if path.as_path() <= main {
            continue;
        }
        let Some((id, spawned_by)) = rollout_spawned_by(&path) else {
            continue;
        };
        if !included.contains(&spawned_by) {
            continue;
        }
        included.insert(id.clone());
        out.push((id, path));
    }
    out
}

/// The spawn call's id, if `body` is the parent's `SubAgentActivity` /
/// `started` record for the child thread `id` (see the module doc).
fn codex_subagent_parent(body: &Value, id: &str) -> Option<String> {
    if body.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let item = body.pointer("/payload/item")?;
    let field = |key: &str| item.get(key).and_then(Value::as_str);
    if field("type") != Some("SubAgentActivity")
        || field("kind") != Some("started")
        || field("agent_thread_id") != Some(id)
    {
        return None;
    }
    field("id").map(str::to_string)
}

pub(crate) const CODEX_SUBAGENTS: SubagentLayout = SubagentLayout {
    files: codex_subagent_files,
    parent_tool_use: codex_subagent_parent,
    // Rollout lines carry no id: positional, namespaced by the child id.
    id_field: None,
};

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A rollout's `session_meta` line in the on-disk shape (codex 0.153.4).
    /// `parent` set = a sub-agent spawned by that thread, at `depth` (1 for a
    /// child of the top-level thread, 2 for a child of a child, …).
    fn spawned_meta(id: &str, parent: &str, depth: u64) -> String {
        session_meta_at(id, Some(parent), depth)
    }

    /// `session_meta` for a thread at depth 1 (or a top-level one).
    fn session_meta(id: &str, parent: Option<&str>) -> String {
        session_meta_at(id, parent, 1)
    }

    fn session_meta_at(id: &str, parent: Option<&str>, depth: u64) -> String {
        let source = match parent {
            Some(p) => json!({ "subagent": { "thread_spawn": {
                "parent_thread_id": p, "depth": depth,
                "agent_path": "/root/task", "agent_nickname": "Hypatia", "agent_role": null,
            } } }),
            None => json!("exec"),
        };
        json!({
            "timestamp": "2026-09-19T12:48:06.674Z", "type": "session_meta",
            "payload": { "id": id, "session_id": parent.unwrap_or(id), "source": source,
                         "thread_source": if parent.is_some() { "subagent" } else { "user" } }
        })
        .to_string()
    }

    /// `<sessions>/YYYY/MM/DD/rollout-<ts>-<id>.jsonl` holding `lines`.
    fn write_rollout(sessions: &Path, day: &str, ts: &str, id: &str, lines: &[String]) -> PathBuf {
        let dir = sessions.join(day);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("rollout-{ts}-{id}.jsonl"));
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        path
    }

    #[test]
    fn subagent_files_are_the_later_rollouts_descending_from_this_thread() {
        let td = tempfile::tempdir().unwrap();
        let sessions = td.path().join("sessions");
        let main = write_rollout(
            &sessions,
            "2026/09/19",
            "2026-09-19T20-48-00",
            "parent",
            &[session_meta("parent", None)],
        );
        // Two children (one written on the next day), a grandchild spawned by
        // the first child, a sibling top-level session, another parent's child
        // and *its* child, an earlier file, and junk.
        let c1 = write_rollout(
            &sessions,
            "2026/09/19",
            "2026-09-19T20-48-06",
            "child-1",
            &[session_meta("child-1", Some("parent"))],
        );
        let grand = write_rollout(
            &sessions,
            "2026/09/19",
            "2026-09-19T20-48-30",
            "grandchild",
            &[spawned_meta("grandchild", "child-1", 2)],
        );
        let c2 = write_rollout(
            &sessions,
            "2026/09/20",
            "2026-09-20T01-00-00",
            "child-2",
            &[session_meta("child-2", Some("parent"))],
        );
        write_rollout(
            &sessions,
            "2026/09/19",
            "2026-09-19T20-49-00",
            "other",
            &[session_meta("other", None)],
        );
        write_rollout(
            &sessions,
            "2026/09/19",
            "2026-09-19T20-50-00",
            "other-child",
            &[session_meta("other-child", Some("other"))],
        );
        write_rollout(
            &sessions,
            "2026/09/19",
            "2026-09-19T20-51-00",
            "other-grandchild",
            &[spawned_meta("other-grandchild", "other-child", 2)],
        );
        write_rollout(
            &sessions,
            "2026/09/18",
            "2026-09-18T10-00-00",
            "early",
            &[session_meta("early", Some("parent"))],
        );
        std::fs::write(sessions.join("2026/09/19/notes.txt"), "\"subagent\"").unwrap();

        // The closure, in creation order — so a spawner always precedes the
        // threads it spawned. `other-grandchild` descends from a foreign
        // top-level session and stays out at every depth.
        assert_eq!(
            codex_subagent_files(&main),
            vec![
                ("child-1".to_string(), c1),
                ("grandchild".to_string(), grand),
                ("child-2".to_string(), c2)
            ]
        );
    }

    #[test]
    fn subagent_files_is_empty_for_an_unreadable_or_foreign_main() {
        let td = tempfile::tempdir().unwrap();
        assert!(codex_subagent_files(&td.path().join("missing.jsonl")).is_empty());
        let not_meta = td.path().join("a/b/c/d/rollout-x-y.jsonl");
        std::fs::create_dir_all(not_meta.parent().unwrap()).unwrap();
        std::fs::write(&not_meta, "{\"type\":\"event_msg\"}\n").unwrap();
        assert!(codex_subagent_files(&not_meta).is_empty());
    }

    /// The parent's persisted `SubAgentActivity` event, as on disk.
    fn activity(kind: &str, id: &str, thread: &str) -> Value {
        json!({
            "type": "event_msg",
            "payload": { "type": "item_completed", "thread_id": "parent", "item": {
                "type": "SubAgentActivity", "id": id, "kind": kind,
                "agent_thread_id": thread, "agent_path": "/root/task",
            } }
        })
    }

    #[test]
    fn subagent_parent_is_the_spawn_call_id_on_the_started_activity() {
        assert_eq!(
            codex_subagent_parent(&activity("started", "call_spawn", "child-1"), "child-1")
                .as_deref(),
            Some("call_spawn")
        );
    }

    #[test]
    fn subagent_parent_ignores_other_records() {
        // Another child's start.
        assert_eq!(
            codex_subagent_parent(&activity("started", "call_spawn", "child-2"), "child-1"),
            None
        );
        // The same child's completion / interruption carry ids of their own —
        // not the spawn call's.
        for kind in ["completed", "interrupted"] {
            assert_eq!(
                codex_subagent_parent(
                    &activity(kind, "subagent-completed-x", "child-1"),
                    "child-1"
                ),
                None,
                "{kind}"
            );
        }
        // The spawn function_call itself names no thread id.
        let call = json!({ "type": "response_item", "payload": {
            "type": "function_call", "name": "spawn_agent", "namespace": "collaboration",
            "call_id": "call_spawn", "arguments": "{\"task_name\":\"task\"}" } });
        assert_eq!(codex_subagent_parent(&call, "child-1"), None);
    }
}
