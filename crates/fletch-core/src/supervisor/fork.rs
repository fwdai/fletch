//! Forking a conversation into a new workspace.
//!
//! A fork is a normal [`Supervisor::spawn_agent`] whose new session is seeded
//! along two *independent* axes, so nothing about the runtime, sandbox,
//! worktree, streaming or chat rendering changes:
//!
//!  - **Code** ([`ForkCode`]) — what the new worktree starts from: the parent's
//!    base branch, optionally with the parent's current working tree on top.
//!  - **Context** ([`ForkContext`]) — what of the parent conversation the new
//!    session continues: nothing, or everything through a chosen turn.
//!
//! Carried context reaches the child two ways:
//!  1. **Display** — the child's session references the parent's history up to
//!     the anchor through session lineage (see `workspace::lineage`). Nothing is
//!     copied; the chat renders it through the stitched history read.
//!  2. **Agent knowledge** — the child's agent starts a fresh provider session,
//!     so it is told what was discussed by a plain-text digest of the same
//!     range, composed into its brief. The frontend renders the digest, since it
//!     has every provider's chat adapter, and the backend only wraps it.

use std::borrow::Cow;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::workspace::{AgentRecord, Anchor};

use super::{SpawnRequest, Supervisor};

/// Sentinels that visually bracket the injected prior-conversation digest inside
/// the composed prompt, so the agent can see where the carried context begins and
/// ends. HTML-comment form, namespaced to Fletch. These are purely presentational
/// now: the digest is persisted in its own `forked_context` session column (never
/// spliced into the user brief), so nothing ever parses these back out — a fork
/// simply drops the parent's `forked_context` and rebuilds a fresh one.
const FORK_CONTEXT_OPEN: &str = "<!-- fletch:forked-conversation-context -->";
const FORK_CONTEXT_CLOSE: &str = "<!-- /fletch:forked-conversation-context -->";

/// Hard cap, in bytes, on the stored forked context. It reaches the agent
/// inside a single argv element (e.g. `--append-system-prompt <text>`), and the
/// OS rejects oversized arguments with E2BIG ("Argument list too long"): Linux
/// caps any one argument at 128 KiB (`MAX_ARG_STRLEN`), macOS caps argv + env
/// together at 1 MiB (`ARG_MAX`). 64 KiB leaves room for the rest of the brief.
/// Enforced here, not just by the frontend, because remote clients send their
/// own digest.
const MAX_FORKED_CONTEXT_BYTES: usize = 64 * 1024;

/// First line of a digest that [`keep_tail`] had to cut.
const OMITTED_NOTE: &str =
    "[Earlier conversation omitted to fit the size limit; the most recent part follows.]\n";

/// What the forked workspace's worktree starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkCode {
    /// A fresh worktree from the parent's base branch — no uncommitted work.
    Clean,
    /// The parent's current working tree (incl. uncommitted work) overlaid onto
    /// the fresh checkout — "build on unmerged work".
    Carry,
}

/// What of the parent conversation the fork continues.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ForkContext {
    /// Fresh conversation — carry nothing (the agent's brief is preserved).
    None,
    /// The parent conversation through turn `turn_id` and its reply, or through
    /// the end of the parent's current session when `turn_id` is `None`.
    Through { turn_id: Option<String> },
}

impl ForkContext {
    /// The point in the parent's conversation this context forks at, or `None`
    /// for a fresh conversation.
    fn anchor(&self) -> Option<Anchor<'_>> {
        match self {
            ForkContext::None => None,
            ForkContext::Through { turn_id: Some(t) } => Some(Anchor::Through(t)),
            ForkContext::Through { turn_id: None } => Some(Anchor::End),
        }
    }
}

impl Supervisor {
    /// Fork `parent_id` into a brand-new workspace, seeding its worktree
    /// (`code`) and conversation (`context`) independently.
    ///
    /// `context_digest` is the frontend-rendered prose for the same range
    /// `context` selects (`None` when nothing is carried); it becomes the
    /// injected brief context.
    ///
    /// Returns the new agent record. Heavy provisioning runs in the background
    /// exactly like a normal spawn; the session's lineage and brief are in place
    /// before this returns, so the frontend can open the new agent and load its
    /// history immediately.
    pub async fn fork_agent(
        self: Arc<Self>,
        ctx: Arc<EngineCtx>,
        parent_id: &str,
        code: ForkCode,
        context: ForkContext,
        context_digest: Option<String>,
    ) -> Result<AgentRecord> {
        let parent = self.workspace.agent(parent_id)?;
        let primary = parent
            .repos
            .first()
            .ok_or_else(|| Error::Other("parent agent has no tracked repos".into()))?
            .clone();

        // Resolved before anything is created, so an anchor that can't be
        // placed (its message is still syncing) fails the fork cleanly.
        let lineage = context
            .anchor()
            .map(|anchor| self.workspace.resolve_anchor(parent_id, anchor))
            .transpose()?;

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

        // Code: reuse the normal spawn/provision path. Both modes fork the
        // parent's own base branch (so the fork starts where the parent did);
        // `Carry` additionally overlays the parent's current working tree after
        // provisioning, so its uncommitted work reads as the fork's diff.
        let fork_base = Some(primary.base_branch().await);
        let carry_from = match code {
            ForkCode::Clean => None,
            ForkCode::Carry => Some(primary.checkout_path(parent_id)?),
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
            lineage,
            custom_agent_id: parent.custom_agent_id.clone(),
            skills: parent.skills.clone(),
            mcp_servers: parent.mcp_servers.clone(),
            fork_base,
            run_repo: None,
            owner_run_id: None,
            existing_workspace: None,
            carry_from,
            // A fork is a fresh line of work; it doesn't inherit the parent's
            // originating issue (only one workspace should close it).
            issue_ref: None,
            // Nor its purpose: forking is a sidebar action, and the fork is an
            // ordinary agent that carries the parent's conversation forward.
            purpose: None,
            // A fork has no first prompt yet; its task is captured on first send.
            task: None,
        };
        self.spawn_agent(ctx, req).await
    }
}

/// Wrap the frontend-supplied conversation prose in the fork sentinels plus a
/// short framing line the agent reads as instructions, keeping only as much of
/// the prose's tail as fits the whole thing in [`MAX_FORKED_CONTEXT_BYTES`].
fn wrap_context(prose: &str) -> String {
    let frame = |prose: &str| {
        format!(
            "{FORK_CONTEXT_OPEN}\n\
             The conversation below is the prior context this session was forked from. \
             Treat it as already-established history and continue from where it left off; \
             do not redo work that is already complete.\n\n\
             {prose}\n\
             {FORK_CONTEXT_CLOSE}"
        )
    };
    let budget = MAX_FORKED_CONTEXT_BYTES - frame("").len();
    frame(&keep_tail(prose, budget))
}

/// Fit `text` into `max` bytes (`max` must exceed [`OMITTED_NOTE`]) by keeping
/// its most recent tail — the end of the conversation is what the fork
/// continues from — behind [`OMITTED_NOTE`]. The cut lands on a char boundary
/// and, when one is near, at the start of a line.
fn keep_tail(text: &str, max: usize) -> Cow<'_, str> {
    if text.len() <= max {
        return Cow::Borrowed(text);
    }
    // How far past the byte cut to look for a line start before settling for
    // a mid-line cut, rather than give up most of the budget to reach one.
    const LINE_SEARCH: usize = 1024;

    let mut start = text.len() - (max - OMITTED_NOTE.len());
    while !text.is_char_boundary(start) {
        start += 1;
    }
    if !text[..start].ends_with('\n') {
        let window = &text.as_bytes()[start..text.len().min(start + LINE_SEARCH)];
        if let Some(nl) = window.iter().position(|&b| b == b'\n') {
            start += nl + 1;
        }
    }
    Cow::Owned(format!("{OMITTED_NOTE}{}", &text[start..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wrap_brackets_prose_with_sentinels() {
        let w = wrap_context("User: hi\n\nAssistant: hey");
        assert!(w.starts_with(FORK_CONTEXT_OPEN));
        assert!(w.trim_end().ends_with(FORK_CONTEXT_CLOSE));
        assert!(w.contains("User: hi"));
        assert!(w.contains("Assistant: hey"));
        assert!(!w.contains(OMITTED_NOTE));
    }

    #[test]
    fn wrap_caps_an_oversized_digest_keeping_its_tail() {
        // ~3 MB, the size of the real forks that blew past ARG_MAX.
        let prose: String = (0..100_000)
            .map(|i| format!("User: message {i:06}\n\n"))
            .collect();
        let w = wrap_context(&prose);
        assert!(w.len() <= MAX_FORKED_CONTEXT_BYTES);
        assert!(w.starts_with(FORK_CONTEXT_OPEN));
        assert!(w.ends_with(FORK_CONTEXT_CLOSE));
        assert!(w.contains(OMITTED_NOTE));
        assert!(!w.contains("message 000000"));
        assert!(w.contains("User: message 099999\n"));
        // The cut snapped to a line start, so the first kept line is whole.
        let after_note = w.split(OMITTED_NOTE).nth(1).unwrap();
        assert!(after_note.trim_start().starts_with("User: message "));
    }

    #[test]
    fn keep_tail_passes_text_within_the_cap_through() {
        assert_eq!(keep_tail("short", 100), "short");
        let exact = "x".repeat(100);
        assert_eq!(keep_tail(&exact, 100), exact);
    }

    #[test]
    fn keep_tail_prefers_a_line_start() {
        let text = format!("{}\nkept line\n", "a".repeat(200));
        let max = OMITTED_NOTE.len() + 14;
        // The byte cut lands mid-way through the "a" run; the next line start
        // is close, so the cut moves there.
        assert_eq!(keep_tail(&text, max), format!("{OMITTED_NOTE}kept line\n"));
    }

    #[test]
    fn keep_tail_cuts_mid_line_when_no_line_start_is_near() {
        let text = "a".repeat(10_000);
        let max = OMITTED_NOTE.len() + 100;
        let kept = keep_tail(&text, max);
        assert_eq!(kept, format!("{OMITTED_NOTE}{}", "a".repeat(100)));
    }

    #[test]
    fn keep_tail_never_splits_a_multibyte_char() {
        // 4-byte chars with every leading pad, so the raw byte cut falls at each
        // offset within a char. Slicing off a boundary would panic.
        for pad in 0..4 {
            let text = format!("{}{}", "x".repeat(pad), "😀".repeat(1_000));
            for budget in 1..12 {
                let max = OMITTED_NOTE.len() + budget;
                let kept = keep_tail(&text, max);
                assert!(kept.len() <= max, "pad {pad}, budget {budget}");
                let tail = kept.strip_prefix(OMITTED_NOTE).unwrap();
                assert_eq!(tail, "😀".repeat(budget / 4), "pad {pad}, budget {budget}");
            }
        }
    }

    #[test]
    fn context_deserializes_to_its_anchor() {
        let none: ForkContext = serde_json::from_value(json!({"kind": "none"})).unwrap();
        assert_eq!(none.anchor(), None);
        let turn: ForkContext =
            serde_json::from_value(json!({"kind": "through", "turn_id": "t1"})).unwrap();
        assert_eq!(turn.anchor(), Some(Anchor::Through("t1")));
        let end: ForkContext =
            serde_json::from_value(json!({"kind": "through", "turn_id": null})).unwrap();
        assert_eq!(end.anchor(), Some(Anchor::End));

        let clean: ForkCode = serde_json::from_value(json!("clean")).unwrap();
        assert_eq!(clean, ForkCode::Clean);
        let carry: ForkCode = serde_json::from_value(json!("carry")).unwrap();
        assert_eq!(carry, ForkCode::Carry);
    }
}
