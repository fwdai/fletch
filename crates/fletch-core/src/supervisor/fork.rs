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
//!  - **Context** ([`ForkContext`]) — how the new session's agent knows the
//!    parent conversation up to the anchor: the full conversation, or a
//!    summary of it.
//!
//! The conversation reaches the child two ways:
//!  1. **Display** — the child's session references the parent's history up to
//!     the anchor through session lineage (see `workspace::lineage`). Nothing is
//!     stored twice; the chat renders it through the stitched history read.
//!  2. **Agent knowledge** — `Full`: the same history is written as the child's
//!     own provider transcript once its checkouts exist
//!     (`Supervisor::materialize`), and its CLI resumes it. `Summary`: the
//!     child's agent starts a fresh provider session and is told a summary of
//!     the range, written while the spawn provisions (`crate::handoff`). The
//!     frontend renders the transcript the summary is made from, since it has
//!     every provider's chat adapter.

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::workspace::{AgentRecord, Anchor};

use super::checkpoints::OTHER_CODE;
use super::{PinnedCode, SpawnHandoff, SpawnRequest, Supervisor};

/// What the forked workspace's checkouts start from. Every mode but `Clean`
/// copies a snapshot faithfully: the checkout's working tree is the snapshot,
/// and its HEAD the commit the snapshot was taken on, so committed work stays
/// committed and uncommitted work uncommitted. It takes a snapshot of every
/// repo of the source workspace, or fails before anything is created: a fork
/// never starts from part of the code ([`PinnedCode`]).
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

/// How the forked session's agent knows the parent conversation up to the
/// anchor. Either way the chat shows that history, through lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkContext {
    /// The full conversation: the agent resumes it natively, as its own
    /// transcript. Only for a provider Fletch can write transcripts for
    /// (`AgentCapabilities::transcript_writer`), and a history all its own.
    Full,
    /// A summary of it.
    Summary,
}

impl Supervisor {
    /// Fork `parent_id` into a brand-new workspace at an anchor: through turn
    /// `turn_id` and its reply, or through the end of the parent's current
    /// session when `None`. Its worktree (`code`) and conversation (`context`)
    /// are seeded independently.
    ///
    /// `transcript` is the frontend-rendered text of the conversation up to the
    /// anchor, for a `Summary`; the spawn summarizes it into the new session's
    /// handoff context. A `Full` fork that can't be made (see [`ForkContext`])
    /// is refused before anything is created.
    ///
    /// Returns the new agent record. Heavy provisioning, writing the
    /// transcript or summarizing included, runs in the background exactly like
    /// a normal spawn; the session's lineage is in place before this returns,
    /// so the frontend can open the new agent and load its history
    /// immediately.
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
        // placed (its message is still syncing), or a conversation the child
        // can't continue in full, fails the fork cleanly.
        let anchor = match turn_id {
            Some(turn_id) => Anchor::Through(turn_id),
            None => Anchor::End,
        };
        let lineage = self.workspace.resolve_anchor(parent_id, anchor)?;
        let handoff = match context {
            ForkContext::Full => {
                if let Err(why) = self.native_history(&parent.provider, &lineage)? {
                    return Err(Error::Other(format!(
                        "The full conversation can't be carried over: {why} Fork with a summary instead."
                    )));
                }
                Some(SpawnHandoff::Native {
                    context: self
                        .workspace
                        .session_handoff_context(&lineage.parent_session_id)?,
                })
            }
            ForkContext::Summary => transcript
                .filter(|t| !t.trim().is_empty())
                .map(SpawnHandoff::Summary),
        };

        // Code: reuse the normal spawn/provision path. Every mode forks the
        // parent's own base branch (so the fork targets what the parent did);
        // the others then start the checkouts from a snapshot, resolved — and
        // for the parent's current code, taken — before anything is created.
        let fork_base = Some(primary.base_branch().await);
        let code_from = self.fork_code(&parent, code, turn_id).await?;
        let pin = code_from.clone();

        let req = SpawnRequest {
            view: parent.view,
            repo_path: primary.repo_path.clone(),
            provider: parent.provider.clone(),
            name: None,
            effort: parent.effort.clone(),
            model: parent.model.clone(),
            // The parent's brief passes through verbatim. What its agent was
            // told of an earlier conversation goes with a full one; a summary
            // is a fork's own.
            instructions: parent.instructions.clone(),
            handoff,
            lineage: Some(lineage),
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
            // A fresh agent follows the account active in Settings.
            account: None,
        };
        let spawned = self.spawn_agent(ctx, req).await;
        // Once spawned, its provisioning releases the code's pin; a spawn
        // refused before that never will.
        if let (Err(_), Some(code)) = (&spawned, pin) {
            code.release().await;
        }
        spawned
    }

    /// The pinned code `code` starts the fork's checkouts from (`None` for a
    /// clean fork) — always the code of every checkout of its workspace, never
    /// part of it. `turn_id` is the fork's anchor; code as of a message needs
    /// one.
    async fn fork_code(
        &self,
        parent: &AgentRecord,
        code: ForkCode,
        turn_id: Option<&str>,
    ) -> Result<Option<PinnedCode>> {
        match (code, turn_id) {
            (ForkCode::Clean, _) => Ok(None),
            (ForkCode::Current, _) => self.pin_code(&parent.id).await.map(Some),
            (ForkCode::AtMessage, Some(turn_id)) => {
                self.code_through(parent, turn_id).await.map(Some)
            }
            (ForkCode::AtMessage, None) => Err(Error::Other(
                "Code as of a message needs a message to fork from.".into(),
            )),
        }
    }

    /// The code as turn `turn_id`'s reply left it: the checkpoint of the turn
    /// delivered next in `turn_id`'s own session, in every checkout of the
    /// workspace that ran it (an ancestor's, for an inherited turn). When
    /// nothing came next and `turn_id` is the latest turn of `parent`'s
    /// conversation, that code is `parent`'s current code.
    async fn code_through(&self, parent: &AgentRecord, turn_id: &str) -> Result<PinnedCode> {
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
        match self.workspace.turn_after(turn_id)? {
            Some((owner, next)) => self.code_at(&owner, &next).await,
            None if position + 1 == history.len() => self.pin_code(&parent.id).await,
            // The turn ended its session, but the conversation went on in a
            // later one: the code as that session left it was never pinned.
            None => Err(Error::Other(format!(
                "The code as of this message wasn't kept. {OTHER_CODE}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::checkpoint;
    use crate::supervisor::tests::{
        committed_repo, pins, record_in_checkouts, test_supervisor, workspace_of,
    };
    use crate::supervisor::PinnedCheckout;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    #[test]
    fn code_and_context_deserialize_from_their_wire_names() {
        for (wire, context) in [
            ("full", ForkContext::Full),
            ("summary", ForkContext::Summary),
        ] {
            assert_eq!(
                serde_json::from_value::<ForkContext>(json!(wire)).unwrap(),
                context
            );
        }
        // Every fork continues the conversation: there is no fresh one.
        assert!(serde_json::from_value::<ForkContext>(json!("none")).is_err());

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

    async fn code(sup: &Supervisor, ws: &str, turn: Option<&str>) -> Result<PinnedCode> {
        let parent = sup.workspace.agent(ws).unwrap();
        let code = sup.fork_code(&parent, ForkCode::AtMessage, turn).await?;
        Ok(code.expect("code as of a message pins code"))
    }

    /// `a.txt` as pinned under `key` in `checkout`.
    async fn pinned_file(checkout: &Path, key: &str) -> Vec<u8> {
        let sha = checkpoint::resolve(checkout, key).await.unwrap().unwrap();
        crate::git::run_git(checkout, &["show", &format!("{sha}:a.txt")], "show")
            .await
            .unwrap()
            .stdout
    }

    #[tokio::test]
    async fn code_as_of_an_inherited_message_comes_from_the_workspace_that_ran_it() {
        let td = tempfile::tempdir().unwrap();
        let (sup, alps, _) = forked_pair(td.path()).await;

        // From `andes`, a1 is inherited: the code its reply left is a2's
        // checkpoint, kept in `alps`.
        let found = code(&sup, "andes", Some("a1")).await.unwrap();
        assert_eq!(found.key(), "a2");
        assert_eq!(
            found.checkouts(),
            [PinnedCheckout {
                repo_path: alps.clone(),
                subdir: "repo-0".into(),
                checkout: alps.clone(),
            }]
        );
        assert_eq!(pinned_file(&alps, "a2").await, b"after a1");
    }

    #[tokio::test]
    async fn code_as_of_the_latest_message_is_the_parents_code_now() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _, andes) = forked_pair(td.path()).await;
        edit(&andes, "after b1");

        let found = code(&sup, "andes", Some("b1")).await.unwrap();
        assert_eq!(found.checkouts().len(), 1);
        assert_eq!(found.checkouts()[0].checkout, andes);
        assert_eq!(pinned_file(&andes, found.key()).await, b"after b1");
    }

    #[tokio::test]
    async fn code_as_of_a_message_fails_without_its_snapshot() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _, _) = forked_pair(td.path()).await;
        let not_kept = |r: Result<PinnedCode>| {
            let err = r.unwrap_err().to_string();
            assert!(err.contains("wasn't kept"), "{err}");
            assert!(err.contains("Current code"), "{err}");
        };

        // a2 ended `alps`'s session, but `atlas` went on from it in one of
        // its own: nothing pinned the code a2 left.
        let atlas = committed_repo(td.path(), "atlas").await;
        let lineage = sup
            .workspace
            .resolve_anchor("alps", Anchor::Through("a2"))
            .unwrap();
        agent(&sup, "atlas", &atlas, Some(lineage));
        deliver(&sup, "atlas", "c1").await;
        not_kept(code(&sup, "atlas", Some("a2")).await);

        // a3 went out without a checkpoint (mid-turn, or before checkpoints).
        sup.workspace
            .insert_user_turn("alps", "a3", "a3", &[])
            .unwrap();
        not_kept(code(&sup, "alps", Some("a2")).await);

        // Not this conversation's turn (a2 is past the cut `andes` forked
        // at), or no message at all.
        assert!(code(&sup, "andes", Some("a2")).await.is_err());
        assert!(code(&sup, "andes", None).await.is_err());
    }

    /// A checkout that can't be read is an error of its own, not a snapshot
    /// that was never kept.
    #[tokio::test]
    async fn a_failed_lookup_is_not_taken_for_a_missing_snapshot() {
        let td = tempfile::tempdir().unwrap();
        let (sup, alps, _) = forked_pair(td.path()).await;
        std::fs::remove_dir_all(&alps).unwrap();

        let err = code(&sup, "andes", Some("a1"))
            .await
            .unwrap_err()
            .to_string();
        assert!(!err.contains("wasn't kept"), "{err}");
        assert!(!err.contains("isn't available"), "{err}");
    }

    /// `denali` checks out two repos and ran t1, t2 and t3; t3's checkpoint
    /// was only taken in its first repo.
    async fn two_repos_one_snapshot_short(dir: &Path) -> (Arc<Supervisor>, [PathBuf; 2]) {
        let sup = Arc::new(test_supervisor());
        let checkouts = [
            committed_repo(dir, "a").await,
            committed_repo(dir, "b").await,
        ];
        let mut record = record_in_checkouts(&sup, "denali", &checkouts);
        sup.workspace.add_agent(&mut record).unwrap();
        deliver(&sup, "denali", "t1").await;
        edit(&checkouts[0], "after t1");
        edit(&checkouts[1], "after t1");
        deliver(&sup, "denali", "t2").await;
        checkpoint::capture(&checkouts[0], "t3").await.unwrap();
        sup.workspace
            .insert_user_turn("denali", "t3", "t3", &[])
            .unwrap();
        (sup, checkouts)
    }

    #[tokio::test]
    async fn code_as_of_a_message_takes_every_repo() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkouts) = two_repos_one_snapshot_short(td.path()).await;

        let found = code(&sup, "denali", Some("t1")).await.unwrap();
        assert_eq!(found.key(), "t2");
        let pinned: Vec<&Path> = found
            .checkouts()
            .iter()
            .map(|c| c.checkout.as_path())
            .collect();
        assert_eq!(pinned, [checkouts[0].as_path(), checkouts[1].as_path()]);
        for checkout in &checkouts {
            assert_eq!(pinned_file(checkout, "t2").await, b"after t1");
        }
    }

    #[tokio::test]
    async fn a_fork_missing_any_repos_snapshot_fails_and_spawns_nothing() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = two_repos_one_snapshot_short(td.path()).await;
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        let err = sup
            .clone()
            .fork_agent(
                ctx,
                "denali",
                Some("t2"),
                ForkCode::AtMessage,
                ForkContext::Summary,
                None,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("wasn't kept for repo-1."), "{err}");
        assert!(!err.contains("repo-0"), "{err}");
        assert!(
            err.contains("\"Current code\" or \"Clean from base\""),
            "{err}"
        );
        assert_eq!(sup.workspace.current().unwrap().agents.len(), 1);
    }

    /// The spawn is refused before its provisioning (which releases the pin)
    /// ever runs: the fork releases it itself.
    #[tokio::test]
    async fn a_fork_refused_at_spawn_leaves_no_pin() {
        let td = tempfile::tempdir().unwrap();
        let sup = Arc::new(test_supervisor());
        let source = committed_repo(td.path(), "src").await;
        let checkout = committed_repo(td.path(), "checkout").await;
        workspace_of(&sup, "denali", &[&source], std::slice::from_ref(&checkout));
        // The source repo is gone, so the spawn refuses it.
        std::fs::remove_dir_all(source.join(".git")).unwrap();
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();

        let forked = sup
            .clone()
            .fork_agent(
                ctx,
                "denali",
                None,
                ForkCode::Current,
                ForkContext::Summary,
                None,
            )
            .await;
        assert!(forked
            .unwrap_err()
            .to_string()
            .contains("not a git repository"));
        assert_eq!(pins(&checkout).await, Vec::<String>::new());
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
        assert_eq!(clean, None);
        let current = sup
            .fork_code(&record, ForkCode::Current, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.checkouts().len(), 2);
        for (pinned, checkout) in current.checkouts().iter().zip(&checkouts) {
            assert_eq!(&pinned.checkout, checkout);
            let sha = checkpoint::resolve(checkout, current.key()).await.unwrap();
            assert!(sha.is_some(), "{}", checkout.display());
        }
    }
}
