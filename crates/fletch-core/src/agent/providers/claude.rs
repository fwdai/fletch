//! Claude transcript reader.
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

use crate::agent::transcript::{
    records_with_id, JsonlTail, RawRecord, ReadDiagnostics, SubagentLayout, TranscriptReader,
};

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
/// their own `agentId`, and the `tool_use` input holds no id yet.
fn claude_subagent_parent(body: &Value, id: &str) -> Option<String> {
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
                claude_subagent_parent(&parent_record(status, "abc", "toolu_1"), "abc").as_deref(),
                Some("toolu_1"),
                "{status}"
            );
        }
    }

    #[test]
    fn subagent_parent_ignores_other_records() {
        // Another sub-agent's result.
        assert_eq!(
            claude_subagent_parent(&parent_record("completed", "other", "toolu_1"), "abc"),
            None
        );
        // The background task notification names the id in text only — no
        // `toolUseResult` — and the sub-agent's own records name themselves.
        let notification = json!({
            "type": "user",
            "message": { "role": "user", "content": "<task-notification><task-id>abc</task-id></task-notification>" }
        });
        assert_eq!(claude_subagent_parent(&notification, "abc"), None);
        let own =
            json!({ "type": "assistant", "isSidechain": true, "agentId": "abc", "message": {} });
        assert_eq!(claude_subagent_parent(&own, "abc"), None);
    }
}
