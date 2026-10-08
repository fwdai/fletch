//! Continuing a conversation natively (docs/fork-and-rewind.md,
//! "Materialize"). Fletch stores every provider's transcript lines verbatim,
//! so a new session needs no CLI fork feature to pick a conversation up: the
//! history it continues is written as its own provider session file, where the
//! CLI looks for it, and the CLI resumes it like any session of its own.
//!
//! The file opens with that history, which the session already shows through
//! its lineage, so it is recorded on the session how many records that is
//! (`sessions.transcript_prefix`) and ingestion drops them.

use serde_json::Value;

use crate::agent::{capabilities, provider_bin_label, transcript_reader, ReadDiagnostics};
use crate::error::{Error, Result};
use crate::workspace::{AgentRecord, NativeTranscript, SessionLineage, SessionRecord};

use super::lifecycle::stamped_engine;
use super::session_sync::{agent_sync_lock, SUBAGENT_TAG};
use super::Supervisor;

/// The bodies of `records` a native continuation copies: the main transcript's
/// lines, as the CLI wrote them. Not a sub-agent's, ingested from a file of its
/// own and tagged, nor a record compiled from the live stream. `Err` names why
/// a `provider` session can't continue them: part of them is another
/// provider's.
fn transcript_bodies(
    provider: &str,
    records: Vec<SessionRecord>,
) -> std::result::Result<Vec<Value>, String> {
    let mut bodies = Vec::new();
    for record in records {
        if record.source != "transcript" || record.body.get(SUBAGENT_TAG).is_some() {
            continue;
        }
        if record.provider != provider {
            return Err(format!(
                "Part of this conversation was held by {}, so {} can't continue it as its own.",
                label(&record.provider),
                label(provider),
            ));
        }
        bodies.push(record.body);
    }
    Ok(bodies)
}

fn label(provider: &str) -> &str {
    provider_bin_label(provider).map_or(provider, |(_, label)| label)
}

impl Supervisor {
    /// The transcript a `provider` session continuing `lineage` natively
    /// starts with, or why there can't be one: the provider has no transcript
    /// writer, or part of the history is another provider's. The outer
    /// `Result` is a failed read.
    pub(super) fn native_history(
        &self,
        provider: &str,
        lineage: &SessionLineage,
    ) -> Result<std::result::Result<Vec<Value>, String>> {
        if !capabilities(provider).transcript_writer {
            return Ok(Err(format!(
                "{} keeps its conversations where Fletch can't write them.",
                label(provider)
            )));
        }
        let records = self.workspace.read_lineage_records(lineage)?;
        Ok(transcript_bodies(provider, records))
    }

    /// Write `bodies` as session `session_id` of `record`'s provider, where its
    /// CLI looks for it when run in `record`'s primary checkout, and count the
    /// records the file opens with by reading it back with the provider's own
    /// reader, so the prefix ingestion drops is exactly what is there. `None`
    /// when there was nothing to write, or nothing the CLI could resume.
    pub(super) fn write_native_transcript(
        &self,
        record: &AgentRecord,
        session_id: &str,
        bodies: &[Value],
    ) -> Result<Option<NativeTranscript>> {
        if bodies.is_empty() {
            return Ok(None);
        }
        let provider = record.provider.as_str();
        let reader = transcript_reader(provider);
        let (Some(reader), Some(write)) = (reader, reader.and_then(|r| r.write)) else {
            return Err(Error::Other(format!(
                "Fletch can't write {}'s sessions",
                label(provider)
            )));
        };
        let cwd = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no tracked repos".into()))?
            .checkout_path(&record.id)?;
        let container = stamped_engine(record).is_container();
        // Where the agent's CLI will look: under the account it was stamped
        // with when its launch runs the CLI there (`CODEX_HOME`), else the
        // default dir every claude account shares. A removed account is an
        // error here too: the launch that follows would fail on it anyway, and
        // a codex writer would otherwise recreate its directory.
        let account_dir =
            crate::agent::accounts::existing_account_dir(provider, record.account.as_deref())?
                .filter(|_| crate::agent::accounts::launches_in_account_dir(provider));
        let Some(path) = write(session_id, &cwd, container, account_dir.as_deref(), bodies)? else {
            return Ok(None);
        };
        let mut diag = ReadDiagnostics::default();
        let located = (reader.locate)(session_id, &cwd, &mut diag);
        if !located.contains(&path) {
            return Err(Error::Other(format!(
                "the conversation was written to {}, where {} won't find it",
                path.display(),
                label(provider)
            )));
        }
        Ok(Some(NativeTranscript {
            provider_session_id: session_id.to_string(),
            prefix: (reader.read)(&located, &mut diag).len(),
        }))
    }

    /// Write the history `agent_id`'s current session continues as its own
    /// transcript, before its first launch: a fork's full conversation. The
    /// session is the provider session it was written as from then on —
    /// claude's own id, or one minted here for a per-turn provider, which
    /// otherwise gets its id from its first turn — so the launch resumes it.
    ///
    /// Under the agent's sync lock, so no ingestion pass reads the file before
    /// the session records how much of it is the copied history.
    pub(super) fn materialize(&self, agent_id: &str) -> Result<()> {
        let serialize = agent_sync_lock(agent_id);
        let _pass = serialize.lock();
        let record = self.workspace.agent(agent_id)?;
        let lineage = record
            .lineage
            .as_ref()
            .ok_or_else(|| Error::Other("the session continues no conversation".into()))?;
        let bodies = self
            .native_history(&record.provider, lineage)?
            .map_err(Error::Other)?;
        let session_id = record
            .session_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if let Some(native) = self.write_native_transcript(&record, &session_id, &bodies)? {
            self.workspace.set_native_transcript(agent_id, &native)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::supervisor::tests::{record_in_checkouts, test_supervisor};
    use crate::supervisor::{ForkCode, ForkContext};
    use crate::workspace::Anchor;

    /// Workspace `id` of `provider`, working in `<td>/<id>/repo`, continuing
    /// `lineage`. In a container sandbox, so its claude transcripts are in the
    /// per-agent projects dir beside the checkout.
    fn agent(
        sup: &Supervisor,
        td: &Path,
        id: &str,
        provider: &str,
        lineage: Option<SessionLineage>,
    ) {
        let checkout = td.join(id).join("repo");
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        let mut record = record_in_checkouts(sup, id, &[checkout]);
        record.provider = provider.into();
        record.sandbox_engine = Some("docker".into());
        record.lineage = lineage;
        sup.workspace.add_agent(&mut record).unwrap();
    }

    fn meta() -> Value {
        json!({"type": "mode", "mode": "normal", "sessionId": "alps-session"})
    }

    fn prompt(turn: &str, parent: Option<&str>) -> Value {
        json!({"type": "user", "uuid": format!("{turn}-u"), "parentUuid": parent,
               "sessionId": "alps-session", "cwd": "/old", "message": {"content": turn}})
    }

    fn reply(turn: &str) -> Value {
        json!({"type": "assistant", "uuid": format!("{turn}-a"), "parentUuid": format!("{turn}-u"),
               "sessionId": "alps-session", "cwd": "/old"})
    }

    /// Ingest `lines` into `ws` as its own transcript records (claude's ids:
    /// the `uuid`, else positional from `first`), and match turn `turn` to its
    /// prompt.
    fn ingested(sup: &Supervisor, ws: &str, turn: &str, first: usize, lines: &[Value]) {
        sup.workspace.insert_user_turn(ws, turn, turn, &[]).unwrap();
        let records: Vec<(String, &Value)> = lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let id = line["uuid"]
                    .as_str()
                    .map_or(format!("ln:{}", first + i), str::to_string);
                (id, line)
            })
            .collect();
        let batch: Vec<(&str, &Value)> = records.iter().map(|(id, l)| (id.as_str(), *l)).collect();
        sup.workspace
            .append_session_records(ws, "claude", "transcript", None, &batch)
            .unwrap();
        sup.workspace.associate_pending_user_turns(ws).unwrap();
    }

    /// `ws`'s current transcript file, where claude looks for it.
    fn transcript(sup: &Supervisor, ws: &str) -> PathBuf {
        let record = sup.workspace.agent(ws).unwrap();
        let cwd = record.repos[0].checkout_path(ws).unwrap();
        let mut diag = ReadDiagnostics::default();
        let found =
            crate::transcripts::find_session_jsonl(&record.session_id.unwrap(), &cwd, &mut diag);
        found.expect("the session's transcript is where claude looks")
    }

    fn lines_of(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// What the CLI does when it resumes the file: append the new turn.
    fn resume_and_say(path: &Path, lines: &[Value]) {
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn ids(records: Vec<crate::workspace::SessionRecord>) -> Vec<(String, bool)> {
        records
            .into_iter()
            .map(|r| (r.native_id, r.inherited))
            .collect()
    }

    fn own(ids: &[&str]) -> Vec<(String, bool)> {
        ids.iter().map(|id| (id.to_string(), false)).collect()
    }

    fn inherited(ids: &[&str]) -> Vec<(String, bool)> {
        ids.iter().map(|id| (id.to_string(), true)).collect()
    }

    /// `alps` (claude) said a1 then a2; `andes` forked it through a1, in full.
    fn forked(td: &Path) -> Supervisor {
        let sup = test_supervisor();
        agent(&sup, td, "alps", "claude", None);
        ingested(
            &sup,
            "alps",
            "a1",
            0,
            &[meta(), prompt("a1", None), reply("a1")],
        );
        ingested(
            &sup,
            "alps",
            "a2",
            3,
            &[prompt("a2", Some("a1-a")), reply("a2")],
        );
        let lineage = sup
            .workspace
            .resolve_anchor("alps", Anchor::Through("a1"))
            .unwrap();
        agent(&sup, td, "andes", "claude", Some(lineage));
        sup.materialize("andes").unwrap();
        sup
    }

    /// A full fork: the child's own transcript opens with the parent's
    /// conversation through the anchor, where claude resumes it, and the
    /// child stores only what is said after it.
    #[test]
    fn a_full_fork_resumes_the_parents_conversation_and_stores_only_its_own() {
        let td = tempfile::tempdir().unwrap();
        let sup = forked(td.path());
        let andes = sup.workspace.agent("andes").unwrap();
        let file = transcript(&sup, "andes");
        let cwd = andes.repos[0].checkout_path("andes").unwrap();

        // The parent's lines through a1, now the child's session.
        let written = lines_of(&file);
        assert_eq!(
            written
                .iter()
                .map(|l| l["uuid"].clone())
                .collect::<Vec<_>>(),
            [Value::Null, json!("a1-u"), json!("a1-a")]
        );
        let session = andes.session_id.clone().unwrap();
        assert!(written.iter().all(|l| l["sessionId"] == session.as_str()));
        assert_eq!(written[1]["cwd"], cwd.to_string_lossy().as_ref());
        assert_eq!(sup.workspace.session_transcript_prefix("andes").unwrap(), 3);
        assert!(crate::agent::claude_session_has_messages(&session, &cwd));
        assert_eq!(
            andes.lineage,
            Some(
                sup.workspace
                    .resolve_anchor("alps", Anchor::Through("a1"))
                    .unwrap()
            )
        );
        // Nothing is stored twice: the history is the parent's.
        sup.sync_session("andes");
        assert!(sup
            .workspace
            .read_session_records("andes")
            .unwrap()
            .is_empty());

        // Claude resumes it: what it appends is the child's own.
        let b1 = [meta(), prompt("b1", Some("a1-a")), reply("b1")];
        resume_and_say(&file, &b1);
        sup.workspace
            .insert_user_turn("andes", "b1", "b1", &[])
            .unwrap();
        sup.sync_session("andes");
        resume_and_say(&file, &[meta(), prompt("b2", Some("b1-a")), reply("b2")]);
        sup.sync_session("andes");

        // Positional ids are the lines' places in the file, as a full read
        // numbers them.
        let claude = transcript_reader("claude").unwrap();
        let all = (claude.read)(std::slice::from_ref(&file), &mut ReadDiagnostics::default());
        let expected: Vec<String> = all[3..].iter().map(|r| r.native_id.clone()).collect();
        assert_eq!(expected, ["ln:3", "b1-u", "b1-a", "ln:6", "b2-u", "b2-a"]);
        assert_eq!(
            ids(sup.workspace.read_session_records("andes").unwrap()),
            own(&["ln:3", "b1-u", "b1-a", "ln:6", "b2-u", "b2-a"]),
            "the child's own records — all its usage counts — exclude the copy"
        );
        assert_eq!(
            ids(sup.workspace.read_history_records("andes").unwrap()),
            [
                inherited(&["ln:0", "a1-u", "a1-a"]),
                own(&["ln:3", "b1-u", "b1-a", "ln:6", "b2-u", "b2-a"]),
            ]
            .concat()
        );
        let turns = sup.workspace.read_history_turns("andes").unwrap();
        assert_eq!(turns.last().unwrap().native_id.as_deref(), Some("b1-u"));
    }

    /// A fork of a fork carries what the fork inherited too, ahead of what it
    /// said itself, as one conversation.
    #[test]
    fn a_fork_of_a_fork_carries_the_inherited_part() {
        let td = tempfile::tempdir().unwrap();
        let sup = forked(td.path());
        let file = transcript(&sup, "andes");
        resume_and_say(&file, &[prompt("b1", Some("a1-a")), reply("b1")]);
        sup.workspace
            .insert_user_turn("andes", "b1", "b1", &[])
            .unwrap();
        sup.sync_session("andes");

        let lineage = sup
            .workspace
            .resolve_anchor("andes", Anchor::Through("b1"))
            .unwrap();
        agent(&sup, td.path(), "atlas", "claude", Some(lineage));
        sup.materialize("atlas").unwrap();

        let written = lines_of(&transcript(&sup, "atlas"));
        assert_eq!(
            written
                .iter()
                .map(|l| l["uuid"].clone())
                .collect::<Vec<_>>(),
            [
                Value::Null,
                json!("a1-u"),
                json!("a1-a"),
                json!("b1-u"),
                json!("b1-a")
            ]
        );
        let session = sup.workspace.agent("atlas").unwrap().session_id.unwrap();
        assert!(written.iter().all(|l| l["sessionId"] == session.as_str()));
        assert_eq!(sup.workspace.session_transcript_prefix("atlas").unwrap(), 5);
        sup.sync_session("atlas");
        assert!(sup
            .workspace
            .read_session_records("atlas")
            .unwrap()
            .is_empty());
    }

    /// A per-turn provider (codex) is read in full every pass, and gets the
    /// thread id the transcript was written as, which every turn resumes.
    #[test]
    fn a_full_reader_drops_the_copy_on_every_pass() {
        let _env = super::super::session_sync::tests::ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CODEX_HOME", home.path());
        let td = tempfile::tempdir().unwrap();
        let sup = test_supervisor();
        agent(&sup, td.path(), "rainier", "codex", None);
        let meta = json!({"type": "session_meta", "payload": {"id": "old-thread",
                          "session_id": "old-thread", "cwd": "/old"}});
        let said = |text: &str| {
            json!({"type": "response_item", "payload": {"type": "message",
                                       "role": "assistant", "content": text}})
        };
        let lines = [meta, said("one"), said("two")];
        let batch: Vec<(String, &Value)> = lines
            .iter()
            .enumerate()
            .map(|(i, l)| (format!("ln:{i}"), l))
            .collect();
        let batch: Vec<(&str, &Value)> = batch.iter().map(|(id, l)| (id.as_str(), *l)).collect();
        sup.workspace
            .append_session_records("rainier", "codex", "transcript", None, &batch)
            .unwrap();
        let lineage = sup
            .workspace
            .resolve_anchor("rainier", Anchor::End)
            .unwrap();
        agent(&sup, td.path(), "shasta", "codex", Some(lineage));

        sup.materialize("shasta").unwrap();
        let thread = sup.workspace.agent("shasta").unwrap().session_id.unwrap();
        let mut diag = ReadDiagnostics::default();
        let rollouts = crate::transcripts::find_codex_rollouts(&thread, &mut diag);
        resume_and_say(&rollouts[0], &[said("three")]);
        sup.sync_session("shasta");
        sup.sync_session("shasta");
        let written = lines_of(&rollouts[0]);
        std::env::remove_var("CODEX_HOME");

        assert_eq!(rollouts.len(), 1);
        assert_eq!(written[0]["payload"]["id"], thread.as_str());
        assert_eq!(
            sup.workspace.session_transcript_prefix("shasta").unwrap(),
            3
        );
        assert_eq!(
            ids(sup.workspace.read_session_records("shasta").unwrap()),
            own(&["ln:3"])
        );
    }

    /// A full fork is refused before anything is created when the child's
    /// provider has no transcript writer, or the history is partly another
    /// provider's.
    #[tokio::test]
    async fn a_full_fork_that_cant_be_made_is_refused_up_front() {
        let td = tempfile::tempdir().unwrap();
        let sup = Arc::new(test_supervisor());
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        agent(&sup, td.path(), "kenya", "cursor", None);
        agent(&sup, td.path(), "alps", "claude", None);
        ingested(&sup, "alps", "a1", 0, &[prompt("a1", None), reply("a1")]);
        let foreign = json!({"type": "response_item"});
        sup.workspace
            .append_session_records("alps", "codex", "transcript", None, &[("ln:9", &foreign)])
            .unwrap();
        let before = sup.workspace.current().unwrap().agents.len();

        for (parent, why) in [
            (
                "kenya",
                "Cursor keeps its conversations where Fletch can't write them.",
            ),
            ("alps", "Part of this conversation was held by Codex"),
        ] {
            let err = sup
                .clone()
                .fork_agent(
                    ctx.clone(),
                    parent,
                    None,
                    ForkCode::Clean,
                    ForkContext::Full,
                    None,
                )
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains(why), "{err}");
            assert!(err.contains("Fork with a summary instead"), "{err}");
        }
        assert_eq!(sup.workspace.current().unwrap().agents.len(), before);
    }
}
