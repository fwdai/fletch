//! Moving a workspace between the Linux VM and the macOS sandbox, in place:
//! the same record, name and sidebar row; its code, committed and not, on its
//! branch; and its conversation, which the agent goes on with natively.
//!
//! Both sandboxes keep a workspace's checkouts at the same path
//! ([`agent_parent_dir`]): a box in its guest, every path under it routed
//! there while the box is registered; seatbelt on this Mac. The two sides
//! can't both be there, so the side on this Mac waits beside it, in
//! [`moving_dir`]:
//!
//!  - **Into a VM**: the checkouts on this Mac step aside there, the box is
//!    provisioned at the path, and each checkout's snapshot is pushed in.
//!  - **Onto this Mac**: the checkouts are built there from the snapshots the
//!    box hands out, and put at the path once the box is gone.
//!
//! The workspace's stamp is the commit point. Until it changes the old side
//! is the workspace and the new side scrap; after, the reverse. [`settle`]
//! reads the stamp and makes the disk agree: the move's last step, its
//! rollback, and at boot the recovery from a move the app didn't live to
//! finish ([`settle_interrupted_moves`]).
//!
//! While a move runs, and for as long as one is left unsettled (its staging
//! dir still there), the workspace's checkouts are not at rest, and nothing
//! may put work into them: every entry point that would asks
//! [`Supervisor::checkouts_at_rest`] first. A launch goes further and settles
//! what is left first, so a workspace recovers at the next message, not only
//! at the next boot.
//!
//! What doesn't travel: ignored files (`node_modules`, `.env`), which no
//! snapshot holds, and earlier turns' checkpoints, so the code can't be
//! rewound past the move (the conversation still can).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::git::{self, checkpoint};
use crate::host::EngineCtx;
use crate::sandbox::provision::CheckoutSpec;
use crate::sandbox::{self, stamped_engine, EngineKind};
use crate::workspace::{
    agent_parent_dir, folder_grants, moving_dir, moving_root, AgentRecord, AgentStatus, Anchor,
    SessionLineage, TrackedRepo, WorkspaceManager,
};

use super::events::emit_workspace_changed;
use super::lifecycle::{arm_spawn_timeout, fail_spawn, provision_codegraph_index};
use super::Supervisor;

const BUSY: &str = "Wait for the turn to finish before moving the workspace.";
const MOVING: &str = "The workspace is moving to another sandbox. Try again once it's done.";
const UNSETTLED: &str = "The workspace's last move between sandboxes didn't finish";

/// One `settle` at a time: a move, a launch and boot can each try to finish
/// what a move left.
static SETTLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A move under way, from its start until the disk is settled: the workspace
/// is out of rest (`Supervisor::moving`) for as long as this lives.
struct InFlight<'a> {
    sup: &'a Supervisor,
    agent_id: String,
}

impl<'a> InFlight<'a> {
    fn begin(sup: &'a Supervisor, agent_id: &str) -> Result<Self> {
        if !sup.moving.lock().insert(agent_id.to_string()) {
            return Err(Error::Other(MOVING.into()));
        }
        Ok(Self {
            sup,
            agent_id: agent_id.to_string(),
        })
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.sup.moving.lock().remove(&self.agent_id);
    }
}

/// The conversation a moved workspace goes on with, read while the old
/// sandbox still holds its transcript.
struct Conversation {
    lineage: SessionLineage,
    history: Vec<Value>,
    context: Option<String>,
}

/// How long a move waits for the one-shot actions already at work in the
/// checkouts (a commit, a pull, a verification) before it refuses.
const QUIESCE_WAIT: Duration = Duration::from_secs(10);

/// A one-shot action's hold on an agent's checkouts, for as long as it acts
/// in them ([`Supervisor::hold_checkouts`]): no move starts on them until it
/// is dropped.
pub struct CheckoutHold(#[allow(dead_code)] tokio::sync::OwnedRwLockReadGuard<()>);

impl Supervisor {
    /// Whether work may be put into `agent_id`'s checkouts now, or why not: a
    /// move between sandboxes is under way, or one didn't finish. Asked by
    /// every entry point that would. What starts something long-lived there
    /// asks on entry and again as it registers, under the lock the move's
    /// sweep takes ([`Self::refuse_if_moving`]): the agent's launch
    /// ([`Self::settle_before_launch`]), the user's shell, the Run panel's
    /// app. A one-shot action holds them instead ([`Self::hold_checkouts`]):
    /// the Git panel's (`commands::agent_repo_checkout`). A local review
    /// waits for them (`review::engine`).
    pub fn checkouts_at_rest(&self, agent_id: &str) -> Result<()> {
        if self.moving.lock().contains(agent_id) {
            return Err(Error::Other(MOVING.into()));
        }
        if moving_dir(agent_id)?.exists() {
            return Err(Error::Other(format!(
                "{UNSETTLED}. Send a message or restart Fletch to finish it."
            )));
        }
        Ok(())
    }

    /// Refuse while a move is under way: what something long-lived checks as
    /// it registers, under the same lock the move's sweep (`take_down`)
    /// takes, so it is either refused here or swept there.
    pub(super) fn refuse_if_moving(&self, agent_id: &str) -> Result<()> {
        if self.moving.lock().contains(agent_id) {
            return Err(Error::Other(MOVING.into()));
        }
        Ok(())
    }

    /// Hold `agent_id`'s checkouts for a one-shot action in them, or why not
    /// ([`Self::checkouts_at_rest`]). A move waits for the hold to go
    /// before it touches them.
    pub fn hold_checkouts(&self, agent_id: &str) -> Result<CheckoutHold> {
        let hold = self
            .checkout_lock(agent_id)
            .try_read_owned()
            .map_err(|_| Error::Other(MOVING.into()))?;
        // Checked once held: a move flags itself before it waits for holds.
        self.checkouts_at_rest(agent_id)?;
        Ok(CheckoutHold(hold))
    }

    fn checkout_lock(&self, agent_id: &str) -> Arc<tokio::sync::RwLock<()>> {
        self.checkout_locks
            .lock()
            .entry(agent_id.to_string())
            .or_default()
            .clone()
    }

    /// [`Self::checkouts_at_rest`] for the agent's launch, which first
    /// finishes a move that didn't, when it can.
    pub(super) async fn settle_before_launch(&self, agent_id: &str) -> Result<()> {
        if self.moving.lock().contains(agent_id) {
            return Err(Error::Other(MOVING.into()));
        }
        settle(&self.workspace, agent_id)
            .await
            .map_err(|e| Error::Other(format!("{UNSETTLED}, and finishing it failed: {e}")))
    }

    /// Move `agent_id`'s workspace to the sandbox `kind` names (`host`, the
    /// macOS sandbox, or `box`, a Linux VM), keeping its code, branch and
    /// conversation. Resolves to the moved record once its agent is launching
    /// there.
    ///
    /// Everything that can refuse is checked before anything changes. A
    /// failure before the restamp leaves the workspace where it was, its agent
    /// relaunched if it was up; after it, the workspace has moved, and what
    /// couldn't follow is the error. Either way, a disk that couldn't be
    /// settled keeps the workspace out of rest, its agent down, until it is.
    pub async fn move_workspace(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        kind: &str,
    ) -> Result<AgentRecord> {
        // Rewind's prologue: no message is routed into the workspace, and no
        // archive takes it, until the move is over.
        self.workspace.agent(agent_id)?;
        let _delivering = self.lock_delivery(agent_id).await;
        let _route = self.open_route(agent_id)?;
        // Out of rest from here: no shell, Run app, Git action or review
        // starts in the checkouts until the disk is settled. The actions
        // already at work in them finish first.
        let in_flight = InFlight::begin(self, agent_id)?;
        let _exclusive =
            tokio::time::timeout(QUIESCE_WAIT, self.checkout_lock(agent_id).write_owned())
                .await
                .map_err(|_| {
                    Error::Other(
                        "A Git action or a verification is still running in the workspace. \
                 Try again once it's done."
                            .into(),
                    )
                })?;
        // A move the app didn't finish is put right before another starts.
        settle(&self.workspace, agent_id).await?;
        let record = self.workspace.agent(agent_id)?;
        let to = self.check_movable(&record, kind).await?;

        let was_up = self.take_down(agent_id).inspect_err(|_| {
            self.discard_prefetch(agent_id);
        })?;
        let carried = match self.read_conversation(&record) {
            Ok(conversation) => self.carry_code(&record, to).await.map(|()| conversation),
            Err(e) => Err(e),
        };
        let conversation = match carried {
            Ok(conversation) => conversation,
            Err(e) => {
                drop(in_flight);
                return Err(self.give_up(ctx, agent_id, was_up, e).await);
            }
        };
        // Restamped: the new side is the workspace. Its conversation goes
        // first, where the new sandbox keeps it, so the switch holds whatever
        // happens to the disk next: whatever finishes the move launches it.
        let moved = self.workspace.agent(agent_id)?;
        emit_workspace_changed(ctx.sink.as_ref());
        tracing::info!(agent_id, to = to.as_setting(), "moved the workspace");
        let Conversation {
            lineage,
            history,
            context,
        } = conversation;
        let switched = self
            .begin_native_session(ctx, &moved, &lineage, &history)
            .await;
        // Then the old side goes. If it can't, the workspace stays out of
        // rest, its agent down and in error, until a launch or boot finishes
        // it.
        if let Err(e) = settle(&self.workspace, agent_id).await {
            let why = format!(
                "The workspace moved, but its old sandbox couldn't be cleared ({e}). \
                 Send a message or restart Fletch to finish."
            );
            fail_spawn(self, ctx, agent_id, why.clone());
            return Err(Error::Other(why));
        }
        drop(in_flight);
        warm_indexes(&moved).await;
        switched.map_err(|e| {
            Error::Other(format!(
                "The workspace moved, but its conversation couldn't go with it: {e}"
            ))
        })?;
        self.launch_native_session(ctx, &moved, context).await;
        self.agent_record(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))
    }

    /// The engine `record` would move to for `kind`, or why it can't move:
    /// a workspace that isn't its own (a workflow step, a tree it adopts), a
    /// provider whose conversation Fletch can't carry, a turn or a local
    /// review under way, a sandbox it can't run in now, folders or a login
    /// the target can't have. Resolves the target's login as its launch will.
    async fn check_movable(&self, record: &AgentRecord, kind: &str) -> Result<EngineKind> {
        let refusal = if record.archive.is_some() {
            Some("agent is archived")
        } else if record.owner_run_id.is_some() {
            Some("A workflow step runs where its run does.")
        } else if record.repos.is_empty() || record.repos.iter().any(TrackedRepo::is_adopted) {
            Some("This workspace works in another's checkout, so it can't move.")
        } else if self.is_busy(&record.id) {
            Some(BUSY)
        } else if self.reviewed(&record.id) {
            Some("A local review is running in this workspace. Wait for it to finish.")
        } else {
            None
        };
        if let Some(refusal) = refusal {
            return Err(Error::Other(refusal.into()));
        }
        sandbox::ensure_engine_not_retired(record)?;
        let (wanted, provider) = (kind.to_string(), record.provider.clone());
        let to =
            tokio::task::spawn_blocking(move || sandbox::engine_to_move_to(&wanted, &provider))
                .await
                .map_err(|e| Error::Other(format!("sandbox check failed: {e}")))??;
        if to == stamped_engine(record) {
            return Err(Error::Other("The workspace already runs there.".into()));
        }
        // After the target's own refusals, which name the provider when a VM
        // can't run it.
        if !crate::agent::capabilities(&record.provider).transcript_writer {
            return Err(Error::Other(
                "Fletch can't carry this agent's conversation to another sandbox.".into(),
            ));
        }
        folder_grants::check_all(record.folder_grants.clone(), to).await?;
        let mut target = record.clone();
        target.sandbox_engine = Some(to.as_setting().to_string());
        if let Err(e) = self
            .prefetch_login_as(&record.id, &target, record.account.as_deref(), None)
            .await
        {
            self.discard_prefetch(&record.id);
            return Err(Error::Other(format!(
                "Couldn't sign the agent in there: {e}"
            )));
        }
        Ok(to)
    }

    /// Whether a local reviewer is at work in `agent_id`'s checkouts, which a
    /// move would pull from under it.
    fn reviewed(&self, agent_id: &str) -> bool {
        crate::review::engine::global()
            .reviewers_of(agent_id)
            .into_iter()
            .any(|(_, reviewer)| {
                self.workspace
                    .agent(&reviewer)
                    .is_ok_and(|r| r.archive.is_none())
            })
    }

    /// Take down what runs in the workspace's checkouts: its agent's process,
    /// the user's shell and the Run panel's app. Refused mid-turn. Returns
    /// whether the agent had a process.
    fn take_down(&self, agent_id: &str) -> Result<bool> {
        // As a session switch takes its process: checked under the `agents`
        // lock, since a keystroke in the native view starts a turn without
        // the delivery lock.
        let taken = {
            let mut agents = self.agents.lock();
            if self.is_busy(agent_id) {
                return Err(Error::Other(BUSY.into()));
            }
            agents.remove(agent_id)
        };
        // Before the shutdown, so the exit it causes is recognized as ours.
        self.bump_generation(agent_id);
        let was_up = taken.is_some();
        if let Some(agent) = taken {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.native_inputs.lock().remove(agent_id);
        self.end_shell(agent_id);
        let run = self.runs.lock().remove(agent_id);
        if let Some(run) = run {
            run.stop();
        }
        Ok(was_up)
    }

    /// After a move failed before its restamp: drop the login resolved for
    /// the target, and relaunch the agent if the move took it down, where it
    /// was, but only once its checkouts are back at rest. A rollback that
    /// couldn't settle the disk leaves it down. Hands back the error.
    async fn give_up(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        was_up: bool,
        error: Error,
    ) -> Error {
        self.discard_prefetch(agent_id);
        if was_up && self.checkouts_at_rest(agent_id).is_ok() {
            let _ = self.prefetch_login(agent_id).await;
            self.set_status(ctx, agent_id, AgentStatus::Spawning, None);
            arm_spawn_timeout(self.clone(), ctx.clone(), agent_id.to_string());
            if let Err(e) = self.start_process(ctx, agent_id).await {
                fail_spawn(self, ctx, agent_id, e.to_string());
            }
        }
        error
    }

    /// The conversation `record`'s moved workspace goes on with: its session
    /// to the end, every reply ingested first, since the transcript stays with
    /// the old sandbox.
    fn read_conversation(&self, record: &AgentRecord) -> Result<Conversation> {
        self.finish_ingest(&record.id, || ());
        let lineage = self.workspace.resolve_anchor(&record.id, Anchor::End)?;
        let history = self
            .native_history(&record.provider, &lineage)?
            .map_err(Error::Other)?;
        let context = self
            .workspace
            .session_handoff_context(&lineage.parent_session_id)?;
        Ok(Conversation {
            lineage,
            history,
            context,
        })
    }

    /// Put every checkout of `record` on `to`'s side, each from a snapshot of
    /// it as it stands, then restamp the workspace with `to`: the commit
    /// point. On an error the stamp is as it was, and the old side is put
    /// back as it stood ([`settle`]).
    async fn carry_code(&self, record: &AgentRecord, to: EngineKind) -> Result<()> {
        let code = self.pin_code(&record.id).await?;
        let carried = async {
            let here = agent_parent_dir(&record.id)?;
            let staged = moving_dir(&record.id)?;
            let onto_mac = to.caps().checkout_on_host;
            if let Some(parent) = staged.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            if !onto_mac {
                // This Mac's checkouts step aside: the box takes the path.
                tokio::fs::rename(&here, &staged).await?;
            }
            let (from, into) = if onto_mac {
                (&here, &staged)
            } else {
                (&staged, &here)
            };
            for repo in &record.repos {
                carry_checkout(
                    record,
                    repo,
                    to,
                    &from.join(&repo.subdir),
                    &into.join(&repo.subdir),
                    code.key(),
                )
                .await?;
            }
            // The files the workspace adopted (`attachments`): outside every
            // checkout, and the conversation names them by path.
            let (from, into) = (from.clone(), into.clone());
            tokio::task::spawn_blocking(move || crate::attachments::carry(&from, &into))
                .await
                .map_err(|e| Error::Other(format!("carrying attachments failed: {e}")))??;
            self.workspace
                .update_agent_engine(&record.id, to.as_setting())
        }
        .await;
        let Err(e) = carried else {
            return Ok(());
        };
        // Still stamped as it was, so settling puts the old side back at the
        // path, where the pin is then dropped from its checkouts. One that
        // can't be put back is the error: the workspace isn't where it was.
        match settle(&self.workspace, &record.id).await {
            Ok(()) => {
                code.release().await;
                Err(e)
            }
            Err(undo) => Err(Error::Other(format!(
                "The move failed ({e}), and putting the workspace back failed too ({undo}). \
                 Its agent stays down until that's done: send a message or restart Fletch \
                 to retry."
            ))),
        }
    }
}

/// Build `dest`, `repo`'s checkout on `to`'s side, from the snapshot pinned
/// under `key` in `source`: provisioned at its base as a spawn's is, on the
/// branch `source` is on when it is on one, then put to the snapshot (HEAD on the
/// commit it was taken on, the uncommitted work uncommitted), with the
/// branch's `origin` tracking ref beside it so its ahead/behind and its PR
/// read as before. The snapshot crosses the box boundary as a fork's code
/// does (`checkpoint::fetch_into`).
async fn carry_checkout(
    record: &AgentRecord,
    repo: &TrackedRepo,
    to: EngineKind,
    source: &Path,
    dest: &Path,
    key: &str,
) -> Result<()> {
    let base = repo.base_branch().await;
    let fallback = git::base_tip(&repo.repo_path, &base).await;
    let spec = CheckoutSpec {
        source_repo: &repo.repo_path,
        base_ref: fallback.as_deref().unwrap_or(&base),
        dest,
    };
    // The branch the checkout is on: the record learns one only when it's
    // pushed, and a branch the agent made itself is just as much the work's.
    let branch = git::current_branch(source).await?;
    sandbox::provision_checkout(to, record, &spec, &base).await?;
    if let Some(branch) = &branch {
        git::run_git(dest, &["checkout", "-q", "-B", branch], "create the branch").await?;
    }
    let sha = checkpoint::fetch_into(dest, source, key).await?;
    let restored = checkpoint::restore(dest, &sha).await;
    if let Err(e) = checkpoint::unpin(dest, key).await {
        tracing::warn!(error = %e, subdir = %repo.subdir, "dropping a moved code pin failed");
    }
    restored?;
    if let Some(branch) = &branch {
        let tracking = format!("refs/remotes/origin/{branch}");
        if git::resolve(source, &tracking).await?.is_some() {
            sandbox::carry_checkpoint(source, dest, &tracking).await?;
        }
    }
    Ok(())
}

/// Warm the codegraph index of every moved checkout, as a restore does.
/// Best-effort; nothing when indexing is off.
async fn warm_indexes(record: &AgentRecord) {
    for repo in &record.repos {
        let Ok(checkout) = repo.checkout_path(&record.id) else {
            continue;
        };
        let head = git::rev_parse(&checkout, "HEAD").await.ok();
        provision_codegraph_index(
            record.project_id.clone(),
            repo.repo_path.clone(),
            checkout,
            head,
            &record.provider,
            &record.mcp_servers,
        )
        .await;
    }
}

/// Make the disk agree with `agent_id`'s stamp after a move, finished or not.
/// Its checkouts belong on this Mac: any box it has is discarded and the
/// staged checkouts are put at the workspace's path. They belong in a VM, or
/// the workspace is archived or gone: the staged checkouts go. Nothing to do
/// when nothing is staged. One at a time ([`SETTLING`]).
pub(super) async fn settle(workspace: &WorkspaceManager, agent_id: &str) -> Result<()> {
    let staged = moving_dir(agent_id)?;
    if !staged.exists() {
        return Ok(());
    }
    let _one = SETTLING.lock().await;
    if !staged.exists() {
        return Ok(());
    }
    let on_mac = match workspace.agent(agent_id) {
        Ok(record) if record.archive.is_some() => false,
        Ok(record) => stamped_engine(&record).caps().checkout_on_host,
        Err(Error::AgentNotFound(_)) => false,
        Err(e) => return Err(e),
    };
    settle_at(agent_id, on_mac, &agent_parent_dir(agent_id)?, &staged).await
}

/// [`settle`] with its paths: `here` the workspace's, `staged` its staging dir.
async fn settle_at(agent_id: &str, on_mac: bool, here: &Path, staged: &Path) -> Result<()> {
    if !on_mac {
        tokio::fs::remove_dir_all(staged).await?;
        return Ok(());
    }
    // Discarded first: until its route is gone, the path still means the
    // guest. What a box leaves at the path is empty placeholders.
    sandbox::discard_agent_state(agent_id).await?;
    // Entry by entry, each staged one replacing its namesake: what was
    // written at the path meanwhile (the moved session's transcript, for a
    // provider that keeps it beside the checkouts) stays, and a settle cut
    // short picks up where it left off.
    tokio::fs::create_dir_all(here).await?;
    for entry in std::fs::read_dir(staged)? {
        let entry = entry?;
        let target = here.join(entry.file_name());
        match tokio::fs::symlink_metadata(&target).await {
            Ok(meta) if meta.is_dir() => tokio::fs::remove_dir_all(&target).await?,
            Ok(_) => tokio::fs::remove_file(&target).await?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        tokio::fs::rename(entry.path(), &target).await?;
    }
    tokio::fs::remove_dir(staged).await?;
    Ok(())
}

/// At boot: settle every move the app didn't live to finish. Boot waits for
/// it, so nothing launches into a workspace before its move is settled.
pub async fn settle_interrupted_moves(workspace: Arc<WorkspaceManager>) {
    let Ok(root) = moving_root() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(agent_id) = entry.file_name().into_string() else {
            continue;
        };
        match settle(&workspace, &agent_id).await {
            Ok(()) => tracing::info!(agent_id, "settled an interrupted sandbox move"),
            Err(e) => {
                tracing::warn!(agent_id, error = %e, "settling an interrupted sandbox move failed")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::supervisor::tests::{committed_repo, record_in_checkouts, test_supervisor};

    async fn git_out(repo: &Path, args: &[&str]) -> String {
        let out = git::run_git(repo, args, "test").await.unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A checkout of `source` at `dir/name` with an identity, as a spawn's.
    async fn checkout_of(source: &Path, dir: &Path, name: &str) -> PathBuf {
        let checkout = dir.join(name);
        let spec = CheckoutSpec {
            source_repo: source,
            base_ref: "HEAD",
            dest: &checkout,
        };
        sandbox::provision::provision(&spec).await.unwrap();
        for (key, value) in [("user.email", "t@example.com"), ("user.name", "Tester")] {
            git_out(&checkout, &["config", key, value]).await;
        }
        checkout
    }

    /// A checkout carried to this Mac's side comes out as the source stood:
    /// on its branch, HEAD on its last commit, the edit and the new file
    /// still uncommitted, its `origin` tracking ref in place, and no pin left.
    #[tokio::test]
    async fn a_checkout_is_carried_on_its_branch_with_its_work() {
        let td = tempfile::tempdir().unwrap();
        let repo = committed_repo(td.path(), "src").await;
        let base = git_out(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
        let source = checkout_of(&repo, td.path(), "source").await;
        git_out(&source, &["checkout", "-q", "-b", "feat/move"]).await;
        std::fs::write(source.join("b.txt"), b"committed").unwrap();
        git::commit_all(&source, "work").await.unwrap();
        let pushed = git_out(&source, &["rev-parse", "HEAD~1"]).await;
        git_out(
            &source,
            &["update-ref", "refs/remotes/origin/feat/move", &pushed],
        )
        .await;
        std::fs::write(source.join("a.txt"), b"edited").unwrap();
        std::fs::write(source.join("new.txt"), b"untracked").unwrap();

        let sup = test_supervisor();
        let mut record = record_in_checkouts(&sup, "denali", std::slice::from_ref(&source));
        // No branch of record: it was never pushed. The checkout's own is
        // what travels.
        let repo_entry = TrackedRepo {
            repo_path: repo.clone(),
            branch: None,
            parent_branch: Some(base),
            ..record.repos[0].clone()
        };
        record.repos = vec![repo_entry.clone()];
        let key = "move-test";
        checkpoint::capture(&source, key).await.unwrap();

        let dest = td.path().join("staged").join("repo-0");
        let mac = stamped_engine(&record);
        carry_checkout(&record, &repo_entry, mac, &source, &dest, key)
            .await
            .unwrap();

        assert_eq!(
            git_out(&dest, &["rev-parse", "--abbrev-ref", "HEAD"]).await,
            "feat/move"
        );
        assert_eq!(
            git_out(&dest, &["rev-parse", "HEAD"]).await,
            git_out(&source, &["rev-parse", "HEAD"]).await
        );
        assert_eq!(
            git_out(&dest, &["status", "--porcelain"]).await,
            git_out(&source, &["status", "--porcelain"]).await
        );
        assert_eq!(std::fs::read(dest.join("new.txt")).unwrap(), b"untracked");
        assert_eq!(
            git_out(&dest, &["rev-parse", "refs/remotes/origin/feat/move"]).await,
            pushed
        );
        assert_eq!(
            git_out(&dest, &["for-each-ref", "refs/fletch/checkpoints"]).await,
            ""
        );
    }

    /// While a move is under way nothing gets into the checkouts, through any
    /// door: the rest check, the check at registration, a one-shot hold, a
    /// second move. A hold already taken keeps the move off them until it
    /// goes. Once the move is over, they are at rest again.
    #[tokio::test]
    async fn a_move_under_way_keeps_all_work_out_of_the_checkouts() {
        let sup = test_supervisor();
        let id = "move-gate-test";
        sup.checkouts_at_rest(id).unwrap();
        let hold = sup.hold_checkouts(id).unwrap();

        let moving = InFlight::begin(&sup, id).unwrap();
        assert!(sup.checkouts_at_rest(id).is_err());
        assert!(sup.refuse_if_moving(id).is_err());
        assert!(sup.hold_checkouts(id).is_err());
        assert!(InFlight::begin(&sup, id).is_err(), "one move at a time");
        let lock = sup.checkout_lock(id);
        assert!(
            lock.clone().try_write_owned().is_err(),
            "the action already at work finishes first"
        );
        drop(hold);
        let exclusive = lock.try_write_owned().unwrap();
        drop(exclusive);
        drop(moving);

        sup.checkouts_at_rest(id).unwrap();
        sup.refuse_if_moving(id).unwrap();
        drop(sup.hold_checkouts(id).unwrap());
    }

    /// Stamped for a VM, the staged checkouts on this Mac are scrap; stamped
    /// for this Mac, they replace what is at the path (a box's placeholders).
    #[tokio::test]
    async fn settling_follows_the_stamp() {
        let td = tempfile::tempdir().unwrap();
        let here = td.path().join("ws").join("denali");
        let staged = td.path().join("ws.moving").join("denali");
        for dir in [&here, &staged] {
            std::fs::create_dir_all(dir.join("repo")).unwrap();
        }
        std::fs::write(staged.join("repo").join("code.txt"), b"staged").unwrap();
        // An id no box was ever made for: discarding its box is a no-op.
        let id = "settle-test-no-such-box";

        settle_at(id, false, &here, &staged).await.unwrap();
        assert!(!staged.exists());
        assert!(here.join("repo").is_dir());

        std::fs::create_dir_all(staged.join("repo")).unwrap();
        std::fs::write(staged.join("repo").join("code.txt"), b"staged").unwrap();
        // Written at the path while the move was settling: it stays.
        std::fs::create_dir_all(here.join(".sessions")).unwrap();
        std::fs::write(here.join(".sessions").join("s.jsonl"), b"{}").unwrap();
        settle_at(id, true, &here, &staged).await.unwrap();
        assert!(!staged.exists());
        assert_eq!(
            std::fs::read(here.join("repo").join("code.txt")).unwrap(),
            b"staged"
        );
        assert!(here.join(".sessions").join("s.jsonl").exists());
    }
}
