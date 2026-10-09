//! Rewinding an agent's workspace to just before one of its messages, in
//! place: the conversation, the code, or both (docs/fork-and-rewind.md,
//! "Rewind"). A rewind is a fork into the same workspace, made of the pieces
//! fork and the session switch already use:
//!
//!  - **Conversation** — a new session continues the history before the turn
//!    (`resolve_anchor(Before)`, `start_session`). Its agent knows that
//!    history natively when it can (`Exact`: the history is written as the new
//!    session's own transcript, which its CLI resumes), or from a summary, as
//!    a fork's agent does (`Summary`).
//!  - **Code** — every checkout goes back to the turn's checkpoint, behind an
//!    undo point (`restore_turn_code`).
//!
//! Orchestration only. The steps run under the agent's delivery lock and
//! input route, so no message starts a turn and no archive takes the
//! workspace halfway through.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::workspace::{AgentRecord, AgentStatus, Anchor, NativeTranscript, SessionLineage};

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
    /// What the code restore did; `None` when the code wasn't rewound. Its
    /// undo point is the backend's ([`Supervisor::undo_code_restore`]).
    pub code: Option<RestoreReport>,
    /// Why the conversation couldn't be rewound after the code was. The code
    /// stays restored, and can be undone, so this is reported alongside it
    /// rather than as the rewind's error.
    pub conversation_error: Option<String>,
}

/// How the rewound session's agent learns the conversation before the turn.
#[derive(Debug, PartialEq, Eq)]
enum Handoff {
    /// `Exact`: `history` is written as the new session's own transcript,
    /// which its CLI resumes, and its agent is told `context`, what the
    /// turn's session's agent was told (nothing for a workspace's first
    /// session). With no history to write, it starts fresh, told that.
    Native {
        history: Vec<Value>,
        context: Option<String>,
    },
    /// `Summary`: a summary of this transcript of the conversation before the
    /// turn, rendered by the client.
    Summarize(String),
    /// `Summary` of nothing: no conversation comes before the turn.
    Nothing,
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
    /// Refused while a turn runs ("stop the agent first") and for a workflow
    /// step. Everything that can refuse is checked before anything changes,
    /// the code's checkpoint included; then the code goes first and the
    /// conversation second. A conversation that can't be rewound after the
    /// code was restored is reported in the outcome, with the code's undo.
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
    /// fork's parent), whose checkouts hold its checkpoints, or no checkout
    /// kept a checkpoint of it. When only some did, the others are in the
    /// report without one, for the confirmation to name: they stay as they
    /// are. A checkpoint that can't be looked up is an error, never taken for
    /// a missing one.
    pub async fn preview_rewind_code(
        &self,
        agent_id: &str,
        turn_id: &str,
    ) -> Result<RestoreReport> {
        if !self.workspace.turn_is_own(agent_id, turn_id)? {
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

    /// Why `record` can't be rewound now, if it can't.
    fn check_rewindable(&self, record: &AgentRecord) -> Result<()> {
        let refusal = if record.archive.is_some() {
            "agent is archived"
        } else if record.owner_run_id.is_some() {
            "A workflow step's conversation belongs to its run."
        } else if self.is_busy(&record.id) {
            "Stop the agent before rewinding."
        } else {
            return Ok(());
        };
        Err(Error::Other(refusal.into()))
    }

    /// Where `record`'s rewound conversation starts — the history before
    /// `turn_id`'s prompt — and how its agent learns that history: natively
    /// when its provider can continue it (`Exact`), else from a summary.
    fn plan_conversation(
        &self,
        record: &AgentRecord,
        turn_id: &str,
        transcript: Option<String>,
    ) -> Result<ConversationPlan> {
        let lineage = self
            .workspace
            .resolve_anchor(&record.id, Anchor::Before(turn_id))?;
        let handoff = match self.native_history(&record.provider, &lineage)? {
            Ok(history) => Handoff::Native {
                history,
                context: self
                    .workspace
                    .session_handoff_context(&lineage.parent_session_id)?,
            },
            Err(why) => {
                tracing::info!(agent_id = %record.id, %why, "can't rewind natively; summarizing");
                match transcript.filter(|t| !t.trim().is_empty()) {
                    Some(transcript) => Handoff::Summarize(transcript),
                    None => Handoff::Nothing,
                }
            }
        };
        Ok(ConversationPlan { lineage, handoff })
    }

    /// Rewind `record`'s conversation as `plan` says, then launch its agent.
    /// An error means the conversation wasn't rewound: an `Exact` history is
    /// written as the new session's transcript before the switch, so a
    /// failure to write it changes nothing. Once it is rewound, the launch is
    /// the agent's own outcome, as a spawn's is: a failure leaves the agent in
    /// error, and resuming it launches the rewound session.
    async fn rewind_conversation(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        record: &AgentRecord,
        plan: ConversationPlan,
    ) -> Result<()> {
        let native = match &plan.handoff {
            Handoff::Native { history, .. } => {
                let session = uuid::Uuid::new_v4().to_string();
                self.write_native_transcript(record, &session, history)?
            }
            _ => None,
        };
        self.start_rewound_session(ctx, record, &plan.lineage, native.as_ref())
            .await?;
        if let Err(e) = self.launch_rewound_session(ctx, record, plan.handoff).await {
            tracing::warn!(agent_id = %record.id, error = %e, "rewound agent failed to start");
            fail_spawn(self, ctx, &record.id, e.to_string());
        }
        Ok(())
    }

    /// Replace `record`'s session with one that continues `lineage`, as the
    /// provider session `native` was written as when given, and leave the
    /// agent `Spawning` for its launch. Under the lifecycle lock, as a spawn
    /// creates its agent: project deletion either goes first or finds the
    /// agent `Spawning` and waits the launch out.
    async fn start_rewound_session(
        &self,
        ctx: &Arc<EngineCtx>,
        record: &AgentRecord,
        lineage: &SessionLineage,
        native: Option<&NativeTranscript>,
    ) -> Result<()> {
        {
            let _lifecycle = self.agent_lifecycle.lock().await;
            if self.deleting_projects.lock().contains(&record.project_id) {
                return Err(Error::Other("project deletion is in progress".into()));
            }
            self.start_session(&record.id, lineage, native)?;
            self.set_status(ctx, &record.id, AgentStatus::Spawning, None);
        }
        // The session, and with it the history the chat shows, changed.
        emit_workspace_changed(ctx.sink.as_ref());
        Ok(())
    }

    /// Brief the rewound session's agent as `handoff` says, then start it:
    /// resuming its written transcript, or fresh.
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
        // Ahead of the spawn watchdog; kept for the launch (`prefetch_login`).
        let _ = self.prefetch_login(&record.id).await;
        arm_spawn_timeout(self.clone(), ctx.clone(), record.id.clone());
        self.start_process(ctx, &record.id).await
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
            Handoff::Native { context, .. } => context,
            Handoff::Nothing => None,
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
    use crate::workspace::tests::test_prompts;

    const AGENT: &str = "denali";

    /// Workspace `id` of `provider`, working in `checkout`, continuing
    /// `lineage`. In a container sandbox, so a claude transcript written for
    /// it goes to the per-agent projects dir beside the checkout.
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
        record.sandbox_engine = Some("docker".into());
        sup.workspace.add_agent(&mut record).unwrap();
    }

    fn append(sup: &Supervisor, ws: &str, records: &[(&str, &Value)]) {
        let provider = sup.workspace.agent(ws).unwrap().provider;
        sup.workspace
            .append_session_records(ws, &provider, "transcript", None, records)
            .unwrap();
    }

    /// One exchange in `ws`, delivered as `deliver_as_turn` does (checkpoint,
    /// then the turn row) and ingested, matched: the prompt `{turn}-u`,
    /// chained to `parent` as claude chains it, and the reply `{turn}-a`.
    async fn say(sup: &Supervisor, ws: &str, turn: &str, parent: Option<&str>) {
        sup.checkpoint_turn(ws, turn).await;
        sup.workspace.insert_user_turn(ws, turn, turn, &[]).unwrap();
        let (prompt_id, reply_id) = (format!("{turn}-u"), format!("{turn}-a"));
        append(
            sup,
            ws,
            &[
                (&prompt_id, &prompt(turn, parent)),
                (&reply_id, &reply(turn)),
            ],
        );
        sup.workspace
            .associate_pending_user_turns(ws, test_prompts)
            .unwrap();
    }

    fn prompt(turn: &str, parent: Option<&str>) -> Value {
        json!({"type": "user", "uuid": format!("{turn}-u"), "parentUuid": parent,
               "sessionId": "old", "message": {"content": turn}})
    }

    fn reply(turn: &str) -> Value {
        json!({"type": "assistant", "uuid": format!("{turn}-a"), "parentUuid": format!("{turn}-u"),
               "sessionId": "old"})
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

    /// The `Exact` handoff that writes the records `ids` (in `history`'s
    /// order) and tells nothing more.
    fn native(sup: &Supervisor, ws: &str, ids: &[&str]) -> Handoff {
        let bodies = sup.workspace.read_history_records(ws).unwrap();
        let history = ids
            .iter()
            .map(|id| {
                bodies
                    .iter()
                    .find(|r| r.native_id == *id)
                    .unwrap_or_else(|| panic!("no record {id}"))
                    .body
                    .clone()
            })
            .collect();
        Handoff::Native {
            history,
            context: None,
        }
    }

    fn compaction() -> Value {
        json!({"type": "system", "subtype": "compact_boundary", "uuid": "c1", "parentUuid": null,
               "logicalParentUuid": "t2-a", "sessionId": "old"})
    }

    // ── Exact or Summary ──────────────────────────────────────────────────

    #[tokio::test]
    async fn the_history_before_the_turn_is_continued_natively() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;

        let plan = plan(&sup, AGENT, "t2", Some("User: t1"));

        assert_eq!(
            plan.lineage,
            sup.workspace
                .resolve_anchor(AGENT, Anchor::Before("t2"))
                .unwrap()
        );
        assert_eq!(plan.handoff, native(&sup, AGENT, &["t1-u", "t1-a"]));
    }

    #[tokio::test]
    async fn a_turn_that_opened_the_conversation_starts_fresh_as_its_session_did() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        assert_eq!(handoff(&sup, AGENT, "t1", None), native(&sup, AGENT, &[]));

        sup.workspace
            .set_handoff_context(AGENT, "what came before")
            .unwrap();
        assert_eq!(
            handoff(&sup, AGENT, "t1", Some("User: earlier")),
            Handoff::Native {
                history: vec![],
                context: Some("what came before".into()),
            }
        );
    }

    /// The copied lines hold the provider's own compaction records, so a cut
    /// on either side of one needs nothing special.
    #[tokio::test]
    async fn a_cut_around_a_compaction_copies_what_the_cli_wrote() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        append(&sup, AGENT, &[("c1", &compaction())]);
        say(&sup, AGENT, "t3", Some("c1")).await;

        assert_eq!(
            handoff(&sup, AGENT, "t3", None),
            native(&sup, AGENT, &["t1-u", "t1-a", "t2-u", "t2-a", "c1"])
        );
        assert_eq!(
            handoff(&sup, AGENT, "t2", Some("User: t1")),
            native(&sup, AGENT, &["t1-u", "t1-a"])
        );
    }

    #[tokio::test]
    async fn a_provider_fletch_cant_write_for_summarizes() {
        let td = tempfile::tempdir().unwrap();
        let sup = test_supervisor();
        let checkout = committed_repo(td.path(), "rainier").await;
        agent(&sup, "rainier", "cursor", &checkout, None);
        say(&sup, "rainier", "r1", None).await;
        say(&sup, "rainier", "r2", Some("r1-a")).await;

        assert_eq!(
            handoff(&sup, "rainier", "r2", Some("User: r1")),
            Handoff::Summarize("User: r1".into())
        );
        // With no transcript to summarize, nothing is told.
        assert_eq!(
            handoff(&sup, "rainier", "r2", Some(" \n")),
            Handoff::Nothing
        );
    }

    /// A history part of which another provider wrote can't be continued as
    /// this one's own transcript.
    #[tokio::test]
    async fn a_history_partly_another_providers_summarizes() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        let foreign = json!({"type": "response_item", "payload": {"type": "message"}});
        sup.workspace
            .append_session_records(AGENT, "codex", "transcript", None, &[("ln:9", &foreign)])
            .unwrap();
        say(&sup, AGENT, "t3", Some("t2-a")).await;

        assert_eq!(
            handoff(&sup, AGENT, "t3", Some("User: t1")),
            Handoff::Summarize("User: t1".into())
        );
    }

    /// Only the main transcript is copied: a sub-agent's records, ingested
    /// from files of their own, and records compiled from the live stream
    /// aren't lines of it.
    #[tokio::test]
    async fn only_the_main_transcript_is_copied() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        let sub = json!({"type": "assistant", "uuid": "s1", "isSidechain": true,
                         "parent_tool_use_id": "toolu_1"});
        let live = json!({"type": "usage", "tokens": 12});
        append(&sup, AGENT, &[("s1", &sub)]);
        sup.workspace
            .append_session_records(AGENT, "claude", "live_compiled", None, &[("live-1", &live)])
            .unwrap();
        say(&sup, AGENT, "t3", Some("t2-a")).await;

        assert_eq!(
            handoff(&sup, AGENT, "t3", None),
            native(&sup, AGENT, &["t1-u", "t1-a", "t2-u", "t2-a"])
        );
    }

    #[tokio::test]
    async fn a_turn_inherited_from_another_workspace_continues_natively_but_has_no_code() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        let fork = committed_repo(td.path(), "fuji").await;
        let lineage = sup
            .workspace
            .resolve_anchor(AGENT, Anchor::Through("t1"))
            .unwrap();
        agent(&sup, "fuji", "claude", &fork, Some(lineage));
        say(&sup, "fuji", "f1", Some("t1-a")).await;

        // The history is copied from what Fletch stored, wherever it ran.
        assert_eq!(
            handoff(&sup, "fuji", "f1", None),
            native(&sup, "fuji", &["t1-u", "t1-a"])
        );
        // But t1's code is in `denali`'s checkouts.
        let err = sup
            .preview_rewind_code("fuji", "t1")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("another workspace"), "{err}");
        assert!(sup.preview_rewind_code("fuji", "f1").await.is_ok());
    }

    #[tokio::test]
    async fn a_session_the_workspace_rewound_away_from_is_still_in_its_history() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        say(&sup, AGENT, "t3", Some("t2-a")).await;
        let before_t3 = sup
            .workspace
            .resolve_anchor(AGENT, Anchor::Before("t3"))
            .unwrap();
        sup.workspace
            .start_session(AGENT, &before_t3, None)
            .unwrap();
        say(&sup, AGENT, "n1", Some("t2-a")).await;

        assert_eq!(
            handoff(&sup, AGENT, "n1", None),
            native(&sup, AGENT, &["t1-u", "t1-a", "t2-u", "t2-a"])
        );
        assert!(sup.preview_rewind_code(AGENT, "t2").await.is_ok());
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

    /// `denali`'s current session's transcript file, where claude looks for
    /// it from the checkout: the container's per-agent projects dir.
    fn transcript_file(sup: &Supervisor, td: &Path) -> PathBuf {
        let record = sup.workspace.agent(AGENT).unwrap();
        let checkout = td.join(AGENT);
        td.join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME)
            .join(crate::transcripts::claude_project_dirname(&checkout).unwrap())
            .join(format!("{}.jsonl", record.session_id.unwrap()))
    }

    fn append_lines(path: &Path, lines: &[Value]) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    /// An `Exact` rewind past a compaction, step by step: the history is
    /// written as the new session's transcript, which the session starts as;
    /// what the CLI appends to it is the session's own, and nothing it copied
    /// is stored twice.
    #[tokio::test]
    async fn an_exact_rewind_starts_its_session_as_the_written_history() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        append(&sup, AGENT, &[("c1", &compaction())]);
        say(&sup, AGENT, "t3", Some("c1")).await;
        let (ctx, sink, _dir) = test_ctx();
        let record = sup.workspace.agent(AGENT).unwrap();
        let ConversationPlan { lineage, handoff } = plan(&sup, AGENT, "t3", None);
        let Handoff::Native { history, .. } = &handoff else {
            panic!("{handoff:?}")
        };
        let session = uuid::Uuid::new_v4().to_string();

        let native = sup
            .write_native_transcript(&record, &session, history)
            .unwrap()
            .unwrap();
        sup.start_rewound_session(&ctx, &record, &lineage, Some(&native))
            .await
            .unwrap();

        let rewound = sup.workspace.agent(AGENT).unwrap();
        assert_eq!(rewound.lineage, Some(lineage));
        assert_eq!(rewound.session_id.as_deref(), Some(session.as_str()));
        assert_eq!(native.prefix, 5);
        assert_eq!(sup.workspace.session_transcript_prefix(AGENT).unwrap(), 5);
        assert_eq!(sup.status_of(AGENT), Some(AgentStatus::Spawning));
        assert_eq!(sup.brief(&ctx, &record, handoff).await, None);
        assert_eq!(
            events(&sink),
            [
                ("agent:status".to_string(), json!("spawning")),
                ("workspace:changed".to_string(), Value::Null),
            ]
        );
        // The file claude resumes: the history, with the new session's id.
        let file = transcript_file(&sup, td.path());
        let written: Vec<Value> = std::fs::read_to_string(&file)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            written
                .iter()
                .map(|l| l["uuid"].clone())
                .collect::<Vec<_>>(),
            ["t1-u", "t1-a", "t2-u", "t2-a", "c1"]
        );
        assert!(written.iter().all(|l| l["sessionId"] == session.as_str()));

        // Claude resumes it and answers a new prompt: only that is stored.
        append_lines(&file, &[prompt("n1", Some("c1")), reply("n1")]);
        sup.workspace
            .insert_user_turn(AGENT, "n1", "n1", &[])
            .unwrap();
        sup.sync_session(AGENT);
        let own: Vec<String> = sup
            .workspace
            .read_session_records(AGENT)
            .unwrap()
            .into_iter()
            .map(|r| r.native_id)
            .collect();
        assert_eq!(own, ["n1-u", "n1-a"]);
        let history: Vec<(String, bool)> = sup
            .workspace
            .read_history_records(AGENT)
            .unwrap()
            .into_iter()
            .map(|r| (r.native_id, r.inherited))
            .collect();
        assert_eq!(
            history,
            [
                ("t1-u".to_string(), true),
                ("t1-a".to_string(), true),
                ("t2-u".to_string(), true),
                ("t2-a".to_string(), true),
                ("c1".to_string(), true),
                ("n1-u".to_string(), false),
                ("n1-a".to_string(), false),
            ]
        );
        let turns = sup.workspace.read_history_turns(AGENT).unwrap();
        assert_eq!(turns.last().unwrap().native_id.as_deref(), Some("n1-u"));
    }

    /// A rewind whose history can't be written changes nothing.
    #[tokio::test]
    async fn a_history_that_cant_be_written_leaves_the_conversation_as_it_was() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = talked(td.path()).await;
        let (ctx, sink, _dir) = test_ctx();
        let record = sup.workspace.agent(AGENT).unwrap();
        // Something already sits where the projects dir goes.
        std::fs::write(
            td.path()
                .join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME),
            "",
        )
        .unwrap();

        let plan = plan(&sup, AGENT, "t2", None);
        assert!(sup.rewind_conversation(&ctx, &record, plan).await.is_err());

        assert_eq!(
            sup.workspace.agent(AGENT).unwrap().session_id,
            record.session_id
        );
        assert!(sink.events().is_empty());
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
    async fn nothing_changes_while_a_turn_runs() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = talked(td.path()).await;
        let (ctx, sink, _dir) = test_ctx();
        let session = sup.workspace.agent(AGENT).unwrap().session_id;

        for busy in [AgentStatus::Running, AgentStatus::Spawning] {
            sup.statuses.lock().insert(AGENT.into(), busy);
            let err = sup
                .rewind(&ctx, AGENT, "t2", RewindScope::Both, None)
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains("Stop the agent"), "{err}");
        }

        assert_eq!(code(&checkout), "after t2");
        assert_eq!(sup.workspace.agent(AGENT).unwrap().session_id, session);
        assert!(sink.events().is_empty());
    }

    /// The native view is no obstacle: what it resumes is a transcript like
    /// any other.
    #[tokio::test]
    async fn the_native_view_rewinds_too() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = talked(td.path()).await;
        let (ctx, _sink, _dir) = test_ctx();
        sup.workspace
            .update_agent_view(AGENT, crate::workspace::AgentView::Native)
            .unwrap();

        sup.rewind(&ctx, AGENT, "t2", RewindScope::Code, None)
            .await
            .unwrap();

        assert_eq!(code(&checkout), "after t1");
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
        assert!(outcome.code.is_some());
        assert_eq!(sup.workspace.agent(AGENT).unwrap().session_id, session);

        assert!(sup.has_code_undo(AGENT).await.unwrap());
        sup.undo_code_restore(AGENT).await.unwrap();
        assert_eq!(code(&checkout), "after t2");
        assert!(!sup.has_code_undo(AGENT).await.unwrap());
    }

    /// A workspace with a checkout attached after the turn: the code it can
    /// restore is restored, and the preview names the checkout it can't.
    #[tokio::test]
    async fn a_partial_snapshot_restores_what_it_has_and_names_the_rest() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = talked(td.path()).await;
        let (ctx, _sink, _dir) = test_ctx();
        let later = committed_repo(td.path(), "later").await;
        sup.workspace.add_workspace_repo(later.clone()).unwrap();
        let attached = crate::workspace::TrackedRepo {
            repo_path: later.clone(),
            subdir: "repo-1".into(),
            adopted_checkout: Some(later.clone()),
            ..sup.workspace.agent(AGENT).unwrap().repos[0].clone()
        };
        sup.workspace.append_tracked_repo(AGENT, attached).unwrap();
        edit(&later, "after t2");

        let preview = sup.preview_rewind_code(AGENT, "t2").await.unwrap();
        let snapshots: Vec<(&str, bool)> = preview
            .repos
            .iter()
            .map(|repo| (repo.subdir.as_str(), repo.checkpoint.is_some()))
            .collect();
        assert_eq!(snapshots, [("repo-0", true), ("repo-1", false)]);

        sup.rewind(&ctx, AGENT, "t2", RewindScope::Code, None)
            .await
            .unwrap();
        assert_eq!(code(&checkout), "after t1");
        assert_eq!(code(&later), "after t2");
    }

    /// A checkpoint that can't be looked up is an error of its own, never
    /// mistaken for one that wasn't kept.
    #[tokio::test]
    async fn a_failed_lookup_is_not_a_missing_snapshot() {
        let td = tempfile::tempdir().unwrap();
        let (sup, checkout) = talked(td.path()).await;
        std::fs::remove_dir_all(&checkout).unwrap();

        let err = sup
            .preview_rewind_code(AGENT, "t2")
            .await
            .unwrap_err()
            .to_string();

        assert!(!err.contains("no snapshot"), "{err}");
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
        assert!(sup.has_code_undo(AGENT).await.unwrap());
        sup.undo_code_restore(AGENT).await.unwrap();
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
