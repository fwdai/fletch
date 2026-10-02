//! Forking a conversation into a new workspace.
//!
//! A fork is a normal [`Supervisor::spawn_agent`] whose new session is seeded
//! from a point in the parent's conversation, the *anchor* (a turn, or the end
//! of the conversation), along two independent axes, so nothing about the
//! runtime, sandbox, worktree, streaming or chat rendering changes:
//!
//!  - **Code** ([`ForkCode`]) — what the new worktree starts from: the parent's
//!    base branch, optionally with the parent's current working tree on top.
//!  - **Context** ([`ForkContext`]) — what the new session knows of the
//!    parent conversation up to the anchor: nothing, or a summary.
//!
//! Carried context reaches the child two ways:
//!  1. **Display** — the child's session references the parent's history up to
//!     the anchor through session lineage (see `workspace::lineage`). Nothing is
//!     copied; the chat renders it through the stitched history read.
//!  2. **Agent knowledge** — the child's agent starts a fresh provider session,
//!     so it is told what was discussed by a summary of the same range, written
//!     while the spawn provisions (`crate::handoff`). The frontend renders the
//!     transcript the summary is made from, since it has every provider's chat
//!     adapter.

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::workspace::{AgentRecord, Anchor};

use super::{SpawnRequest, Supervisor};

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

/// What the forked session knows of the parent conversation up to the anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkContext {
    /// A fresh conversation — carry nothing (the agent's brief is preserved).
    None,
    /// Show the parent's history up to the anchor, and tell the agent about it
    /// through a summary.
    Summary,
}

impl Supervisor {
    /// Fork `parent_id` into a brand-new workspace at an anchor: through turn
    /// `turn_id` and its reply, or through the end of the parent's current
    /// session when `None`. Its worktree (`code`) and conversation (`context`)
    /// are seeded independently.
    ///
    /// `transcript` is the frontend-rendered text of the conversation up to the
    /// anchor (`None` when nothing is carried); the spawn summarizes it into the
    /// new session's handoff context.
    ///
    /// Returns the new agent record. Heavy provisioning, summarizing included,
    /// runs in the background exactly like a normal spawn; the session's
    /// lineage is in place before this returns, so the frontend can open the
    /// new agent and load its history immediately.
    pub async fn fork_agent(
        self: Arc<Self>,
        ctx: Arc<EngineCtx>,
        parent_id: &str,
        turn_id: Option<&str>,
        code: ForkCode,
        context: ForkContext,
        transcript: Option<String>,
    ) -> Result<AgentRecord> {
        let parent = self.workspace.agent(parent_id)?;
        let primary = parent
            .repos
            .first()
            .ok_or_else(|| Error::Other("parent agent has no tracked repos".into()))?
            .clone();

        // Resolved before anything is created, so an anchor that can't be
        // placed (its message is still syncing) fails the fork cleanly.
        let anchor = match turn_id {
            Some(turn_id) => Anchor::Through(turn_id),
            None => Anchor::End,
        };
        let (lineage, handoff_transcript) = match context {
            ForkContext::None => (None, None),
            ForkContext::Summary => (
                Some(self.workspace.resolve_anchor(parent_id, anchor)?),
                transcript.filter(|t| !t.trim().is_empty()),
            ),
        };

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
            // The parent's brief passes through verbatim; its handoff context
            // never does. Each fork gets a summary of its own.
            instructions: parent.instructions.clone(),
            handoff_transcript,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn code_and_context_deserialize_from_their_wire_names() {
        let none: ForkContext = serde_json::from_value(json!("none")).unwrap();
        assert_eq!(none, ForkContext::None);
        let summary: ForkContext = serde_json::from_value(json!("summary")).unwrap();
        assert_eq!(summary, ForkContext::Summary);

        let clean: ForkCode = serde_json::from_value(json!("clean")).unwrap();
        assert_eq!(clean, ForkCode::Clean);
        let carry: ForkCode = serde_json::from_value(json!("carry")).unwrap();
        assert_eq!(carry, ForkCode::Carry);
    }
}
