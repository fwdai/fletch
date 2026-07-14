//! Forking a conversation into a new workspace.
//!
//! A fork is a normal [`Supervisor::spawn_agent`] whose freshly-created session
//! is seeded along two *independent* axes — so nothing about the runtime,
//! sandbox, worktree, streaming, or chat rendering changes; the existing
//! machinery is handed a pre-seeded session instead of an empty one:
//!
//!  - **Code** ([`ForkCode`]) — what the new worktree starts from. Today only
//!    `Clean` (fork the parent's base branch, no uncommitted work carried);
//!    carrying the parent's working tree ("build on unmerged work") lands in a
//!    follow-up slice.
//!  - **Context** ([`ForkContext`]) — how much of the parent conversation the
//!    new session carries: nothing, the whole history, or everything up to a
//!    chosen message. Summarized context is a follow-up slice.
//!
//! When context is carried it is seeded two ways, both reusing existing paths:
//!  1. **Display** — the parent's `session_records` up to the cutoff are copied
//!     into the new session, so the chat renders the prior history with no new
//!     UI (it reduces exactly like any transcript).
//!  2. **Agent knowledge** — those same records are rendered into a text digest
//!     appended to the new agent's `instructions` (the standing brief, injected
//!     every spawn and never shown as a chat bubble). Provider-portable; no
//!     `--resume` / transcript-file synthesis.

use std::collections::HashMap;

use serde_json::Value;
use tauri::AppHandle;

use crate::error::{Error, Result};
use crate::workspace::{AgentRecord, SessionRecord, UserTurn};

use super::{SpawnRequest, Supervisor};

/// The panel's git-action trigger prefix. Git actions are sent as ordinary user
/// messages (so they create `session_user_turns` rows), but the chat's
/// navigable-turn list excludes them — so the fork's turn indexing must too, to
/// stay aligned with the ordinal the frontend passes. Mirrors the frontend
/// `APP_ACTION_PREFIX` (see `src/components/RightPanel/delegation.ts`).
const APP_ACTION_PREFIX: &str = "[app-action] ";

/// Delimiters bracketing the injected prior-conversation digest inside the
/// agent's brief. Kept stable so forking a fork strips the parent's injected
/// block (rebuilt fresh from records) instead of nesting digests unboundedly.
const FORK_CONTEXT_OPEN: &str = "<forked-conversation-context>";
const FORK_CONTEXT_CLOSE: &str = "</forked-conversation-context>";

/// What the forked workspace's worktree starts from. Defined as an enum (rather
/// than a bool) so `Carry` — bring the parent's current working tree, incl.
/// uncommitted work — slots in as a variant in its own slice without changing
/// the command shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkCode {
    /// A fresh worktree from the parent's base branch — no uncommitted work.
    Clean,
}

/// How much of the parent conversation the fork carries. `Summary` (a
/// summarized digest rather than verbatim) is a follow-up variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ForkContext {
    /// Fresh conversation — carry nothing (the agent's brief is preserved).
    None,
    /// Carry the entire parent conversation.
    Full,
    /// Carry the conversation up to (and including) the navigable user prompt at
    /// this 0-based ordinal — git-action turns excluded, matching the chat's
    /// turn list.
    UpToMessage { prompt: usize },
}

impl Supervisor {
    /// Fork `parent_id` into a brand-new workspace, seeding its worktree
    /// (`code`) and conversation (`context`) independently.
    ///
    /// Returns the new agent record. Heavy provisioning runs in the background
    /// exactly like a normal spawn; any carried history and injected brief are
    /// in place before this returns, so the frontend can open the new agent and
    /// load its transcript immediately.
    pub async fn fork_agent(
        self: std::sync::Arc<Self>,
        app: AppHandle,
        parent_id: &str,
        code: ForkCode,
        context: ForkContext,
    ) -> Result<AgentRecord> {
        let parent = self.workspace.agent(parent_id)?;
        let primary = parent
            .repos
            .first()
            .ok_or_else(|| Error::Other("parent agent has no tracked repos".into()))?
            .clone();

        // Resolve which of the parent's records the fork carries (if any).
        let records = self.workspace.read_session_records(parent_id)?;
        let cutoff = match context {
            ForkContext::None => None,
            ForkContext::Full => Some(i64::MAX),
            ForkContext::UpToMessage { prompt } => {
                let turns = self.workspace.read_user_turns(parent_id)?;
                Some(fork_cutoff_seq(&records, &turns, prompt)?)
            }
        };
        let carried: Vec<&SessionRecord> = match cutoff {
            Some(c) => records.iter().filter(|r| r.seq < c).collect(),
            None => Vec::new(),
        };

        // Brief: always start from the parent's brief with any *prior* fork
        // digest stripped (so a fork of a fork carries one digest rebuilt from
        // records, not a growing stack). Append a fresh digest only when
        // carrying history.
        let base_brief = strip_forked_context(parent.instructions.as_deref().unwrap_or(""));
        let instructions = if carried.is_empty() {
            (!base_brief.is_empty()).then_some(base_brief)
        } else {
            let digest = build_digest(&parent.provider, &carried);
            Some(combine_instructions(base_brief, &digest))
        };

        // Code: reuse the normal spawn/provision path. `Clean` forks the parent's
        // own base branch, so the fork starts where the parent did.
        let fork_base = match code {
            ForkCode::Clean => primary.parent_branch.clone(),
        };

        let req = SpawnRequest {
            view: parent.view,
            repo_path: primary.repo_path.clone(),
            provider: parent.provider.clone(),
            name: None,
            effort: parent.effort.clone(),
            model: parent.model.clone(),
            instructions,
            custom_agent_id: parent.custom_agent_id.clone(),
            skills: parent.skills.clone(),
            mcp_servers: parent.mcp_servers.clone(),
            fork_base,
            run_repo: None,
            owner_run_id: None,
        };
        let child = self.clone().spawn_agent(app, req).await?;

        if !carried.is_empty() {
            // Display continuity: copy the parent's records into the fork's
            // session. Their native_ids are the parent's; the fork's own future
            // records carry different ids, so ingestion appends after these
            // without collision.
            let pairs: Vec<(&str, &Value)> = carried
                .iter()
                .map(|r| (r.native_id.as_str(), &r.body))
                .collect();
            self.workspace.append_session_records(
                &child.id,
                &parent.provider,
                "transcript",
                None,
                &pairs,
            )?;

            // ChatView only loads a transcript for an agent whose task is
            // non-empty (its "has prior conversation" gate). A fresh spawn leaves
            // task empty, so stamp the parent's topic here to unlock the load.
            let topic = first_nonempty(&[parent.task.trim(), "Forked conversation"]);
            let _ = self.workspace.set_agent_task_if_empty(&child.id, topic);
        }

        // Re-read so the returned record reflects the stamped task.
        self.workspace.agent(&child.id)
    }
}

/// Resolve the `session_records.seq` an `UpToMessage` fork copies *up to but not
/// including*.
///
/// `up_to_prompt` is the 0-based ordinal of a navigable user prompt (git actions
/// excluded). We find that prompt among the non-app-action user turns, then take
/// the record seq of the *next* user turn of any kind (its `native_id` mapped
/// through `records`); everything strictly before that is the forked turn's
/// content plus all prior history. With no resolvable next turn (forking the
/// latest prompt, or a pending/unmatched next turn), the cutoff is `i64::MAX` —
/// copy everything.
fn fork_cutoff_seq(
    records: &[SessionRecord],
    turns: &[UserTurn],
    up_to_prompt: usize,
) -> Result<i64> {
    let real: Vec<&UserTurn> = turns
        .iter()
        .filter(|t| !t.text.starts_with(APP_ACTION_PREFIX))
        .collect();
    if real.is_empty() {
        return Err(Error::Other("parent has no forkable prompts".into()));
    }
    // Clamp rather than error: an ordinal past the end forks the whole thing.
    let idx = up_to_prompt.min(real.len() - 1);
    let boundary_seq = real[idx].seq;

    let native_to_seq: HashMap<&str, i64> =
        records.iter().map(|r| (r.native_id.as_str(), r.seq)).collect();

    let cutoff = turns
        .iter()
        .filter(|t| t.seq > boundary_seq)
        .filter_map(|t| t.native_id.as_deref())
        .filter_map(|nid| native_to_seq.get(nid).copied())
        .min()
        .unwrap_or(i64::MAX);
    Ok(cutoff)
}

/// Render the carried records into a plain-text digest wrapped in the fork
/// delimiters. Text turns only (tool calls/results are omitted to keep the brief
/// tight).
fn build_digest(_provider: &str, records: &[&SessionRecord]) -> String {
    let mut blocks: Vec<String> = Vec::new();
    for r in records {
        if let Some((role, text)) = extract_message(&r.body) {
            blocks.push(format!("{role}: {text}"));
        }
    }
    let body = blocks.join("\n\n");
    format!(
        "{FORK_CONTEXT_OPEN}\n\
         The conversation below is the prior context this session was forked from. \
         Treat it as already-established history and continue from where it left off; \
         do not redo work that is already complete.\n\n\
         {body}\n\
         {FORK_CONTEXT_CLOSE}"
    )
}

/// Pull a `(Role, text)` pair out of one transcript record body, or `None` for
/// records that carry no user/assistant prose (tool-only turns, summaries,
/// system events). Handles the Claude-family shape (`type` + `message.content`
/// as a string or an array of typed blocks); other providers that share it work
/// too, and those that don't simply contribute nothing to the digest.
fn extract_message(body: &Value) -> Option<(&'static str, String)> {
    let role = match body.get("type").and_then(Value::as_str)? {
        "user" => "User",
        "assistant" => "Assistant",
        _ => return None,
    };
    let content = body.get("message")?.get("content")?;
    let text = match content {
        Value::String(s) => s.trim().to_string(),
        Value::Array(blocks) => {
            let mut parts: Vec<&str> = Vec::new();
            for b in blocks {
                if b.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        parts.push(t);
                    }
                }
            }
            parts.join("\n").trim().to_string()
        }
        _ => String::new(),
    };
    if text.is_empty() {
        return None;
    }
    Some((role, text))
}

/// Remove any previously-injected fork digest block(s) from a brief, so a fork
/// of a fork carries a single digest rebuilt from records rather than a growing
/// stack of nested ones.
fn strip_forked_context(brief: &str) -> String {
    let mut out = brief.to_string();
    while let Some(start) = out.find(FORK_CONTEXT_OPEN) {
        let Some(rel_end) = out[start..].find(FORK_CONTEXT_CLOSE) else {
            break;
        };
        let end = start + rel_end + FORK_CONTEXT_CLOSE.len();
        out.replace_range(start..end, "");
    }
    out.trim().to_string()
}

/// Join the parent's (digest-stripped) brief with the fresh digest, or use the
/// digest alone when there was no brief.
fn combine_instructions(base_brief: String, digest: &str) -> String {
    if base_brief.is_empty() {
        digest.to_string()
    } else {
        format!("{base_brief}\n\n{digest}")
    }
}

/// First non-empty candidate, falling back to the last.
fn first_nonempty<'a>(candidates: &[&'a str]) -> &'a str {
    candidates
        .iter()
        .copied()
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| candidates.last().copied().unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(seq: i64, native_id: &str, body: Value) -> SessionRecord {
        SessionRecord {
            seq,
            provider: "claude".into(),
            source: "transcript".into(),
            native_id: native_id.into(),
            agent_version: None,
            body,
        }
    }

    fn turn(seq: i64, text: &str, native_id: Option<&str>) -> UserTurn {
        UserTurn {
            turn_id: format!("t{seq}"),
            seq,
            text: text.into(),
            attachments: vec![],
            native_id: native_id.map(str::to_string),
            started_at: None,
            ended_at: None,
        }
    }

    fn user(text: &str) -> Value {
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]}})
    }
    fn assistant(text: &str) -> Value {
        json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}})
    }

    // records: u1(seq1) a1(seq2) u2(seq3) a2(seq4) u3(seq5) a3(seq6)
    fn sample_records() -> Vec<SessionRecord> {
        vec![
            rec(1, "u1", user("first")),
            rec(2, "a1", assistant("resp1")),
            rec(3, "u2", user("second")),
            rec(4, "a2", assistant("resp2")),
            rec(5, "u3", user("third")),
            rec(6, "a3", assistant("resp3")),
        ]
    }
    fn sample_turns() -> Vec<UserTurn> {
        vec![
            turn(1, "first", Some("u1")),
            turn(2, "second", Some("u2")),
            turn(3, "third", Some("u3")),
        ]
    }

    #[test]
    fn cutoff_at_first_prompt_carries_only_first_turn() {
        // Fork at prompt 0 → next turn is u2 (seq 3), so copy seq < 3 = [u1, a1].
        let seq = fork_cutoff_seq(&sample_records(), &sample_turns(), 0).unwrap();
        assert_eq!(seq, 3);
    }

    #[test]
    fn cutoff_at_middle_prompt_carries_through_that_turn() {
        // Fork at prompt 1 → next turn is u3 (seq 5), so copy seq < 5.
        let seq = fork_cutoff_seq(&sample_records(), &sample_turns(), 1).unwrap();
        assert_eq!(seq, 5);
    }

    #[test]
    fn cutoff_at_last_prompt_carries_everything() {
        let seq = fork_cutoff_seq(&sample_records(), &sample_turns(), 2).unwrap();
        assert_eq!(seq, i64::MAX);
    }

    #[test]
    fn out_of_range_prompt_clamps_to_last() {
        let seq = fork_cutoff_seq(&sample_records(), &sample_turns(), 99).unwrap();
        assert_eq!(seq, i64::MAX);
    }

    #[test]
    fn app_action_turns_are_excluded_from_prompt_indexing() {
        // A git-action turn sits between prompt 0 and prompt 1. It must not shift
        // the navigable ordinal: prompt 1 is still "second".
        let mut records = sample_records();
        records.insert(2, rec(0, "ga", user("[app-action] commit")));
        for (i, r) in records.iter_mut().enumerate() {
            r.seq = i as i64 + 1;
        }
        // turns: first(seq1,u1), [app-action](seq2,ga), second(seq3,u2), third(seq4,u3)
        let turns = vec![
            turn(1, "first", Some("u1")),
            turn(2, "[app-action] commit base=\"main\"", Some("ga")),
            turn(3, "second", Some("u2")),
            turn(4, "third", Some("u3")),
        ];
        // Fork at navigable prompt 0 ("first"): next turn is the app-action turn,
        // whose native_id "ga" is at re-sequenced record seq 3 → cutoff 3.
        let seq = fork_cutoff_seq(&records, &turns, 0).unwrap();
        assert_eq!(seq, 3);
    }

    #[test]
    fn digest_wraps_text_turns_and_labels_roles() {
        let records = sample_records();
        let refs: Vec<&SessionRecord> = records.iter().take(2).collect();
        let d = build_digest("claude", &refs);
        assert!(d.starts_with(FORK_CONTEXT_OPEN));
        assert!(d.trim_end().ends_with(FORK_CONTEXT_CLOSE));
        assert!(d.contains("User: first"));
        assert!(d.contains("Assistant: resp1"));
    }

    #[test]
    fn strip_removes_prior_injected_block() {
        let brief = format!("real brief\n\n{FORK_CONTEXT_OPEN}\nold stuff\n{FORK_CONTEXT_CLOSE}");
        assert_eq!(strip_forked_context(&brief), "real brief");
    }

    #[test]
    fn strip_is_noop_without_a_block() {
        assert_eq!(strip_forked_context("just a brief"), "just a brief");
    }

    #[test]
    fn combine_uses_digest_alone_when_no_brief() {
        assert_eq!(combine_instructions(String::new(), "DIGEST"), "DIGEST");
        assert_eq!(combine_instructions("B".into(), "D"), "B\n\nD");
    }

    #[test]
    fn extract_handles_string_content() {
        let body = json!({"type": "user", "message": {"content": "hello"}});
        assert_eq!(extract_message(&body), Some(("User", "hello".to_string())));
    }

    #[test]
    fn extract_skips_tool_only_and_unknown() {
        let tool_only = json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Bash"}]}});
        assert_eq!(extract_message(&tool_only), None);
        let sys = json!({"type": "summary", "message": {"content": "x"}});
        assert_eq!(extract_message(&sys), None);
    }

    #[test]
    fn context_deserializes_from_tagged_json() {
        let none: ForkContext = serde_json::from_value(json!({"kind": "none"})).unwrap();
        assert_eq!(none, ForkContext::None);
        let full: ForkContext = serde_json::from_value(json!({"kind": "full"})).unwrap();
        assert_eq!(full, ForkContext::Full);
        let upto: ForkContext =
            serde_json::from_value(json!({"kind": "up_to_message", "prompt": 3})).unwrap();
        assert_eq!(upto, ForkContext::UpToMessage { prompt: 3 });
        let code: ForkCode = serde_json::from_value(json!("clean")).unwrap();
        assert_eq!(code, ForkCode::Clean);
    }
}
