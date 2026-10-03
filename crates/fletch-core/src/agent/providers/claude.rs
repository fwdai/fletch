//! Claude transcript reader and writer, and one-shot args.
//!
//! Claude is the lone persistent-runner agent (not in PER_TURN_AGENTS), launched
//! `--session-id <uuid>` / `--resume <uuid>`, so it writes
//! `<config-dir>/projects/<slug>/<uuid>.jsonl` (config-dir = CLAUDE_CONFIG_DIR or
//! `~/.claude`). find_session_jsonl locates it, honoring the Docker sandbox's
//! per-agent transcript dir (derived from `cwd`). Content lines carry a top-level
//! `uuid`; metadata lines (mode/permission-mode/…) don't → positional fallback.
//! Nearly every line also names its session (`sessionId`), and message lines
//! the directory claude ran in (`cwd`).
//!
//! A sub-agent (Task/Agent tool) never writes into that file. Each one gets
//! `<config-dir>/projects/<slug>/<uuid>/subagents/agent-<agentId>.jsonl` — a
//! directory named after the session, beside the session's own file — whose
//! records look like main ones (`type` user/assistant, `uuid`, `parentUuid`,
//! `timestamp`, `message`) plus `isSidechain: true`, `agentId`, and the parent's
//! `sessionId`. The usage scan walks these for spend; the sync ingests them via
//! [`CLAUDE_SUBAGENTS`] so the nested thread survives a reload.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::args::model_args;
use crate::agent::transcript::{
    records_with_id, replace_field, write_new_jsonl, JsonlTail, RawRecord, ReadDiagnostics,
    SubagentLayout, TranscriptReader,
};
use crate::error::{Error, Result};

fn claude_locate(session_id: &str, cwd: &Path, diag: &mut ReadDiagnostics) -> Vec<PathBuf> {
    crate::transcripts::find_session_jsonl(session_id, cwd, diag)
        .into_iter()
        .collect()
}

fn claude_read(paths: &[PathBuf], diag: &mut ReadDiagnostics) -> Vec<RawRecord> {
    let values: Vec<Value> = paths
        .iter()
        .flat_map(|p| crate::transcripts::read_jsonl_values(p, diag))
        .collect();
    records_with_id(values, Some("uuid"))
}

/// `<session-id>/subagents/agent-<id>.jsonl` files beside the main transcript
/// `<session-id>.jsonl`, sorted by path. A missing dir (no sub-agent spawned
/// yet) is simply empty; anything not matching the name pattern is skipped.
fn claude_subagent_files(main: &Path) -> Vec<(String, PathBuf)> {
    let dir = main.with_extension("").join("subagents");
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let id = path
                .file_name()?
                .to_str()?
                .strip_prefix("agent-")?
                .strip_suffix(".jsonl")?
                .to_string();
            Some((id, path))
        })
        .collect();
    out.sort();
    out
}

/// How the main transcript names the sub-agent a tool call spawned, verified
/// on real transcripts (claude 2.1.x). The persisted `user` record that carries
/// the Agent tool's `tool_result` block also carries a `toolUseResult` object
/// with `agentId: "<id>"`, for both kinds of sub-agent:
///
/// - foreground: `toolUseResult.status == "completed"`, written when the agent
///   finishes; the block's content is the agent's final report and never names
///   the id, so this object is the only link. The sub-agent's records are
///   therefore linkable only once it has finished — until then its file is
///   skipped and picked up by a later pass.
/// - background: `toolUseResult.status == "async_launched"`, written at launch.
///   The block's text does say `agentId: <id>`, and a later `<task-notification>`
///   user turn repeats it with the tool-use id, but both are redundant with the
///   object, so only that is read.
///
/// The spawning tool_use id is the block's `tool_use_id`. Nothing else in either
/// file links the two: the sub-agent's records carry only the session id and
/// their own `agentId`, and the `tool_use` input holds no id yet. One record
/// names one agent id, so the first match across `bodies` wins.
fn claude_subagent_parent(bodies: &[Value], id: &str) -> Option<String> {
    bodies.iter().find_map(|body| {
        if body
            .pointer("/toolUseResult/agentId")
            .and_then(Value::as_str)
            != Some(id)
        {
            return None;
        }
        body.pointer("/message/content")?
            .as_array()?
            .iter()
            .find(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))?
            .get("tool_use_id")?
            .as_str()
            .map(str::to_string)
    })
}

const CLAUDE_SUBAGENTS: SubagentLayout = SubagentLayout {
    files: claude_subagent_files,
    needle: None, // the agent id is verbatim in the linking record
    parent_tool_use: claude_subagent_parent,
    id_field: Some("uuid"),
};

pub(crate) static CLAUDE_TRANSCRIPT: TranscriptReader = TranscriptReader {
    locate: claude_locate,
    read: claude_read,
    tail: Some(JsonlTail {
        id_field: Some("uuid"),
    }), // single persistent jsonl
    subagents: Some(CLAUDE_SUBAGENTS),
    write: Some(claude_write),
};

/// Write `bodies` as claude session `session_id`, run in `cwd`: the file
/// `--resume <session_id>` opens, `<projects>/<cwd as a dirname>/<id>.jsonl`
/// in the projects dir claude uses there (the per-agent one in a container,
/// [`crate::transcripts::claude_projects_dir`]). Each line's `sessionId` and
/// `cwd`, where it has them, name the new session; the rest, `uuid` and
/// `parentUuid` chain included, is copied as is, so claude resumes the same
/// conversation, compactions and all.
///
/// A claude session is resumable once it holds a message (a line with a
/// `uuid`, see [`claude_session_has_messages`]); without one nothing is
/// written, since a file claude can't resume also stops `--session-id` from
/// starting the session fresh ("already in use").
fn claude_write(
    session_id: &str,
    cwd: &Path,
    container: bool,
    bodies: &[Value],
) -> Result<Option<PathBuf>> {
    if !bodies.iter().any(|b| b.get("uuid").is_some()) {
        return Ok(None);
    }
    let projects = crate::transcripts::claude_projects_dir(cwd, container)
        .ok_or_else(|| Error::Other("claude's projects directory can't be resolved".into()))?;
    // Claude goes by its working directory as the OS reports it, symlinks
    // resolved; in a container, that is the path the checkout is mounted at.
    let run_dir = if container {
        cwd.to_path_buf()
    } else {
        std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf())
    };
    let dirname = crate::transcripts::claude_project_dirname(&run_dir).ok_or_else(|| {
        Error::Other(format!(
            "{} is too long a path to name claude's session directory for",
            run_dir.display()
        ))
    })?;
    let run_dir_text = run_dir.to_string_lossy();
    let lines: Vec<Value> = bodies
        .iter()
        .map(|body| {
            let mut line = body.clone();
            replace_field(&mut line, "sessionId", session_id);
            replace_field(&mut line, "cwd", &run_dir_text);
            line
        })
        .collect();
    let path = projects.join(dirname).join(format!("{session_id}.jsonl"));
    write_new_jsonl(&path, &lines)?;
    Ok(Some(path))
}

/// Whether claude has written a message into `session_id`'s transcript, i.e.
/// whether the session can be `--resume`d. Claude creates the file with its
/// first message, and only message lines carry a `uuid` (metadata lines such
/// as `mode` don't). Every claude launch asks, so it reads only up to the
/// first message, not a long session's whole transcript.
pub(crate) fn claude_session_has_messages(session_id: &str, cwd: &Path) -> bool {
    use std::io::BufRead;
    let mut diag = ReadDiagnostics::default();
    let Some(path) = crate::transcripts::find_session_jsonl(session_id, cwd, &mut diag) else {
        return false;
    };
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(std::result::Result::ok)
        .any(|line| serde_json::from_str::<Value>(&line).is_ok_and(|v| v.get("uuid").is_some()))
}

/// Claude as a one-shot completion (`OneShot`), per `claude --help` (2.1.287):
/// print mode with a plain-text answer, the prompt read from stdin. `--tools ""`
/// turns every built-in tool off, and `--strict-mcp-config` with no
/// `--mcp-config` loads no MCP server, so the run can only answer.
/// `--disable-slash-commands` keeps the input from invoking a skill, and
/// `--no-session-persistence` leaves no session behind.
pub(crate) fn claude_one_shot_args(model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "text",
        "--tools",
        "",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--no-session-persistence",
    ]
    .map(String::from)
    .into();
    args.extend(model_args(model));
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subagent_files_are_found_beside_the_main_transcript_in_path_order() {
        let td = tempfile::tempdir().unwrap();
        let main = td.path().join("slug").join("sess-1.jsonl");
        let nested = td.path().join("slug").join("sess-1").join("subagents");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(&main, "{}\n").unwrap();
        std::fs::write(nested.join("agent-b2.jsonl"), "{}\n").unwrap();
        std::fs::write(nested.join("agent-a1.jsonl"), "{}\n").unwrap();
        // Not a sub-agent transcript: wrong prefix / extension.
        std::fs::write(nested.join("notes.jsonl"), "{}\n").unwrap();
        std::fs::write(nested.join("agent-c3.txt"), "").unwrap();

        let files = claude_subagent_files(&main);
        assert_eq!(
            files,
            vec![
                ("a1".to_string(), nested.join("agent-a1.jsonl")),
                ("b2".to_string(), nested.join("agent-b2.jsonl")),
            ]
        );
    }

    #[test]
    fn subagent_files_is_empty_when_the_session_spawned_none() {
        let td = tempfile::tempdir().unwrap();
        let main = td.path().join("sess-1.jsonl");
        assert!(claude_subagent_files(&main).is_empty());
    }

    /// The persisted parent record, in the shape real transcripts use.
    fn parent_record(status: &str, agent_id: &str, tool_use_id: &str) -> Value {
        json!({
            "type": "user",
            "uuid": "p-1",
            "toolUseResult": { "status": status, "agentId": agent_id, "prompt": "look" },
            "message": {
                "role": "user",
                "content": [{ "type": "tool_result", "tool_use_id": tool_use_id, "content": "…" }]
            }
        })
    }

    #[test]
    fn subagent_parent_links_foreground_and_background_records() {
        for status in ["completed", "async_launched"] {
            assert_eq!(
                claude_subagent_parent(&[parent_record(status, "abc", "toolu_1")], "abc")
                    .as_deref(),
                Some("toolu_1"),
                "{status}"
            );
        }
        // The linking record is found wherever it sits in the batch.
        let bodies = [
            json!({ "type": "assistant", "message": {} }),
            parent_record("completed", "abc", "toolu_2"),
        ];
        assert_eq!(
            claude_subagent_parent(&bodies, "abc").as_deref(),
            Some("toolu_2")
        );
    }

    #[test]
    fn subagent_parent_ignores_other_records() {
        // Another sub-agent's result.
        assert_eq!(
            claude_subagent_parent(&[parent_record("completed", "other", "toolu_1")], "abc"),
            None
        );
        // The background task notification names the id in text only — no
        // `toolUseResult` — and the sub-agent's own records name themselves.
        let notification = json!({
            "type": "user",
            "message": { "role": "user", "content": "<task-notification><task-id>abc</task-id></task-notification>" }
        });
        let own =
            json!({ "type": "assistant", "isSidechain": true, "agentId": "abc", "message": {} });
        assert_eq!(claude_subagent_parent(&[notification, own], "abc"), None);
    }

    // ── writing a session ─────────────────────────────────────────────────

    const NEW: &str = "0b9d7c1e-5f3a-4c2b-9e8d-7a6b5c4d3e2f";

    /// A real session's lines (the shared usage fixture: messages, a
    /// sidechain line, a compaction), opened by the metadata line claude
    /// writes first and with the directory it ran in on its messages.
    fn session_lines() -> Vec<Value> {
        let mut lines: Vec<Value> =
            include_str!("../../../../../tests/fixtures/usage/claude.jsonl")
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        for line in &mut lines {
            line["cwd"] = json!("/Users/u/.fletch/workspaces/old/repo");
        }
        let session = lines[0]["sessionId"].clone();
        lines.insert(
            0,
            json!({ "type": "mode", "mode": "normal", "sessionId": session }),
        );
        lines
    }

    /// An agent's checkout in a container sandbox, whose projects dir is the
    /// per-agent one beside it, under `td`.
    fn container_cwd(td: &Path) -> PathBuf {
        td.join("ws").join("repo")
    }

    #[test]
    fn a_written_session_is_where_claude_resumes_it_with_only_its_identity_changed() {
        let td = tempfile::tempdir().unwrap();
        let cwd = container_cwd(td.path());
        let lines = session_lines();

        let path = claude_write(NEW, &cwd, true, &lines).unwrap().unwrap();

        // Claude's own dir for the new checkout, under the container's mount.
        assert_eq!(
            path,
            td.path()
                .join("ws")
                .join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME)
                .join(crate::transcripts::claude_project_dirname(&cwd).unwrap())
                .join(format!("{NEW}.jsonl"))
        );
        let mut diag = ReadDiagnostics::default();
        let located = (CLAUDE_TRANSCRIPT.locate)(NEW, &cwd, &mut diag);
        assert_eq!(located, [path]);
        let read = (CLAUDE_TRANSCRIPT.read)(&located, &mut diag);
        assert_eq!(read.len(), lines.len());
        for (back, line) in read.iter().zip(&lines) {
            let mut expected = line.clone();
            expected["sessionId"] = json!(NEW);
            if line.get("cwd").is_some() {
                expected["cwd"] = json!(cwd.to_string_lossy());
            }
            assert_eq!(back.body, expected);
        }
        // The same records under the same ids, so the chain is intact.
        let ids = |records: &[RawRecord]| -> Vec<String> {
            records.iter().map(|r| r.native_id.clone()).collect()
        };
        let original = records_with_id(lines, Some("uuid"));
        assert_eq!(ids(&read), ids(&original));
        // And a launch resumes it rather than starting it fresh.
        assert!(claude_session_has_messages(NEW, &cwd));
    }

    #[test]
    fn a_history_claude_cant_resume_writes_nothing() {
        let td = tempfile::tempdir().unwrap();
        let cwd = container_cwd(td.path());
        let metadata = [json!({ "type": "mode", "mode": "normal", "sessionId": "old" })];

        assert_eq!(claude_write(NEW, &cwd, true, &metadata).unwrap(), None);
        assert_eq!(claude_write(NEW, &cwd, true, &[]).unwrap(), None);
        assert!(!claude_session_has_messages(NEW, &cwd));
    }

    #[test]
    fn a_session_is_never_written_over() {
        let td = tempfile::tempdir().unwrap();
        let cwd = container_cwd(td.path());
        let lines = session_lines();
        let path = claude_write(NEW, &cwd, true, &lines).unwrap().unwrap();
        let before = std::fs::read(&path).unwrap();

        assert!(claude_write(NEW, &cwd, true, &lines[..2]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn claude_names_a_session_dir_after_the_whole_path() {
        let dirname = |p: &str| crate::transcripts::claude_project_dirname(Path::new(p));
        assert_eq!(
            dirname("/Users/alex/.fletch/workspaces/bromo/quorum").as_deref(),
            Some("-Users-alex--fletch-workspaces-bromo-quorum")
        );
        assert_eq!(dirname("/a_b/c d").as_deref(), Some("-a-b-c-d"));
        assert_eq!(
            dirname(&format!("/{}", "x".repeat(199))).unwrap().len(),
            200
        );
        assert_eq!(dirname(&format!("/{}", "x".repeat(200))), None);
    }
}
