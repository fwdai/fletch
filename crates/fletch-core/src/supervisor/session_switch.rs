//! Starting a new session in an agent's workspace, in place — rewind's
//! conversation half, without the orchestration (docs/fork-and-rewind.md).

use crate::agent::{capabilities, BranchPoint};
use crate::error::{Error, Result};
use crate::workspace::SessionLineage;

use super::Supervisor;

impl Supervisor {
    /// Replace `agent_id`'s current session with a new one that continues
    /// `lineage`, natively branched at `branch` when given (see
    /// `WorkspaceManager::start_session`), and return the new session's id.
    ///
    /// Runs under the caller's delivery lock and input route
    /// (`lock_delivery`, `open_route`), so no message is routed against the
    /// session being replaced and no archive tears it down halfway. Refused
    /// while a turn runs: the caller stops it first. An idle process is shut
    /// down, being attached to the session it launched with; the caller
    /// launches the new one (`start_process`) and tells the clients.
    ///
    /// The old session's transcript is ingested one last time, since nothing
    /// ingests into a superseded session (`finish_ingest`). Then everything
    /// in memory that belonged to it goes too:
    /// - its queued follow-ups (their rows go with the switch);
    /// - the live-turn buffer, which holds its last turn;
    /// - the stop flag, a deferred respawn and the native-silence marker, all
    ///   about its process's turn;
    /// - the live status, which now derives from the new row: no process, no
    ///   error.
    ///
    /// What stays is keyed by something that outlives a conversation: the
    /// workspace's checkouts, shells, run panel, delivery lock and stale-base
    /// note, and the sync-health state, which the next healthy turn clears like
    /// any other. The ingest cursors need nothing: the offset and record count
    /// are columns of the session row, and a sub-agent cursor is keyed by its
    /// file, which belongs to one provider session.
    pub fn start_session(
        &self,
        agent_id: &str,
        lineage: &SessionLineage,
        branch: Option<&BranchPoint>,
    ) -> Result<String> {
        let record = self.workspace.agent(agent_id)?;
        if record.archive.is_some() {
            return Err(Error::Other("agent is archived".into()));
        }
        if branch.is_some() && !capabilities(&record.provider).branch_at_message {
            return Err(Error::Other(format!(
                "{} can't branch a conversation at a message",
                record.provider
            )));
        }
        // Checked under the `agents` lock it is removed under: a keystroke in
        // the native view starts a turn without the delivery lock.
        let taken = {
            let mut agents = self.agents.lock();
            if self.is_busy(agent_id) {
                return Err(Error::Other(
                    "stop the agent before starting a new session".into(),
                ));
            }
            agents.remove(agent_id)
        };
        // Before the shutdown, so the exit it causes is recognized as ours.
        self.bump_generation(agent_id);
        if let Some(agent) = taken {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.native_inputs.lock().remove(agent_id);

        let session = self.finish_ingest(agent_id, || {
            // The rows and the in-memory queue go under one hold of the queue
            // lock, as in `detach_runtime`. Lock order queue → db.
            let mut queue = self.message_queue.lock();
            let session = self.workspace.start_session(agent_id, lineage, branch)?;
            queue.clear(agent_id);
            Ok::<_, Error>(session)
        })?;
        if let Some(turn) = self.live_turns.lock().get_mut(agent_id) {
            turn.begin_turn();
        }
        self.interrupted.lock().remove(agent_id);
        self.respawn_pending.lock().remove(agent_id);
        self.heuristic_idle.lock().remove(agent_id);
        self.statuses.lock().remove(agent_id);
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::*;
    use crate::message_queue::PendingMsg;
    use crate::supervisor::tests::{record_in_checkouts, test_supervisor};
    use crate::workspace::{AgentStatus, Anchor};

    const AGENT: &str = "denali";

    /// An idle claude agent whose checkout is `<td>/ws/repo`, and the file its
    /// current session's transcript goes to: the docker per-agent dir beside
    /// the checkout, which the locator checks first.
    fn fixture(td: &Path) -> (Supervisor, PathBuf) {
        let sup = test_supervisor();
        let checkout = td.join("ws").join("repo");
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        let mut record = record_in_checkouts(&sup, AGENT, &[checkout]);
        sup.workspace.add_agent(&mut record).unwrap();
        let transcript = transcript_of(&sup, td);
        (sup, transcript)
    }

    fn transcript_of(sup: &Supervisor, td: &Path) -> PathBuf {
        let own = sup.workspace.agent(AGENT).unwrap().session_id.unwrap();
        let dir = td
            .join("ws")
            .join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME)
            .join("slug");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{own}.jsonl"))
    }

    fn write_lines(path: &Path, lines: &[serde_json::Value]) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn message(uuid: &str, kind: &str, text: &str) -> serde_json::Value {
        json!({"type": kind, "uuid": uuid, "message": {"content": text}})
    }

    fn queued(turn_id: &str) -> PendingMsg {
        PendingMsg {
            turn_id: turn_id.into(),
            text: "later".into(),
            attachments: vec![],
        }
    }

    #[test]
    fn a_new_session_leaves_nothing_of_the_old_one_behind() {
        let td = tempfile::tempdir().unwrap();
        let (sup, transcript) = fixture(td.path());
        sup.workspace
            .insert_user_turn(AGENT, "t1", "alpha", &[])
            .unwrap();
        write_lines(
            &transcript,
            &[
                message("u1", "user", "alpha"),
                message("a1", "assistant", "hi"),
            ],
        );
        sup.sync_session(AGENT);
        let lineage = sup.workspace.resolve_anchor(AGENT, Anchor::End).unwrap();
        let old_session = sup.workspace.agent(AGENT).unwrap().session_id;
        // The old session goes on past the point the new one continues from,
        // and the last of it hasn't been ingested yet.
        write_lines(
            &transcript,
            &[
                message("u2", "user", "bravo"),
                message("a2", "assistant", "ok"),
            ],
        );
        // Runtime state the old session left.
        sup.statuses.lock().insert(AGENT.into(), AgentStatus::Idle);
        sup.message_queue.lock().enqueue(AGENT, queued("q1"));
        sup.workspace
            .enqueue_pending_message(AGENT, &queued("q1"))
            .unwrap();
        sup.live_turns
            .lock()
            .entry(AGENT.into())
            .or_default()
            .push(json!({"type": "assistant"}));
        for flags in [&sup.interrupted, &sup.respawn_pending, &sup.heuristic_idle] {
            flags.lock().insert(AGENT.into());
        }
        let gen = sup.generations.lock().get(AGENT).copied().unwrap_or(0);

        sup.start_session(AGENT, &lineage, None).unwrap();

        let ids = |records: &[crate::workspace::SessionRecord]| {
            records
                .iter()
                .map(|r| r.native_id.clone())
                .collect::<Vec<_>>()
        };
        // The old session was ingested to its end before it was superseded.
        let superseded = sup.workspace.read_superseded_records(AGENT, &[]).unwrap();
        assert_eq!(superseded.len(), 1);
        assert_eq!(ids(&superseded[0].records), ["u1", "a1", "u2", "a2"]);
        // The new one shows the history it continues, and nothing of its own:
        // a later pass reads its own transcript, not the old one.
        sup.sync_session(AGENT);
        assert!(sup
            .workspace
            .read_session_records(AGENT)
            .unwrap()
            .is_empty());
        assert_eq!(
            ids(&sup.workspace.read_history_records(AGENT).unwrap()),
            ["u1", "a1"]
        );
        assert_ne!(sup.workspace.agent(AGENT).unwrap().session_id, old_session);

        assert!(sup.message_queue.lock().is_empty(AGENT));
        assert!(sup
            .workspace
            .read_all_pending_messages()
            .unwrap()
            .is_empty());
        let live = sup.read_live_turn(AGENT);
        assert!(live.events.is_empty());
        assert_eq!(live.next_seq, 1, "the event count goes on");
        for flags in [&sup.interrupted, &sup.respawn_pending, &sup.heuristic_idle] {
            assert!(!flags.lock().contains(AGENT));
        }
        assert_eq!(sup.statuses.lock().get(AGENT), None);
        assert_eq!(sup.status_of(AGENT), Some(AgentStatus::Idle));
        assert!(sup.generations.lock()[AGENT] > gen);
    }

    #[test]
    fn a_running_turn_keeps_its_session() {
        let td = tempfile::tempdir().unwrap();
        let (sup, _) = fixture(td.path());
        let lineage = sup.workspace.resolve_anchor(AGENT, Anchor::End).unwrap();
        let old_session = sup.workspace.agent(AGENT).unwrap().session_id;
        sup.message_queue.lock().enqueue(AGENT, queued("q1"));
        for busy in [AgentStatus::Running, AgentStatus::Spawning] {
            sup.statuses.lock().insert(AGENT.into(), busy);

            let err = sup.start_session(AGENT, &lineage, None).unwrap_err();

            assert!(err.to_string().contains("stop the agent"), "{err}");
            assert_eq!(sup.workspace.agent(AGENT).unwrap().session_id, old_session);
            assert!(!sup.message_queue.lock().is_empty(AGENT));
        }
    }
}
