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
use crate::agent::credential_file::{Entry, PrivateDir};
use crate::agent::transcript::{
    records_with_id, replace_field, RawRecord, ReadDiagnostics, SubagentLayout,
};
use crate::agent::TurnArgs;
use crate::error::{Error, Result};
use crate::instructions;

use super::gated_session_id;

pub(crate) fn codex_locate(
    session_id: &str,
    agent_id: &str,
    _cwd: &Path,
    diag: &mut ReadDiagnostics,
) -> Vec<PathBuf> {
    crate::transcripts::find_codex_rollouts(session_id, agent_id, diag)
}

pub(crate) fn codex_read(paths: &[PathBuf], diag: &mut ReadDiagnostics) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::transcripts::read_jsonl_values(p, diag))
        .collect();
    records_with_id(values, None)
}

/// Write `bodies` as codex thread `session_id` of agent `agent_id`, run in
/// `cwd`: a rollout `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-<local
/// time>-<id>.jsonl` in the agent's own overlay
/// (`codex_login::overlay_for_agent`), whichever account it runs under, named
/// as codex names its own (0.153.4). `codex exec resume <id>` finds it by the
/// id at the end of its name without it being in codex's sqlite index
/// (verified on 0.154.0), as [`codex_locate`] does. Its `session_meta` names
/// the new thread (`id`, and `session_id` and `cwd` where present);
/// everything else is copied as is.
pub(crate) fn codex_write(
    session_id: &str,
    agent_id: &str,
    cwd: &Path,
    _container: bool,
    bodies: &[Value],
) -> Result<Option<PathBuf>> {
    let overlay = crate::agent::codex_login::overlay_for_agent(agent_id)?;
    let sessions = crate::agent::codex_login::open_overlay(&overlay)?.subdir("sessions")?;
    codex_write_in(&sessions, session_id, cwd, bodies).map(Some)
}

/// Copy thread `session_id` from where codex kept it before agents had their
/// own `CODEX_HOME` (the default or an account home) into the overlay's
/// `sessions`, sub-agent threads included, at the same `YYYY/MM/DD` paths, so
/// an agent created before overlays still resumes its conversation. Copied,
/// not moved: the original stays where the user's own codex expects it (the
/// usage scan counts a file name once, so the copy isn't spend twice).
///
/// Run at every launch and checked file by file: each copy lands whole under
/// its final name (temp then rename), so a file that is there is complete,
/// and one an earlier launch failed to copy is tried again. One that is
/// there is never touched, since codex appends to it once it resumes.
/// Written through a handle on the overlay (`codex_login::open_overlay`), so
/// a link the agent planted in it is replaced, never followed. Once every
/// file of the thread is confirmed there, a marker in the overlay
/// ([`ADOPTED_DIRNAME`]) skips the walk of the legacy roots at later
/// launches.
pub(crate) fn adopt_legacy_rollouts(session_id: &str, overlay: &Path) -> Result<()> {
    adopt_rollouts_from(
        session_id,
        overlay,
        &crate::transcripts::legacy_codex_sessions_dirs(),
    )
}

/// The overlay dir holding one empty file per thread whose adoption is
/// complete, named for the thread id.
const ADOPTED_DIRNAME: &str = ".fletch-adopted";

/// [`adopt_legacy_rollouts`] from the `legacy` session roots given.
fn adopt_rollouts_from(session_id: &str, overlay: &Path, legacy: &[PathBuf]) -> Result<()> {
    let overlay = crate::agent::codex_login::open_overlay(overlay)?;
    let adopted = overlay.subdir(ADOPTED_DIRNAME)?;
    // A thread id that can't name a file is never marked, only walked.
    let marked = adopted.entry(session_id).ok();
    if marked.as_ref().is_some_and(|e| *e != Entry::Missing) {
        return Ok(());
    }
    let mut diag = ReadDiagnostics::default();
    let mains = legacy
        .iter()
        .flat_map(|root| crate::transcripts::find_codex_rollouts_in(root, session_id, &mut diag))
        .collect::<Vec<_>>();
    let sessions = overlay.subdir("sessions")?;
    // A thread the overlay already holds under any name is the copy codex
    // resumed and appends to; only its sub-agents' files are checked.
    let have_main =
        !crate::transcripts::find_codex_rollouts_in(sessions.path(), session_id, &mut diag)
            .is_empty();
    if mains.is_empty() && !have_main {
        // Not written yet, or gone: nothing to confirm.
        return Ok(());
    }
    let mut complete = true;
    for main in mains {
        let children = codex_subagent_files(&main).into_iter().map(|(_, p)| p);
        let main = (!have_main).then(|| main.clone());
        for path in main.into_iter().chain(children) {
            if let Err(e) = copy_rollout(&path, &sessions) {
                complete = false;
                tracing::warn!(error = %e, "could not copy a codex thread into the agent's home");
            }
        }
    }
    if complete && marked.is_some() {
        adopted.write_file(session_id, b"", 0o600)?;
    }
    Ok(())
}

/// The `YYYY/MM/DD` directory under `sessions`, created as needed.
fn rollout_dir(
    sessions: &PrivateDir,
    year: &str,
    month: &str,
    day: &str,
) -> std::io::Result<PrivateDir> {
    sessions.subdir(year)?.subdir(month)?.subdir(day)
}

/// Copy one rollout to the same `YYYY/MM/DD/<file>` under `sessions`, unless
/// a file of that name is already there.
fn copy_rollout(path: &Path, sessions: &PrivateDir) -> std::io::Result<()> {
    let names: Vec<&str> = path
        .components()
        .rev()
        .take(4)
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let [file, day, month, year] = names[..] else {
        return Err(std::io::Error::other("not a sessions/YYYY/MM/DD rollout"));
    };
    let dir = rollout_dir(sessions, year, month, day)?;
    if dir.entry(file)? != Entry::Missing {
        return Ok(());
    }
    let mut source = std::fs::File::open(path)?;
    dir.write_stream(file, &mut source, 0o600)
}

/// [`codex_write`] under the `sessions` dir given.
fn codex_write_in(
    sessions: &PrivateDir,
    session_id: &str,
    cwd: &Path,
    bodies: &[Value],
) -> Result<PathBuf> {
    let cwd = cwd.to_string_lossy();
    let mut out: Vec<u8> = Vec::new();
    for body in bodies {
        let mut line = body.clone();
        if line.get("type").and_then(Value::as_str) == Some("session_meta") {
            if let Some(meta) = line.get_mut("payload") {
                replace_field(meta, "id", session_id);
                replace_field(meta, "session_id", session_id);
                replace_field(meta, "cwd", &cwd);
            }
        }
        serde_json::to_writer(&mut out, &line)?;
        out.push(b'\n');
    }
    let now = chrono::Local::now();
    let (year, month, day) = (
        now.format("%Y").to_string(),
        now.format("%m").to_string(),
        now.format("%d").to_string(),
    );
    let file = format!(
        "rollout-{}-{session_id}.jsonl",
        now.format("%Y-%m-%dT%H-%M-%S")
    );
    let dir = rollout_dir(sessions, &year, &month, &day)?;
    if dir.entry(&file)? != Entry::Missing {
        return Err(Error::Other(format!(
            "a codex thread named {file} already exists"
        )));
    }
    dir.write_file(&file, &out, 0o600)?;
    Ok(dir.path().join(file))
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

/// The spawn call's id, from the parent's `SubAgentActivity` / `started` record
/// for the child thread `id` (see the module doc) — the first of `bodies` that
/// is one, since a thread id is spawned once.
fn codex_subagent_parent(bodies: &[Value], id: &str) -> Option<String> {
    bodies.iter().find_map(|body| {
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
    })
}

pub(crate) const CODEX_SUBAGENTS: SubagentLayout = SubagentLayout {
    files: codex_subagent_files,
    needle: None, // the child thread id is verbatim in the linking record
    parent_tool_use: codex_subagent_parent,
    // Rollout lines carry no id: positional, namespaced by the file's stem.
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

/// Codex as a one-shot completion (`OneShot`), per `codex exec --help`
/// (0.153.4): `exec` given no prompt reads it from stdin, `--sandbox
/// read-only` lets any command it runs only read, approvals never block,
/// `--skip-git-repo-check` lets it run in the empty scratch dir, and
/// `--ephemeral` persists no session. The answer is read back from the file
/// `--output-last-message` writes (the descriptor's `reply_flag`), so nothing
/// else codex prints can leak into it.
pub(crate) fn codex_one_shot_args(model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "exec",
        "--sandbox",
        "read-only",
        "-c",
        "approval_policy=\"never\"",
        "--skip-git-repo-check",
        "--ephemeral",
        "--color",
        "never",
    ]
    .map(String::from)
    .into();
    args.extend(model_args(model));
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
            codex_subagent_parent(&[activity("started", "call_spawn", "child-1")], "child-1")
                .as_deref(),
            Some("call_spawn")
        );
        // Found wherever it sits among the records the needle matched.
        let bodies = [
            activity("started", "call_other", "child-2"),
            activity("started", "call_spawn", "child-1"),
        ];
        assert_eq!(
            codex_subagent_parent(&bodies, "child-1").as_deref(),
            Some("call_spawn")
        );
    }

    #[test]
    fn subagent_parent_ignores_other_records() {
        // Another child's start.
        assert_eq!(
            codex_subagent_parent(&[activity("started", "call_spawn", "child-2")], "child-1"),
            None
        );
        // The same child's completion / interruption carry ids of their own —
        // not the spawn call's.
        for kind in ["completed", "interrupted"] {
            assert_eq!(
                codex_subagent_parent(
                    &[activity(kind, "subagent-completed-x", "child-1")],
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
        assert_eq!(codex_subagent_parent(&[call], "child-1"), None);
        assert_eq!(codex_subagent_parent(&[], "child-1"), None);
    }

    // ── writing a thread ──────────────────────────────────────────────────

    const NEW: &str = "6f1e2d3c-4b5a-4987-8a6b-5c4d3e2f1a0b";

    /// A real exec rollout (0.153.4): `session_meta`, a turn with reasoning,
    /// a tool call and its output, token counts.
    fn rollout_lines() -> Vec<Value> {
        include_str!("../../../../../tests/adapters/codex/fixtures/rollout-0153.jsonl")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn meta_line(id: &str, parent: Option<&str>) -> String {
        let source = match parent {
            Some(p) => json!({"subagent": {"thread_spawn": {"parent_thread_id": p}}}),
            None => json!("exec"),
        };
        json!({"type": "session_meta", "payload": {"id": id, "source": source}}).to_string()
    }

    fn legacy_rollout(root: &Path, ts: &str, id: &str, parent: Option<&str>) -> PathBuf {
        let day = root.join("2026").join("10").join("01");
        std::fs::create_dir_all(&day).unwrap();
        let path = day.join(format!("rollout-2026-10-01T{ts}-{id}.jsonl"));
        std::fs::write(&path, format!("{}\n", meta_line(id, parent))).unwrap();
        path
    }

    fn names_in(day: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(day)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_legacy_thread_and_its_subagents_are_copied_into_the_agents_home() {
        let td = tempfile::tempdir().unwrap();
        let legacy = td.path().join("legacy");
        let overlay = td.path().join("agent").join(".fletch-codex-home");
        let main = legacy_rollout(&legacy, "09-00-00", "main-1", None);
        legacy_rollout(&legacy, "09-05-00", "child-1", Some("main-1"));
        legacy_rollout(&legacy, "09-06-00", "other-1", None);

        adopt_rollouts_from("main-1", &overlay, std::slice::from_ref(&legacy)).unwrap();

        let day = overlay.join("sessions/2026/10/01");
        let mut copied: Vec<_> = std::fs::read_dir(&day)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        copied.sort();
        assert_eq!(
            copied,
            vec![
                "rollout-2026-10-01T09-00-00-main-1.jsonl",
                "rollout-2026-10-01T09-05-00-child-1.jsonl"
            ]
        );
        assert!(main.exists());
    }

    #[test]
    fn a_thread_the_agents_home_already_holds_is_not_copied_again() {
        let td = tempfile::tempdir().unwrap();
        let legacy = td.path().join("legacy");
        let overlay = td.path().join("agent").join(".fletch-codex-home");
        let sessions = overlay.join("sessions");
        legacy_rollout(&legacy, "09-00-00", "main-1", None);
        let own = legacy_rollout(&sessions, "10-00-00", "main-1", None);
        std::fs::write(&own, "appended by codex\n").unwrap();

        adopt_rollouts_from("main-1", &overlay, &[legacy]).unwrap();

        let mut diag = ReadDiagnostics::default();
        let found = crate::transcripts::find_codex_rollouts_in(&sessions, "main-1", &mut diag);
        assert_eq!(found, vec![own.clone()]);
        assert_eq!(std::fs::read_to_string(own).unwrap(), "appended by codex\n");
    }

    /// A sub-agent's file that an earlier launch failed to copy is copied by
    /// the next, though the main thread is already there.
    #[cfg(unix)]
    #[test]
    fn an_adoption_cut_short_is_finished_at_the_next_launch() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::tempdir().unwrap();
        let legacy = td.path().join("legacy");
        let overlay = td.path().join("agent").join(".fletch-codex-home");
        legacy_rollout(&legacy, "09-00-00", "main-1", None);
        // The sub-agent ran a day later, so its copy needs a dir of its own,
        // which the first launch can't create.
        let child_day = legacy.join("2026/10/02");
        std::fs::create_dir_all(&child_day).unwrap();
        std::fs::write(
            child_day.join("rollout-2026-10-02T09-05-00-child-1.jsonl"),
            format!("{}\n", meta_line("child-1", Some("main-1"))),
        )
        .unwrap();
        let month = overlay.join("sessions/2026/10");
        std::fs::create_dir_all(month.join("01")).unwrap();
        std::fs::set_permissions(&month, std::fs::Permissions::from_mode(0o555)).unwrap();
        adopt_rollouts_from("main-1", &overlay, std::slice::from_ref(&legacy)).unwrap();
        std::fs::set_permissions(&month, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!month.join("02").exists());
        assert!(!overlay.join(ADOPTED_DIRNAME).join("main-1").exists());

        adopt_rollouts_from("main-1", &overlay, &[legacy]).unwrap();

        assert_eq!(
            names_in(&month.join("02")),
            vec!["rollout-2026-10-02T09-05-00-child-1.jsonl"]
        );
        assert!(overlay.join(ADOPTED_DIRNAME).join("main-1").is_file());
    }

    /// Once a thread is confirmed whole, later launches don't walk the legacy
    /// roots for it: a file deleted from the overlay afterwards stays gone.
    #[test]
    fn a_confirmed_adoption_skips_the_legacy_walk() {
        let td = tempfile::tempdir().unwrap();
        let legacy = td.path().join("legacy");
        let overlay = td.path().join("agent").join(".fletch-codex-home");
        legacy_rollout(&legacy, "09-00-00", "main-1", None);
        legacy_rollout(&legacy, "09-05-00", "child-1", Some("main-1"));
        adopt_rollouts_from("main-1", &overlay, std::slice::from_ref(&legacy)).unwrap();
        assert!(overlay.join(ADOPTED_DIRNAME).join("main-1").is_file());
        let day = overlay.join("sessions/2026/10/01");
        std::fs::remove_file(day.join("rollout-2026-10-01T09-05-00-child-1.jsonl")).unwrap();

        adopt_rollouts_from("main-1", &overlay, &[legacy]).unwrap();

        assert_eq!(
            names_in(&day),
            vec!["rollout-2026-10-01T09-00-00-main-1.jsonl"]
        );
    }

    #[test]
    fn a_written_thread_is_where_codex_resumes_it_with_only_its_identity_changed() {
        let td = tempfile::tempdir().unwrap();
        let sessions = td.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let cwd = Path::new("/Users/u/.fletch/workspaces/new/repo");
        let lines = rollout_lines();

        let path = codex_write_in(&PrivateDir::open(&sessions).unwrap(), NEW, cwd, &lines).unwrap();

        // Found by the id ending its name, in the `YYYY/MM/DD` tree.
        let mut diag = ReadDiagnostics::default();
        let located = crate::transcripts::find_codex_rollouts_in(&sessions, NEW, &mut diag);
        assert_eq!(located, std::slice::from_ref(&path));
        let read = codex_read(&located, &mut diag);
        assert_eq!(read.len(), lines.len());
        for (back, line) in read.iter().zip(&lines) {
            let mut expected = line.clone();
            if line["type"] == "session_meta" {
                expected["payload"]["id"] = json!(NEW);
                expected["payload"]["session_id"] = json!(NEW);
                expected["payload"]["cwd"] = json!(cwd.to_string_lossy());
            }
            assert_eq!(back.body, expected);
        }
        // What was the old thread's per-turn context stays as it was.
        assert_eq!(read[4].body["payload"]["cwd"], "/tmp/codex-spike");
        // The new thread is the rollout's own: none of the old one's
        // sub-agents are taken for its.
        assert!(codex_subagent_files(&path).is_empty());
        // And every turn resumes it.
        let turn = codex_build_args(&TurnArgs {
            prompt: "go on",
            session_id: Some(NEW),
            ..Default::default()
        });
        assert_eq!(turn[..3], ["exec", "resume", NEW]);
        assert!(
            codex_pty_args(Some(NEW), None, None, &[]).ends_with(&["resume".into(), NEW.into()])
        );
    }
}
