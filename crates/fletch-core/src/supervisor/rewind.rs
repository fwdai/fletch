//! Rewinding an agent's workspace to just before one of its messages, in
//! place: the conversation, the code, or both (docs/fork-and-rewind.md,
//! "Rewind"). A rewind is a fork into the same workspace, made of the pieces
//! fork and the session switch already use:
//!
//!  - **Conversation** — a new session continues the history before the turn
//!    (`resolve_anchor(Before)`, `start_session`). Its agent knows that
//!    history natively when it can (`Exact`: claude resumes the session that
//!    ran the turn, cut just before it), or from a summary, as a fork's agent
//!    does (`Summary`).
//!  - **Code** — every checkout goes back to the turn's checkpoint, behind an
//!    undo point (`restore_turn_code`).
//!
//! Orchestration only. The steps run under the agent's delivery lock and
//! input route, so no message starts a turn and no archive takes the
//! workspace halfway through.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::{capabilities, claude_branch_before, BranchPoint, SessionStart};
use crate::error::{Error, Result};
use crate::git::checkpoint;
use crate::host::EngineCtx;
use crate::workspace::{AgentRecord, AgentStatus, AgentView, Anchor, SessionLineage};

use super::events::{emit_spawn_progress, emit_workspace_changed, SpawnStage};
use super::lifecycle::{arm_spawn_timeout, fail_spawn};
use super::{RestoreReport, Supervisor};

/// What a rewind puts back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindScope {
    Conversation,
    Code,
    Both,
}

impl RewindScope {
    fn conversation(self) -> bool {
        self != Self::Code
    }

    fn code(self) -> bool {
        self != Self::Conversation
    }
}

/// What a rewind did.
#[derive(Debug, Serialize)]
pub struct RewindOutcome {
    /// The code restore, which [`Supervisor::undo_code_restore`] reverses;
    /// `None` when the code wasn't rewound.
    pub code: Option<RestoreReport>,
    /// Why the conversation couldn't be rewound after the code was. The code
    /// stays restored and `code` can undo it, so this is reported alongside
    /// it rather than as the rewind's error.
    pub conversation_error: Option<String>,
}

/// How the rewound session's agent learns the conversation before the turn.
#[derive(Debug, PartialEq, Eq)]
enum Handoff {
    /// `Exact`: claude resumes the session that ran the turn, cut just before
    /// it.
    Branch(BranchPoint),
    /// `Exact` too, when the turn opened the session that ran it: start fresh
    /// as that session did, told what its agent was told (nothing for a
    /// workspace's first session).
    Carry(Option<String>),
    /// `Summary`: a summary of this transcript of the conversation before the
    /// turn, rendered by the client.
    Summarize(String),
}

/// A conversation rewind, decided before anything changes.
#[derive(Debug)]
struct ConversationPlan {
    lineage: SessionLineage,
    handoff: Handoff,
}

impl Supervisor {
    /// Rewind `agent_id` to just before turn `turn_id`: its conversation, its
    /// code, or both (`scope`). `transcript` is the client-rendered text of
    /// the conversation before the turn, for a `Summary` handoff.
    ///
    /// Refused while a turn runs ("stop the agent first"), for a workflow
    /// step, and from the native view: claude can cut a conversation at a
    /// message only in the chat view. Everything that can refuse is checked
    /// before anything changes, the code's checkpoint included; then the
    /// code goes first and the conversation second. A conversation that can't
    /// be rewound after the code was restored is reported in the outcome, with
    /// the code's undo.
    ///
    /// Returns once the rewound agent is up, or failed to start: it is then in
    /// error, as after a failed spawn.
    pub async fn rewind(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        turn_id: &str,
        scope: RewindScope,
        transcript: Option<String>,
    ) -> Result<RewindOutcome> {
        // The existence check, before a delivery lock is made for the id.
        self.workspace.agent(agent_id)?;
        let _delivering = self.lock_delivery(agent_id).await;
        let _route = self.open_route(agent_id)?;
        let record = self.workspace.agent(agent_id)?;
        self.check_rewindable(&record)?;
        let conversation = if scope.conversation() {
            Some(self.plan_conversation(&record, turn_id, transcript)?)
        } else {
            None
        };
        let code = if scope.code() {
            self.preview_rewind_code(agent_id, turn_id).await?;
            Some(self.restore_turn_code(agent_id, turn_id).await?)
        } else {
            None
        };
        let conversation_error = match conversation {
            None => None,
            Some(plan) => match self.rewind_conversation(ctx, &record, plan).await {
                Ok(()) => None,
                Err(e) if code.is_some() => Some(format!(
                    "The code was restored, but the conversation couldn't be rewound: {e}"
                )),
                Err(e) => return Err(e),
            },
        };
        Ok(RewindOutcome {
            code,
            conversation_error,
        })
    }

    /// What rewinding `agent_id`'s code to before `turn_id` would do, for the
    /// confirmation, or why it can't: the turn ran in another workspace (a
    /// fork's parent), whose checkouts hold its checkpoints, or no checkpoint
    /// of it was kept.
    pub async fn preview_rewind_code(
        &self,
        agent_id: &str,
        turn_id: &str,
    ) -> Result<RestoreReport> {
        if !self.workspace.turn_session(agent_id, turn_id)?.own {
            return Err(Error::Other(
                "The code as of this message isn't available: it ran in another workspace.".into(),
            ));
        }
        let report = self.preview_turn_code_restore(agent_id, turn_id).await?;
        if report.repos.iter().all(|repo| repo.checkpoint.is_none()) {
            return Err(Error::Other(
                "The code as of this message isn't available: no snapshot of it was kept.".into(),
            ));
        }
        Ok(report)
    }

    /// Undo the code restore `report` describes ([`RewindOutcome::code`]):
    /// put each checkout it changed back as it stood just before, from its
    /// undo point. Under the same lock and route as the restore, and refused
    /// while a turn runs, as the restore is.
    pub async fn undo_code_restore(&self, agent_id: &str, report: &RestoreReport) -> Result<()> {
        self.workspace.agent(agent_id)?;
        let _delivering = self.lock_delivery(agent_id).await;
        let _route = self.open_route(agent_id)?;
        if self.is_busy(agent_id) {
            return Err(Error::Other(
                "Stop the agent before undoing the code restore.".into(),
            ));
        }
        let record = self.workspace.agent(agent_id)?;
        let mut undos = Vec::new();
        for restored in &report.repos {
            let Some(undo) = &restored.undo_ref else {
                continue;
            };
            if !checkpoint::is_undo_ref(undo) {
                return Err(Error::Other(format!("not an undo point: {undo:?}")));
            }
            let repo = record
                .repos
                .iter()
                .find(|repo| repo.subdir == restored.subdir)
                .ok_or_else(|| Error::Other(format!("no checkout {}", restored.subdir)))?;
            undos.push((repo.checkout_path(agent_id)?, undo));
        }
        // Each one only resets to its own undo point, so a retry after a
        // failure part way redoes the ones already back without harm.
        for (checkout, undo) in undos {
            checkpoint::restore(&checkout, undo).await?;
        }
        Ok(())
    }

    /// Why `record` can't be rewound now, if it can't.
    fn check_rewindable(&self, record: &AgentRecord) -> Result<()> {
        let refusal = if record.archive.is_some() {
            "agent is archived"
        } else if record.owner_run_id.is_some() {
            "A workflow step's conversation belongs to its run."
        } else if record.view == AgentView::Native {
            "Rewind from the chat view."
        } else if self.is_busy(&record.id) {
            "Stop the agent before rewinding."
        } else {
            return Ok(());
        };
        Err(Error::Other(refusal.into()))
    }

    /// Where `record`'s rewound conversation starts — the history before
    /// `turn_id`'s prompt — and how its agent learns that history.
    fn plan_conversation(
        &self,
        record: &AgentRecord,
        turn_id: &str,
        transcript: Option<String>,
    ) -> Result<ConversationPlan> {
        let lineage = self
            .workspace
            .resolve_anchor(&record.id, Anchor::Before(turn_id))?;
        let handoff = match self.exact_handoff(record, turn_id)? {
            Some(exact) => exact,
            None => match transcript.filter(|t| !t.trim().is_empty()) {
                Some(transcript) => Handoff::Summarize(transcript),
                // Nothing comes before the turn.
                None => Handoff::Carry(None),
            },
        };
        Ok(ConversationPlan { lineage, handoff })
    }

    /// The `Exact` handoff to before `turn_id`, when there is one: `record`'s
    /// provider can branch a session at a message (claude), the workspace ran
    /// the turn's session itself, so the session's transcript is where the
    /// rewound agent looks for it, and the session can still be cut there
    /// (not when it was compacted after the turn).
    fn exact_handoff(&self, record: &AgentRecord, turn_id: &str) -> Result<Option<Handoff>> {
        if !capabilities(&record.provider).branch_at_message {
            return Ok(None);
        }
        let session = self.workspace.turn_session(&record.id, turn_id)?;
        let (true, Some(from_session), Some(prompt)) =
            (session.own, session.provider_session_id, session.prompt)
        else {
            return Ok(None);
        };
        let bodies = self.workspace.bodies_from_turn(turn_id)?;
        match claude_branch_before(&from_session, &bodies, &prompt) {
            Ok(Some(point)) => Ok(Some(Handoff::Branch(point))),
            Ok(None) => Ok(Some(Handoff::Carry(session.handoff_context))),
            Err(e) => {
                tracing::info!(agent_id = %record.id, error = %e, "can't rewind natively; summarizing");
                Ok(None)
            }
        }
    }

    /// Rewind `record`'s conversation as `plan` says, then launch its agent.
    /// An error means the conversation wasn't rewound. Once it is, the launch
    /// is the agent's own outcome, as a spawn's is: a failure leaves the agent
    /// in error, and resuming it launches the rewound session.
    async fn rewind_conversation(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        record: &AgentRecord,
        plan: ConversationPlan,
    ) -> Result<()> {
        let branch = match &plan.handoff {
            Handoff::Branch(point) => Some(point),
            _ => None,
        };
        self.start_rewound_session(ctx, record, &plan.lineage, branch)
            .await?;
        if let Err(e) = self.launch_rewound_session(ctx, record, plan.handoff).await {
            tracing::warn!(agent_id = %record.id, error = %e, "rewound agent failed to start");
            fail_spawn(self, ctx, &record.id, e.to_string());
        }
        Ok(())
    }

    /// Replace `record`'s session with one that continues `lineage`, branched
    /// at `branch` when given, and leave the agent `Spawning` for its launch.
    /// Under the lifecycle lock, as a spawn creates its agent: project
    /// deletion either goes first or finds the agent `Spawning` and waits the
    /// launch out.
    async fn start_rewound_session(
        &self,
        ctx: &Arc<EngineCtx>,
        record: &AgentRecord,
        lineage: &SessionLineage,
        branch: Option<&BranchPoint>,
    ) -> Result<()> {
        {
            let _lifecycle = self.agent_lifecycle.lock().await;
            if self.deleting_projects.lock().contains(&record.project_id) {
                return Err(Error::Other("project deletion is in progress".into()));
            }
            self.start_session(&record.id, lineage, branch)?;
            self.set_status(ctx, &record.id, AgentStatus::Spawning, None);
        }
        // The session, and with it the history the chat shows, changed.
        emit_workspace_changed(ctx.sink.as_ref());
        Ok(())
    }

    /// Brief the rewound session's agent as `handoff` says, then start it.
    async fn launch_rewound_session(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        record: &AgentRecord,
        handoff: Handoff,
    ) -> Result<()> {
        if let Some(context) = self.brief(ctx, record, handoff).await {
            self.workspace.set_handoff_context(&record.id, &context)?;
        }
        emit_spawn_progress(ctx.sink.as_ref(), &record.id, SpawnStage::Starting, None);
        arm_spawn_timeout(self.clone(), ctx.clone(), record.id.clone());
        // Fresh, or the branch the session was started with
        // (`start_process` launches an unlanded one).
        self.start_process(ctx, &record.id, SessionStart::Fresh)
            .await
    }

    /// The handoff context `handoff` gives the rewound session's agent, if
    /// any. A summary is written as a fork's is, in the Summarizing stage,
    /// and falls back to the transcript's tail (`handoff::context`).
    async fn brief(
        &self,
        ctx: &Arc<EngineCtx>,
        record: &AgentRecord,
        handoff: Handoff,
    ) -> Option<String> {
        match handoff {
            Handoff::Branch(_) => None,
            Handoff::Carry(context) => context,
            Handoff::Summarize(transcript) => {
                emit_spawn_progress(ctx.sink.as_ref(), &record.id, SpawnStage::Summarizing, None);
                Some(
                    crate::handoff::context(&record.provider, record.model.as_deref(), &transcript)
                        .await,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use serde_json::{json, Value};

    use super::*;
    use crate::host::ctx::test_ctx;
    use crate::host::sink::RecordingSink;
    use crate::supervisor::tests::{committed_repo, record_in_checkouts, test_supervisor};
    use crate::supervisor::ArchiveTrigger;

    const AGENT: &str = "denali";

    /// Workspace `id` of `provider`, working in `checkout`, continuing
    /// `lineage`.
    fn agent(
        sup: &Supervisor,
        id: &str,
        provider: &str,
        checkout: &Path,
        lineage: Option<SessionLineage>,
    ) {
        let mut record = record_in_checkouts(sup, id, &[checkout.to_path_buf()]);
        record.provider = provider.into();
        record.lineage = lineage;
        sup.workspace.add_agent(&mut record).unwrap();
    }

    fn append(sup: &Supervisor, ws: &str, records: &[(&str, &Value)]) {
        sup.workspace
            .append_session_records(ws, "claude", "transcript", None, records)
            .unwrap();
    }

    /// One exchange in `ws`, delivered as `deliver_as_turn` does (checkpoint,
    /// then the turn row) and ingested, matched: the prompt `{turn}-u`,
    /// chained to `parent` as claude chains it, and the reply `{turn}-a`.
    async fn say(sup: &Supervisor, ws: &str, turn: &str, parent: Option<&str>) {
        sup.checkpoint_turn(ws, turn).await;
        sup.workspace.insert_user_turn(ws, turn, turn, &[]).unwrap();
        let (prompt_id, reply_id) = (format!("{turn}-u"), format!("{turn}-a"));
        let prompt = json!({"type": "user", "uuid": prompt_id, "parentUuid": parent,
                            "message": {"content": turn}});
        let reply = json!({"type": "assistant", "uuid": reply_id, "parentUuid": prompt_id});
        append(sup, ws, &[(&prompt_id, &prompt), (&reply_id, &reply)]);
        sup.workspace.associate_pending_user_turns(ws).unwrap();
    }

    fn edit(checkout: &Path, text: &str) {
        std::fs::write(checkout.join("a.txt"), text).unwrap();
    }

    fn code(checkout: &Path) -> String {
        std::fs::read_to_string(checkout.join("a.txt")).unwrap()
    }

    /// `denali` (claude) said t1, then t2, editing its code after each.
    async fn talked(dir: &Path) -> (Arc<Supervisor>, PathBuf) {
        let sup = Arc::new(test_supervisor());
        let checkout = committed_repo(dir, AGENT).await;
        agent(&sup, AGENT, "claude", &checkout, None);
        say(&sup, AGENT, "t1", None).await;
        edit(&checkout, "after t1");
        say(&sup, AGENT, "t2", Some("t1-a")).await;
        edit(&checkout, "after t2");
        (sup, checkout)
    }

    fn plan(sup: &Supervisor, ws: &str, turn: &str, transcript: Option<&str>) -> ConversationPlan {
        let record = sup.workspace.agent(ws).unwrap();
        sup.plan_conversation(&record, turn, transcript.map(str::to_string))
            .unwrap()
    }

    fn handoff(sup: &Supervisor, ws: &str, turn: &str, transcript: Option<&str>) -> Handoff {
        plan(sup, ws, turn, transcript).handoff
    }

    fn provider_session(sup: &Supervisor, ws: &str) -> String {
        sup.workspace.agent(ws).unwrap().session_id.unwrap()
    }

    fn branch_at(from_session: String, message: &str) -> Handoff {
        Handoff::Branch(BranchPoint {
            from_session,
            at_message: Some(message.into()),
        })
    }

    fn compaction(sup: &Supervisor, ws: &str) {
        let boundary = json!({"type": "system", "subtype": "compact_boundary", "uuid": "c1"});
        append(sup, ws, &[("c1", &boundary)]);
    }

    // ── Exact or Summary ──────────────────────────────────────────────────

    #[tokio::test]
    async fn claude_branches_the_session_that_ran_the_turn_just_before_it() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;

        let plan = plan(&sup, AGENT, "t2", Some("User: t1"));

        assert_eq!(
            plan.lineage,
            sup.workspace
                .resolve_anchor(AGENT, Anchor::Before("t2"))
                .unwrap()
        );
        assert_eq!(
            plan.handoff,
            branch_at(provider_session(&sup, AGENT), "t1-a")
        );
    }

    #[tokio::test]
    async fn a_turn_that_opened_its_session_starts_fresh_as_that_session_did() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        assert_eq!(handoff(&sup, AGENT, "t1", None), Handoff::Carry(None));

        sup.workspace
            .set_handoff_context(AGENT, "what came before")
            .unwrap();
        assert_eq!(
            handoff(&sup, AGENT, "t1", Some("User: earlier")),
            Handoff::Carry(Some("what came before".into()))
        );
    }

    #[tokio::test]
    async fn a_compaction_after_the_turn_falls_back_to_a_summary() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        compaction(&sup, AGENT);

        assert_eq!(
            handoff(&sup, AGENT, "t2", Some("User: t1")),
            Handoff::Summarize("User: t1".into())
        );
        // With no transcript to summarize, nothing is told.
        assert_eq!(
            handoff(&sup, AGENT, "t2", Some(" \n")),
            Handoff::Carry(None)
        );
        // A compaction before the turn is no obstacle.
        say(&sup, AGENT, "t3", Some("c1")).await;
        assert_eq!(
            handoff(&sup, AGENT, "t3", None),
            branch_at(provider_session(&sup, AGENT), "c1")
        );
    }

    #[tokio::test]
    async fn a_provider_that_cant_branch_summarizes() {
        let td = tempfile::tempdir().unwrap();
        let sup = test_supervisor();
        let checkout = committed_repo(td.path(), "rainier").await;
        agent(&sup, "rainier", "codex", &checkout, None);
        say(&sup, "rainier", "r1", None).await;
        say(&sup, "rainier", "r2", Some("r1-a")).await;

        assert_eq!(
            handoff(&sup, "rainier", "r2", Some("User: r1")),
            Handoff::Summarize("User: r1".into())
        );
    }

    #[tokio::test]
    async fn a_turn_inherited_from_another_workspace_summarizes_and_has_no_code() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        let fork = committed_repo(td.path(), "fuji").await;
        let lineage = sup
            .workspace
            .resolve_anchor(AGENT, Anchor::Through("t1"))
            .unwrap();
        agent(&sup, "fuji", "claude", &fork, Some(lineage));
        say(&sup, "fuji", "f1", None).await;

        // t1 ran in `denali`: `fuji`'s agent can't resume that session, nor
        // can its checkouts restore that code.
        assert_eq!(
            handoff(&sup, "fuji", "t1", Some("User: t1")),
            Handoff::Summarize("User: t1".into())
        );
        let err = sup
            .preview_rewind_code("fuji", "t1")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("another workspace"), "{err}");
        // Its own turn has both.
        assert_eq!(handoff(&sup, "fuji", "f1", None), Handoff::Carry(None));
        assert!(sup.preview_rewind_code("fuji", "f1").await.is_ok());
    }

    /// `denali` after t3, rewound to before t3, its first session left
    /// behind as an ancestor.
    fn rewound_before_t3(sup: &Supervisor) {
        let before_t3 = sup
            .workspace
            .resolve_anchor(AGENT, Anchor::Before("t3"))
            .unwrap();
        sup.workspace
            .start_session(AGENT, &before_t3, None)
            .unwrap();
    }

    #[tokio::test]
    async fn a_session_the_workspace_rewound_away_from_is_still_its_own() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        say(&sup, AGENT, "t3", Some("t2-a")).await;
        let first = provider_session(&sup, AGENT);
        rewound_before_t3(&sup);
        say(&sup, AGENT, "n1", None).await;

        assert_eq!(handoff(&sup, AGENT, "t2", None), branch_at(first, "t1-a"));
        assert!(sup.preview_rewind_code(AGENT, "t2").await.is_ok());
    }

    #[tokio::test]
    async fn a_compaction_a_rewind_left_behind_still_counts() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        say(&sup, AGENT, "t3", Some("t2-a")).await;
        compaction(&sup, AGENT);
        rewound_before_t3(&sup);

        // The history no longer shows the compaction, but a resume of the
        // first session would still load only what follows it.
        assert_eq!(
            handoff(&sup, AGENT, "t2", Some("User: t1")),
            Handoff::Summarize("User: t1".into())
        );
    }

    // ── The steps ─────────────────────────────────────────────────────────

    /// The events in `sink` as `(name, the field that tells them apart)`.
    fn events(sink: &RecordingSink) -> Vec<(String, Value)> {
        sink.events()
            .into_iter()
            .map(|(name, payload)| {
                let detail = match name.as_str() {
                    "agent:status" => payload["status"].clone(),
                    "agent:spawn-progress" => payload["stage"].clone(),
                    _ => Value::Null,
                };
                (name, detail)
            })
            .collect()
    }

    #[tokio::test]
    async fn an_exact_rewind_starts_its_session_as_a_branch_and_briefs_nothing() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        let (ctx, sink, _dir) = test_ctx();
        let record = sup.workspace.agent(AGENT).unwrap();
        let ConversationPlan { lineage, handoff } = plan(&sup, AGENT, "t2", None);
        let Handoff::Branch(point) = &handoff else {
            panic!("{handoff:?}")
        };

        sup.start_rewound_session(&ctx, &record, &lineage, Some(point))
            .await
            .unwrap();

        let rewound = sup.workspace.agent(AGENT).unwrap();
        assert_eq!(rewound.lineage, Some(lineage));
        assert_ne!(rewound.session_id, record.session_id);
        assert_eq!(
            sup.workspace.session_branch_point(AGENT).unwrap().as_ref(),
            Some(point)
        );
        assert_eq!(sup.status_of(AGENT), Some(AgentStatus::Spawning));
        assert_eq!(sup.brief(&ctx, &record, handoff).await, None);
        assert_eq!(
            events(&sink),
            [
                ("agent:status".to_string(), json!("spawning")),
                ("workspace:changed".to_string(), Value::Null),
            ]
        );
    }

    #[tokio::test]
    async fn a_summary_is_written_in_the_summarizing_stage() {
        let td = tempfile::tempdir().unwrap();
        // opencode has no tool-less one-shot, so its summary falls back to
        // the transcript's tail without running anything.
        let sup = test_supervisor();
        let checkout = committed_repo(td.path(), "rainier").await;
        agent(&sup, "rainier", "opencode", &checkout, None);
        let (ctx, sink, _dir) = test_ctx();
        let record = sup.workspace.agent("rainier").unwrap();
        let transcript = "User: r1\n\nAssistant: done";

        let context = sup
            .brief(&ctx, &record, Handoff::Summarize(transcript.into()))
            .await
            .unwrap();

        assert!(context.ends_with(transcript), "{context}");
        assert_eq!(
            events(&sink),
            [("agent:spawn-progress".to_string(), json!("summarizing"))]
        );
    }

    #[tokio::test]
    async fn nothing_changes_while_a_turn_runs_or_outside_the_chat_view() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = talked(td.path()).await;
        let (ctx, sink, _dir) = test_ctx();
        let session = sup.workspace.agent(AGENT).unwrap().session_id;
        let refused = |err: Error, why: &str| {
            let err = err.to_string();
            assert!(err.contains(why), "{err}");
        };

        for busy in [AgentStatus::Running, AgentStatus::Spawning] {
            sup.statuses.lock().insert(AGENT.into(), busy);
            let err = sup
                .rewind(&ctx, AGENT, "t2", RewindScope::Both, None)
                .await
                .unwrap_err();
            refused(err, "Stop the agent");
        }
        sup.statuses.lock().remove(AGENT);
        sup.workspace
            .update_agent_view(AGENT, AgentView::Native)
            .unwrap();
        let err = sup
            .rewind(&ctx, AGENT, "t2", RewindScope::Code, None)
            .await
            .unwrap_err();
        refused(err, "chat view");

        assert_eq!(code(&checkout), "after t2");
        assert_eq!(sup.workspace.agent(AGENT).unwrap().session_id, session);
        assert!(sink.events().is_empty());
    }

    #[tokio::test]
    async fn the_code_alone_rewinds_and_its_undo_puts_it_back() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = talked(td.path()).await;
        let (ctx, _sink, _dir) = test_ctx();
        let session = sup.workspace.agent(AGENT).unwrap().session_id;

        let outcome = sup
            .rewind(&ctx, AGENT, "t2", RewindScope::Code, None)
            .await
            .unwrap();

        assert_eq!(code(&checkout), "after t1");
        assert_eq!(outcome.conversation_error, None);
        assert_eq!(sup.workspace.agent(AGENT).unwrap().session_id, session);
        let report = outcome.code.unwrap();
        // Only an undo point the restore made is restored from.
        let mut forged = report.clone();
        forged.repos[0].undo_ref = Some("refs/heads/main".into());
        assert!(sup.undo_code_restore(AGENT, &forged).await.is_err());
        assert_eq!(code(&checkout), "after t1");

        sup.undo_code_restore(AGENT, &report).await.unwrap();
        assert_eq!(code(&checkout), "after t2");
    }

    #[tokio::test]
    async fn what_cant_be_rewound_is_refused_before_anything_changes() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = talked(td.path()).await;
        let (ctx, _sink, _dir) = test_ctx();
        // t3 went out without a checkpoint (mid-turn, or before checkpoints)
        // and hasn't synced.
        sup.workspace
            .insert_user_turn(AGENT, "t3", "t3", &[])
            .unwrap();
        let session = sup.workspace.agent(AGENT).unwrap().session_id;
        let refusal = |scope| {
            let (sup, ctx) = (sup.clone(), ctx.clone());
            async move {
                sup.rewind(&ctx, AGENT, "t3", scope, None)
                    .await
                    .unwrap_err()
                    .to_string()
            }
        };

        let err = refusal(RewindScope::Both).await;
        assert!(err.contains("still syncing"), "{err}");
        let err = refusal(RewindScope::Code).await;
        assert!(err.contains("no snapshot"), "{err}");

        assert_eq!(code(&checkout), "after t2");
        assert_eq!(sup.workspace.agent(AGENT).unwrap().session_id, session);
    }

    #[tokio::test]
    async fn a_conversation_that_cant_follow_the_code_is_reported_with_its_undo() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = talked(td.path()).await;
        let (ctx, sink, _dir) = test_ctx();
        let record = sup.workspace.agent(AGENT).unwrap();
        // A project deletion that started while the code was restored: the
        // session switch, checked under the lifecycle lock, refuses.
        sup.deleting_projects
            .lock()
            .insert(record.project_id.clone());

        let outcome = sup
            .rewind(&ctx, AGENT, "t2", RewindScope::Both, None)
            .await
            .unwrap();

        let err = outcome.conversation_error.unwrap();
        assert!(err.contains("The code was restored"), "{err}");
        assert!(err.contains("project deletion"), "{err}");
        assert_eq!(code(&checkout), "after t1");
        assert_eq!(
            sup.workspace.agent(AGENT).unwrap().session_id,
            record.session_id
        );
        assert!(sink.events().is_empty(), "no session switch was announced");
        sup.undo_code_restore(AGENT, &outcome.code.unwrap())
            .await
            .unwrap();
        assert_eq!(code(&checkout), "after t2");
    }

    #[tokio::test]
    async fn a_rewind_waits_out_a_delivery_and_never_runs_into_an_archive() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = talked(td.path()).await;
        let (ctx, _sink, _dir) = test_ctx();

        // An archive holds the agent: the rewind is refused.
        {
            let record = sup.workspace.agent(AGENT).unwrap();
            let _archiving = sup
                .reserve_disposal(AGENT, &record, ArchiveTrigger::User)
                .unwrap();
            let err = sup
                .rewind(&ctx, AGENT, "t2", RewindScope::Code, None)
                .await
                .unwrap_err();
            assert!(err.to_string().contains("being archived"), "{err}");
        }

        // A delivery is in flight: the rewind waits for it before touching
        // anything.
        let delivering = sup.lock_delivery(AGENT).await;
        let rewind = tokio::spawn({
            let (sup, ctx) = (sup.clone(), ctx.clone());
            async move { sup.rewind(&ctx, AGENT, "t2", RewindScope::Code, None).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!rewind.is_finished());
        assert_eq!(code(&checkout), "after t2");

        drop(delivering);
        rewind.await.unwrap().unwrap();
        assert_eq!(code(&checkout), "after t1");
    }

    #[test]
    fn scopes_deserialize_from_their_wire_names() {
        for (wire, scope) in [
            ("conversation", RewindScope::Conversation),
            ("code", RewindScope::Code),
            ("both", RewindScope::Both),
        ] {
            assert_eq!(
                serde_json::from_value::<RewindScope>(json!(wire)).unwrap(),
                scope
            );
        }
    }
}
