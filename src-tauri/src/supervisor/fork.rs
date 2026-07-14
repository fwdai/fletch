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
//!  2. **Agent knowledge** — a plain-text digest of that same range is appended
//!     to the new agent's `instructions` (the standing brief, injected every
//!     spawn and never shown as a chat bubble). Provider-portable; no `--resume`
//!     / transcript-file synthesis.
//!
//! The digest text is built by the **frontend** and passed in as
//! `context_digest`: the frontend has every provider's chat adapter, so it
//! renders prose uniformly across providers (Claude, Codex, OpenCode, Pi, …) and
//! the injected context always matches the history the child actually shows. The
//! backend only decides the record cutoff (for the display copy) and wraps the
//! digest into the brief.

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

/// Sentinels that visually bracket the injected prior-conversation digest inside
/// the composed prompt, so the agent can see where the carried context begins and
/// ends. HTML-comment form, namespaced to Fletch. These are purely presentational
/// now: the digest is persisted in its own `forked_context` session column (never
/// spliced into the user brief), so nothing ever parses these back out — a fork
/// simply drops the parent's `forked_context` and rebuilds a fresh one.
const FORK_CONTEXT_OPEN: &str = "<!-- fletch:forked-conversation-context -->";
const FORK_CONTEXT_CLOSE: &str = "<!-- /fletch:forked-conversation-context -->";

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

/// How much of the parent conversation the fork carries. Drives the record
/// cutoff for the display copy; the matching digest text is supplied separately
/// by the frontend. `Summary` (a summarized digest) is a follow-up variant.
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
    /// `context_digest` is the frontend-rendered prose for the carried range
    /// (empty/`None` when nothing is carried); it becomes the injected brief
    /// context. `context` independently drives which `session_records` are copied
    /// for display, so the two are built from the same cutoff and stay in step.
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
        context_digest: Option<String>,
        snapshot_max_seq: Option<i64>,
    ) -> Result<AgentRecord> {
        let parent = self.workspace.agent(parent_id)?;
        let primary = parent
            .repos
            .first()
            .ok_or_else(|| Error::Other("parent agent has no tracked repos".into()))?
            .clone();

        // Resolve which of the parent's records the fork carries for display.
        let records = self.workspace.read_session_records(parent_id)?;
        let cutoff = match context {
            ForkContext::None => None,
            ForkContext::Full => Some(i64::MAX),
            ForkContext::UpToMessage { prompt } => {
                let turns = self.workspace.read_user_turns(parent_id)?;
                Some(fork_cutoff_seq(&records, &turns, prompt)?)
            }
        };
        let carried = carried_records(&records, cutoff, snapshot_max_seq);

        // Brief and forked-conversation context are stored in *separate* session
        // columns and only composed at spawn (see `effective_instructions`). So
        // the parent's brief passes through verbatim — never scanned for an
        // injected block — and the fresh digest lands in its own field. A fork of
        // a fork therefore inherits only the pure brief and gets a freshly built
        // digest: no stacking, and no way for sentinel-looking brief text to be
        // mistaken for a machine block and stripped.
        let instructions = parent.instructions.clone();
        let forked_context = context_digest
            .filter(|d| !d.trim().is_empty())
            .map(|prose| wrap_context(&prose));

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
            forked_context,
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

/// Select the parent records a fork copies into the child for display: those
/// below the context `cutoff` (`None` = carry nothing) and within the snapshot
/// the frontend built its digest from (`snapshot_max_seq`).
///
/// The backend reads the parent's records after the frontend did, so a sync that
/// lands in between can append newer rows. Capping at `snapshot_max_seq` keeps
/// those out of the copy — otherwise the child would render turns the injected
/// brief never mentioned. A carried context with `snapshot_max_seq == None`
/// (the caller saw no records) copies nothing, matching the empty digest.
fn carried_records(
    records: &[SessionRecord],
    cutoff: Option<i64>,
    snapshot_max_seq: Option<i64>,
) -> Vec<&SessionRecord> {
    match cutoff {
        Some(c) => records
            .iter()
            .filter(|r| r.seq < c && snapshot_max_seq.is_some_and(|m| r.seq <= m))
            .collect(),
        None => Vec::new(),
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

    let native_to_seq: HashMap<&str, i64> = records
        .iter()
        .map(|r| (r.native_id.as_str(), r.seq))
        .collect();

    let cutoff = turns
        .iter()
        .filter(|t| t.seq > boundary_seq)
        .filter_map(|t| t.native_id.as_deref())
        .filter_map(|nid| native_to_seq.get(nid).copied())
        .min()
        .unwrap_or(i64::MAX);
    Ok(cutoff)
}

/// Wrap the frontend-supplied conversation prose in the fork sentinels plus a
/// short framing line the agent reads as instructions.
fn wrap_context(prose: &str) -> String {
    format!(
        "{FORK_CONTEXT_OPEN}\n\
         The conversation below is the prior context this session was forked from. \
         Treat it as already-established history and continue from where it left off; \
         do not redo work that is already complete.\n\n\
         {prose}\n\
         {FORK_CONTEXT_CLOSE}"
    )
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

    fn carried_seqs(carried: &[&SessionRecord]) -> Vec<i64> {
        carried.iter().map(|r| r.seq).collect()
    }

    #[test]
    fn carried_none_context_copies_nothing() {
        let records = sample_records();
        let carried = carried_records(&records, None, Some(6));
        assert!(carried.is_empty());
    }

    #[test]
    fn carried_full_copies_every_record_within_snapshot() {
        // cutoff = i64::MAX (Full), snapshot saw all six records.
        let records = sample_records();
        let carried = carried_records(&records, Some(i64::MAX), Some(6));
        assert_eq!(carried_seqs(&carried), vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn carried_caps_at_snapshot_when_backend_read_has_newer_rows() {
        // The frontend digested a 4-record snapshot (max seq 4); a sync then
        // appended seq 5 & 6 before the backend read. A Full fork must copy only
        // through seq 4 so the child never shows history the brief omitted.
        let records = sample_records();
        let carried = carried_records(&records, Some(i64::MAX), Some(4));
        assert_eq!(carried_seqs(&carried), vec![1, 2, 3, 4]);
    }

    #[test]
    fn carried_missing_snapshot_copies_nothing() {
        // Context carried but the caller reported no records (None) → copy
        // nothing, staying consistent with a null digest.
        let records = sample_records();
        let carried = carried_records(&records, Some(i64::MAX), None);
        assert!(carried.is_empty());
    }

    #[test]
    fn carried_respects_both_cutoff_and_snapshot() {
        // UpToMessage cutoff 5 (copy seq < 5) intersected with a snapshot capped
        // at seq 3 → only seq 1..=3.
        let records = sample_records();
        let carried = carried_records(&records, Some(5), Some(3));
        assert_eq!(carried_seqs(&carried), vec![1, 2, 3]);
    }

    #[test]
    fn wrap_brackets_prose_with_sentinels() {
        let w = wrap_context("User: hi\n\nAssistant: hey");
        assert!(w.starts_with(FORK_CONTEXT_OPEN));
        assert!(w.trim_end().ends_with(FORK_CONTEXT_CLOSE));
        assert!(w.contains("User: hi"));
        assert!(w.contains("Assistant: hey"));
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
