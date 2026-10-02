//! Forking a conversation into a new workspace.
//!
//! A fork is a normal [`Supervisor::spawn_agent`] whose new session is seeded
//! from a point in the parent's conversation, the *anchor* (a turn, or the end
//! of the conversation), along two independent axes, so nothing about the
//! runtime, sandbox, worktree, streaming or chat rendering changes:
//!
//!  - **Code** ([`ForkCode`]) — what the new checkouts start from: the
//!    parent's base branch, the parent's code now, or the code as it was at
//!    the anchor (a turn checkpoint, see `git::checkpoint`).
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

use super::{CodeSource, SpawnRequest, Supervisor};

/// What the forked workspace's checkouts start from. Every mode but `Clean`
/// copies a snapshot faithfully: the checkout's working tree is the snapshot,
/// and its HEAD the commit the snapshot was taken on, so committed work stays
/// committed and uncommitted work uncommitted. That applies to each repo of a
/// multi-repo workspace that has a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkCode {
    /// Fresh checkouts of the parent's base branch.
    Clean,
    /// The parent's code as it is now — "build on unmerged work".
    Current,
    /// The code as it was through the anchor turn and its reply.
    AtMessage,
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

        // Code: reuse the normal spawn/provision path. Every mode forks the
        // parent's own base branch (so the fork targets what the parent did);
        // the others then start the checkouts from a snapshot, resolved — and
        // for the parent's current code, taken — before anything is created.
        let fork_base = Some(primary.base_branch().await);
        let code_from = self.fork_code(&parent, code, turn_id).await?;

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
            code_from,
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

    /// The snapshots `code` starts the fork's checkouts from (none for a clean
    /// fork). `turn_id` is the fork's anchor; code as of a message needs one.
    async fn fork_code(
        &self,
        parent: &AgentRecord,
        code: ForkCode,
        turn_id: Option<&str>,
    ) -> Result<Vec<CodeSource>> {
        match (code, turn_id) {
            (ForkCode::Clean, _) => Ok(Vec::new()),
            (ForkCode::Current, _) => self.pin_code(&parent.id).await,
            (ForkCode::AtMessage, Some(turn_id)) => self.code_through(parent, turn_id).await,
            (ForkCode::AtMessage, None) => Err(Error::Other(
                "Code as of a message needs a message to fork from.".into(),
            )),
        }
    }

    /// The code as turn `turn_id`'s reply left it: the checkpoint of the turn
    /// delivered next in `turn_id`'s own session, kept in the checkouts of the
    /// workspace that ran it (an ancestor's, for an inherited turn). When
    /// nothing came next and `turn_id` is the latest turn of `parent`'s
    /// conversation, that code is `parent`'s current code.
    async fn code_through(&self, parent: &AgentRecord, turn_id: &str) -> Result<Vec<CodeSource>> {
        let history = self.workspace.read_history_turns(&parent.id)?;
        let position = history
            .iter()
            .position(|t| t.turn_id == turn_id)
            .ok_or_else(|| {
                Error::Other(format!(
                    "turn {turn_id} is not part of {}'s conversation",
                    parent.id
                ))
            })?;
        let code = match self.workspace.turn_after(turn_id)? {
            Some((owner, next)) => self
                .turn_checkpoints(&owner, &next)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|checkpoint| checkpoint.sha.is_some())
                .map(|checkpoint| CodeSource {
                    subdir: checkpoint.subdir,
                    checkout: checkpoint.checkout,
                    key: next.clone(),
                })
                .collect(),
            None if position + 1 == history.len() => return self.pin_code(&parent.id).await,
            None => Vec::new(),
        };
        if code.is_empty() {
            // No snapshot was taken (the turn predates checkpoints, or the
            // next one was sent mid-turn), or the checkouts holding it are
            // gone with their workspace.
            return Err(Error::Other(
                "The code as of this message isn't available: no snapshot of it was kept, \
                 or the workspace that ran it is gone."
                    .into(),
            ));
        }
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::checkpoint;
    use crate::supervisor::tests::{committed_repo, record_in_checkouts, test_supervisor};
    use serde_json::json;
    use std::path::{Path, PathBuf};

    #[test]
    fn code_and_context_deserialize_from_their_wire_names() {
        let none: ForkContext = serde_json::from_value(json!("none")).unwrap();
        assert_eq!(none, ForkContext::None);
        let summary: ForkContext = serde_json::from_value(json!("summary")).unwrap();
        assert_eq!(summary, ForkContext::Summary);

        for (wire, code) in [
            ("clean", ForkCode::Clean),
            ("current", ForkCode::Current),
            ("at_message", ForkCode::AtMessage),
        ] {
            assert_eq!(
                serde_json::from_value::<ForkCode>(json!(wire)).unwrap(),
                code
            );
        }
    }

    /// Workspace `id` working in `checkout`, continuing `lineage`.
    fn agent(
        sup: &Supervisor,
        id: &str,
        checkout: &Path,
        lineage: Option<crate::workspace::SessionLineage>,
    ) -> AgentRecord {
        let mut record = record_in_checkouts(sup, id, &[checkout.to_path_buf()]);
        record.lineage = lineage;
        sup.workspace.add_agent(&mut record).unwrap();
        sup.workspace.agent(id).unwrap()
    }

    /// Deliver `turn` to `ws` as `deliver_as_turn` does — checkpoint, then the
    /// turn row — and ingest its prompt, matched.
    async fn deliver(sup: &Supervisor, ws: &str, turn: &str) {
        sup.checkpoint_turn(ws, turn).await;
        sup.workspace.insert_user_turn(ws, turn, turn, &[]).unwrap();
        let prompt = json!({"type": "user", "text": turn});
        sup.workspace
            .append_session_records(ws, "claude", "transcript", None, &[(turn, &prompt)])
            .unwrap();
        sup.workspace.associate_pending_user_turns(ws).unwrap();
    }

    fn edit(checkout: &Path, text: &str) {
        std::fs::write(checkout.join("a.txt"), text).unwrap();
    }

    /// `alps` ran a1 and a2, editing between them; `andes` forked it through
    /// a1 and has run b1 since.
    async fn forked_pair(dir: &Path) -> (Supervisor, PathBuf, PathBuf) {
        let sup = test_supervisor();
        let (alps, andes) = (
            committed_repo(dir, "alps").await,
            committed_repo(dir, "andes").await,
        );
        agent(&sup, "alps", &alps, None);
        deliver(&sup, "alps", "a1").await;
        edit(&alps, "after a1");
        deliver(&sup, "alps", "a2").await;
        edit(&alps, "after a2");
        let lineage = sup
            .workspace
            .resolve_anchor("alps", Anchor::Through("a1"))
            .unwrap();
        agent(&sup, "andes", &andes, Some(lineage));
        deliver(&sup, "andes", "b1").await;
        (sup, alps, andes)
    }

    async fn code(sup: &Supervisor, ws: &str, turn: Option<&str>) -> Result<Vec<CodeSource>> {
        let parent = sup.workspace.agent(ws).unwrap();
        sup.fork_code(&parent, ForkCode::AtMessage, turn).await
    }

    #[tokio::test]
    async fn code_as_of_an_inherited_message_comes_from_the_workspace_that_ran_it() {
        let td = tempfile::tempdir().unwrap();
        let (sup, alps, _) = forked_pair(td.path()).await;

        // From `andes`, a1 is inherited: the code its reply left is a2's
        // checkpoint, kept in `alps`.
        let found = code(&sup, "andes", Some("a1")).await.unwrap();
        assert_eq!(
            found,
            [CodeSource {
                subdir: "repo-0".into(),
                checkout: alps.clone(),
                key: "a2".into(),
            }]
        );
        let sha = checkpoint::resolve(&alps, "a2").await.unwrap().unwrap();
        let file = crate::git::run_git(&alps, &["show", &format!("{sha}:a.txt")], "show")
            .await
            .unwrap();
        assert_eq!(file.stdout, b"after a1");
    }

    #[tokio::test]
    async fn code_as_of_the_latest_message_is_the_parents_code_now() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _, andes) = forked_pair(td.path()).await;
        edit(&andes, "after b1");

        let found = code(&sup, "andes", Some("b1")).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].checkout, andes);
        let sha = checkpoint::resolve(&andes, &found[0].key)
            .await
            .unwrap()
            .expect("pinned now");
        let file = crate::git::run_git(&andes, &["show", &format!("{sha}:a.txt")], "show")
            .await
            .unwrap();
        assert_eq!(file.stdout, b"after b1");
    }

    #[tokio::test]
    async fn code_as_of_a_message_fails_without_its_snapshot() {
        let td = tempfile::tempdir().unwrap();
        let (sup, alps, _) = forked_pair(td.path()).await;
        let unavailable = |r: Result<Vec<CodeSource>>| {
            let err = r.unwrap_err().to_string();
            assert!(err.contains("isn't available"), "{err}");
        };

        // a3 went out without a checkpoint (mid-turn, or before checkpoints).
        sup.workspace
            .insert_user_turn("alps", "a3", "a3", &[])
            .unwrap();
        unavailable(code(&sup, "alps", Some("a2")).await);
        // `alps` is gone, and its checkpoints with its checkout.
        std::fs::remove_dir_all(&alps).unwrap();
        unavailable(code(&sup, "andes", Some("a1")).await);

        // Not this conversation's turn (a2 is past the cut `andes` forked
        // at), or no message at all.
        assert!(code(&sup, "andes", Some("a2")).await.is_err());
        assert!(code(&sup, "andes", None).await.is_err());
    }

    #[tokio::test]
    async fn clean_code_has_no_snapshot_and_current_code_pins_every_checkout() {
        let td = tempfile::tempdir().unwrap();
        let sup = test_supervisor();
        let checkouts = [
            committed_repo(td.path(), "a").await,
            committed_repo(td.path(), "b").await,
        ];
        let mut record = record_in_checkouts(&sup, "denali", &checkouts);
        sup.workspace.add_agent(&mut record).unwrap();

        let clean = sup.fork_code(&record, ForkCode::Clean, None).await.unwrap();
        assert!(clean.is_empty());
        let current = sup
            .fork_code(&record, ForkCode::Current, None)
            .await
            .unwrap();
        assert_eq!(current.len(), 2);
        for (source, checkout) in current.iter().zip(&checkouts) {
            assert_eq!(&source.checkout, checkout);
            let pinned = checkpoint::resolve(checkout, &source.key).await.unwrap();
            assert!(pinned.is_some(), "{}", checkout.display());
        }
    }
}
