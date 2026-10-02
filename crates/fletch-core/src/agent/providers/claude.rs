//! Claude transcript reader, branch point and one-shot args.
//!
//! Claude is the lone persistent-runner agent (not in PER_TURN_AGENTS), launched
//! `--session-id <uuid>` / `--resume <uuid>`, so it writes
//! `<config-dir>/projects/<slug>/<uuid>.jsonl` (config-dir = CLAUDE_CONFIG_DIR or
//! `~/.claude`). find_session_jsonl locates it, honoring the Docker sandbox's
//! per-agent transcript dir (derived from `cwd`). Content lines carry a top-level
//! `uuid`; metadata lines (mode/permission-mode/…) don't → positional fallback.
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
    records_with_id, JsonlTail, RawRecord, ReadDiagnostics, SubagentLayout, TranscriptReader,
};
use crate::agent::BranchPoint;
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
};

/// The branch of `from_session` that rewinds to just before the user prompt
/// whose `uuid` is `prompt_id`. `bodies` are that session's records in
/// transcript order.
///
/// `--resume-session-at <id>` keeps the resumed conversation "up to and
/// including the chain entry with <id>": claude (verified on 2.1.287) finds
/// the loaded message whose `uuid` is `<id>`, drops everything after it, and
/// exits with "No message found with message.uuid of: <id>" if there's none.
/// Any chain entry is accepted, not only user/assistant ones. So the cut is the
/// entry the prompt was appended to, its `parentUuid` — usually the `system`
/// entry that closed the previous turn (`stop_hook_summary`, `turn_duration`),
/// sometimes an assistant message or an attachment.
///
/// - `Ok(None)`: the prompt opens the session, so nothing is kept. Launch
///   `SessionStart::Fresh`; a `BranchPoint` without a message keeps *all*.
/// - `Err`: the prompt isn't in `bodies`, or the session was compacted after
///   it. A resume loads only the chain since the last compaction, so a cut
///   before one has nothing to resume at.
pub fn claude_branch_before<'a>(
    from_session: &str,
    bodies: impl IntoIterator<Item = &'a Value>,
    prompt_id: &str,
) -> Result<Option<BranchPoint>> {
    let mut bodies = bodies.into_iter();
    let prompt = bodies
        .by_ref()
        .find(|b| b.get("uuid").and_then(Value::as_str) == Some(prompt_id))
        .ok_or_else(|| Error::Other(format!("message {prompt_id} isn't in the session")))?;
    if bodies.any(is_compact_boundary) {
        return Err(Error::Other(
            "the conversation was compacted after this message".into(),
        ));
    }
    Ok(prompt
        .get("parentUuid")
        .and_then(Value::as_str)
        .map(|parent| BranchPoint {
            from_session: from_session.to_string(),
            at_message: Some(parent.to_string()),
        }))
}

fn is_compact_boundary(body: &Value) -> bool {
    body.get("type").and_then(Value::as_str) == Some("system")
        && body.get("subtype").and_then(Value::as_str) == Some("compact_boundary")
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

    /// Two turns in the shape real transcripts use: a metadata line (no uuid),
    /// then prompt → assistant → the `system` entry that closes the turn, which
    /// the next prompt is appended to.
    fn two_turns() -> Vec<Value> {
        vec![
            json!({ "type": "mode", "mode": "normal" }),
            json!({ "type": "user", "uuid": "p1", "parentUuid": null }),
            json!({ "type": "assistant", "uuid": "a1", "parentUuid": "p1" }),
            json!({ "type": "system", "subtype": "turn_duration", "uuid": "s1", "parentUuid": "a1" }),
            json!({ "type": "user", "uuid": "p2", "parentUuid": "s1" }),
            json!({ "type": "assistant", "uuid": "a2", "parentUuid": "p2" }),
        ]
    }

    #[test]
    fn branch_before_a_prompt_keeps_through_the_entry_it_follows() {
        let bodies = two_turns();
        assert_eq!(
            claude_branch_before("src", &bodies, "p2").unwrap(),
            Some(BranchPoint {
                from_session: "src".into(),
                at_message: Some("s1".into()),
            })
        );
    }

    #[test]
    fn branch_before_the_opening_prompt_keeps_nothing() {
        assert_eq!(
            claude_branch_before("src", &two_turns(), "p1").unwrap(),
            None
        );
    }

    #[test]
    fn branch_before_an_unknown_prompt_is_an_error() {
        assert!(claude_branch_before("src", &two_turns(), "nope").is_err());
    }

    #[test]
    fn branch_cannot_cut_before_a_compaction() {
        let boundary = json!({
            "type": "system", "subtype": "compact_boundary", "uuid": "c1", "parentUuid": null
        });
        let mut bodies = two_turns();
        bodies.push(boundary.clone());
        assert!(claude_branch_before("src", &bodies, "p2").is_err());

        // A compaction *before* the prompt is part of the resumed chain.
        let mut bodies = vec![boundary];
        bodies.extend(two_turns());
        assert!(claude_branch_before("src", &bodies, "p2")
            .unwrap()
            .is_some());
    }
}
