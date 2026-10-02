//! Agent disposition: archive, restore, and discard, plus the repo
//! snapshot/teardown helpers they share.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::git;
use crate::host::EngineCtx;
use crate::sandbox::provision::{self, CheckoutSpec};
use crate::sandbox::{docker, podman, EngineKind};
use crate::workspace::{
    agent_parent_dir, repo_checkout_path, AgentRecord, AgentStatus, ArchivedRepoSnapshot,
    DiffStats, TrackedRepo,
};

use super::events::emit_workspace_changed;
use super::lifecycle::{arm_spawn_timeout, fail_spawn, provision_codegraph_index, stamped_engine};
use super::Supervisor;

impl Supervisor {
    /// Move an agent into the History view: stop the process if any,
    /// snapshot each tracked repo's SHA + diff stats, then tear down
    /// the checkouts and branches. The claude session JSONL is left
    /// alone — that's what makes restore possible.
    ///
    /// Rejects while the agent is actively spawning or running a turn.
    /// Idle agents are safe to archive; we shut down the waiting
    /// process before taking repo snapshots.
    ///
    /// The archive is committed — `archived_at` stamped, `workspace:changed`
    /// emitted — BEFORE any cleanup, and everything after that mark is
    /// best-effort. The click is the user's decision; the seconds of git and
    /// `rm -rf` that follow must not be a window in which a workspace read
    /// still reports the agent as live (that is how a just-archived row used
    /// to reappear in the sidebar). Cleanup failures are recorded on the row
    /// (`finish_archive`) rather than returned, so the caller never sees an
    /// error for an archive that did happen.
    pub async fn archive_agent(self: Arc<Self>, ctx: Arc<EngineCtx>, agent_id: &str) -> Result<()> {
        self.archive_agent_as(ctx, agent_id, ArchiveTrigger::User)
            .await
    }

    /// [`Self::archive_agent`] with the trigger spelled out — see
    /// [`ArchiveTrigger`] for the one thing it changes.
    pub async fn archive_agent_as(
        self: Arc<Self>,
        ctx: Arc<EngineCtx>,
        agent_id: &str,
        trigger: ArchiveTrigger,
    ) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        if record.archive.is_some() {
            return Err(Error::Other("agent is already archived".into()));
        }
        // Held to the end: input that arrives from here on is refused rather
        // than delivered into — or queued behind — a runtime about to go.
        let _disposal = self.reserve_disposal(agent_id, &record, trigger)?;

        self.workspace.begin_archive(agent_id)?;
        emit_workspace_changed(ctx.sink.as_ref());

        self.detach_runtime(agent_id);
        reap_agent_containers(agent_id, Some(&record), "archive");

        // Snapshot SHAs + diff stats before any destructive step, then tear
        // down the checkouts/branches (best-effort — a single git failure
        // shouldn't block archive, since the user's intent is "get rid of
        // this").
        let snapshots = capture_repo_snapshots(agent_id, &record.repos).await;

        // A clone workspace's commits exist only inside the clone until they
        // are pushed — teardown deletes unpushed ones for good (restore can
        // refetch a pushed branch from origin, nothing else). Warn loudly so
        // that data-loss case is diagnosable; archive itself stays
        // best-effort by design.
        for snap in &snapshots {
            let Some(tip) = snap.branch_tip_sha.as_deref() else {
                continue;
            };
            if git::rev_parse(&snap.repo_path, tip).await.is_err() {
                tracing::warn!(
                    agent_id,
                    subdir = %snap.subdir,
                    tip,
                    "archiving clone workspace whose tip isn't in the source repo; \
                     restore will need the branch to have been pushed"
                );
            }
        }

        let failures = teardown_agent_checkouts(agent_id, &record.repos, "archive").await;

        let error = (!failures.is_empty()).then(|| failures.join("; "));
        if let Err(e) = self
            .workspace
            .finish_archive(agent_id, &snapshots, error.as_deref())
        {
            // The agent is archived regardless; what's lost is the snapshot
            // restore rebuilds from and the timing trace. Loud, not fatal.
            tracing::error!(agent_id, error = %e, "recording archive completion failed");
        }
        // The snapshot (diff stats, branch tips) reshapes the record beyond
        // what `agent:status` carries, so ping the frontend to reload the
        // workspace now that History has something to show.
        emit_workspace_changed(ctx.sink.as_ref());
        Ok(())
    }

    /// Reserve `agent_id` for archive — see [`Disposal`] for the contract.
    ///
    /// Refused while a route into the agent is open, while it is mid-turn, and
    /// (for [`ArchiveTrigger::Sweep`]) while a follow-up sits in its queue:
    /// each of those is a user message that would otherwise vanish with the
    /// runtime. Every check and the mark happen under the one lock. Released
    /// when the guard drops.
    pub(super) fn reserve_disposal(
        &self,
        agent_id: &str,
        record: &AgentRecord,
        trigger: ArchiveTrigger,
    ) -> Result<DisposalGuard<'_>> {
        let mut disposal = self.disposal.lock();
        if disposal.reserved.contains(agent_id) {
            return Err(Error::Other("archive is already in progress".into()));
        }
        if disposal.routing.get(agent_id).is_some_and(|n| *n > 0) {
            return Err(Error::Other(
                "agent is receiving a message; try again".into(),
            ));
        }
        if matches!(
            self.effective_status(agent_id, record),
            AgentStatus::Spawning | AgentStatus::Running
        ) {
            return Err(Error::Other(
                "agent must be idle, stopped, or in error before archiving".into(),
            ));
        }
        if trigger == ArchiveTrigger::Sweep && !self.message_queue.lock().is_empty(agent_id) {
            return Err(Error::Other("agent has queued messages".into()));
        }
        disposal.reserved.insert(agent_id.to_string());
        Ok(DisposalGuard {
            sup: self,
            agent_id: agent_id.to_string(),
        })
    }

    /// Open a route for input into `agent_id`: a user message about to be
    /// routed, a queued follow-up about to be flushed, a native keystroke about
    /// to be written. Refused while an archive holds the agent, so no input is
    /// delivered into — or queued behind — a runtime about to be torn down.
    /// While the guard lives, [`Self::reserve_disposal`] refuses in turn.
    pub(super) fn open_route(&self, agent_id: &str) -> Result<RouteGuard<'_>> {
        let mut disposal = self.disposal.lock();
        if disposal.reserved.contains(agent_id) {
            return Err(Error::Other("agent is being archived".into()));
        }
        *disposal.routing.entry(agent_id.to_string()).or_insert(0) += 1;
        Ok(RouteGuard {
            sup: self,
            agent_id: agent_id.to_string(),
        })
    }

    /// Pull an archived agent back into the live sidebar: recreate
    /// branches and checkouts from snapshot SHAs, clear archive
    /// metadata, transition to Spawning so the supervisor's start path
    /// attaches to the existing claude session.
    pub async fn restore_agent(self: Arc<Self>, ctx: Arc<EngineCtx>, agent_id: &str) -> Result<()> {
        let _lifecycle_guard = self.agent_lifecycle.lock().await;
        let record = self.workspace.agent(agent_id)?;
        let archive = record
            .archive
            .clone()
            .ok_or_else(|| Error::Other("agent is not archived".into()))?;
        if record.session_id.is_none() {
            return Err(Error::Other(
                "archived agent has no session id; cannot restore".into(),
            ));
        }

        // Pre-flight: every snapshot must have a tip SHA, and that SHA must
        // be recoverable. We do this before any mutation so we don't leave a
        // half-restored agent on failure. A clone's commits live only in the
        // (torn-down) clone and on the real remote once pushed, so a tip that
        // isn't in the source repo is still fine when a branch name exists:
        // provisioning recovers it from origin — by branch, or (branch
        // auto-deleted after merge) by the commit SHA, or failing that by
        // opening detached at the parent base. Any deep failure there tears the
        // half-built clone down and aborts before we mutate state.
        for snap in &archive.repos {
            let sha = snap.branch_tip_sha.as_deref().ok_or_else(|| {
                Error::Other(format!(
                    "snapshot for repo `{}` has no branch tip SHA",
                    snap.subdir
                ))
            })?;
            if let Err(e) = git::rev_parse(&snap.repo_path, sha).await {
                // A tip that's gone from the source store is still recoverable
                // by refetching the pushed branch — but only when the source
                // actually has an `origin` to fetch from. Without one (a repo
                // with no remote), provisioning would fail deep in
                // `fetch_branch` with a rawer error, so reject it here instead.
                let refetchable =
                    snap.branch_name.is_some() && source_has_origin(&snap.repo_path).await;
                if !refetchable {
                    return Err(Error::Other(format!(
                        "branch tip {} no longer reachable in {}: {e}",
                        sha,
                        snap.repo_path.display()
                    )));
                }
            }
        }

        // Ensure the agent parent dir exists.
        let parent_dir = agent_parent_dir(agent_id)?;
        tokio::fs::create_dir_all(&parent_dir)
            .await
            .map_err(|e| Error::Other(format!("create parent dir: {e}")))?;

        let mut restored: Vec<TrackedRepo> = Vec::with_capacity(archive.repos.len());
        for snap in &archive.repos {
            let tip_sha = snap.branch_tip_sha.as_deref().expect("checked above");

            let checkout = repo_checkout_path(agent_id, &snap.subdir)?;
            if let Some(parent) = checkout.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| Error::Other(format!("create checkout parent: {e}")))?;
            }

            let spec = CheckoutSpec {
                source_repo: &snap.repo_path,
                base_ref: tip_sha,
                dest: &checkout,
            };
            let branch = match &snap.branch_name {
                // The agent had pushed a branch → recreate it at the tip,
                // resolving name collisions with a -restored suffix. If the
                // branch was auto-deleted after merge (or the tip is otherwise
                // gone), provisioning degrades to a detached checkout and
                // returns `false`, and we record no branch.
                Some(desired_name) => {
                    let chosen = choose_restore_branch_name(&snap.repo_path, desired_name).await;
                    // `desired_name` rides along as the fetch source: when the
                    // tip must be refetched, the remote only knows the original
                    // name, not the -restored rename. `parent_branch_sha` is
                    // the last-resort detached base.
                    let landed_on_branch = provision::provision_on_branch(
                        &spec,
                        &chosen,
                        desired_name,
                        snap.parent_branch_sha.as_deref(),
                    )
                    .await?;
                    landed_on_branch.then_some(chosen)
                }
                // Branchless agent (never pushed) → restore detached at the
                // tip, ready to name its branch at the next push.
                None => {
                    provision::provision(&spec).await?;
                    None
                }
            };

            // Warm the codegraph index for the restored checkout too (best-effort;
            // no-op when indexing is off or under a container engine).
            provision_codegraph_index(
                record.project_id.clone(),
                snap.repo_path.clone(),
                checkout.clone(),
                Some(tip_sha.to_string()),
                stamped_engine(&record),
                &record.provider,
                &record.mcp_servers,
            )
            .await;

            restored.push(TrackedRepo {
                repo_path: snap.repo_path.clone(),
                subdir: snap.subdir.clone(),
                branch,
                parent_branch: snap.parent_branch.clone(),
                // The fork point persists in the worktrees row across
                // archive/restore (restore_agent doesn't clear base_sha), so
                // this literal value is never written back — None is a
                // placeholder to satisfy the struct.
                base_sha: None,
                // Likewise preserved in the worktrees row across restore;
                // placeholders to satisfy the struct.
                pr_number: None,
                pr_url: None,
                pr_title: None,
                pr_state: None,
                label: None,
                // Restore rebuilt an agent-owned clone at the derived path, so
                // the row's adoption (if it had one) is cleared by
                // `restore_agent` rather than carried forward.
                adopted_checkout: None,
            });
        }

        self.workspace.restore_agent(agent_id, restored)?;
        self.set_status(&ctx, agent_id, AgentStatus::Spawning, None);
        emit_workspace_changed(ctx.sink.as_ref());

        // Restore is an explicit user action, so bring the process up now
        // (set_status(Spawning) above lets start_process promote to Idle).
        arm_spawn_timeout(self.clone(), ctx.clone(), agent_id.to_string());
        let sup = self.clone();
        let ctx_for_task = ctx.clone();
        let id_for_task = agent_id.to_string();
        crate::host::spawn(async move {
            if let Err(e) = sup.start_process(&ctx_for_task, &id_for_task, false).await {
                fail_spawn(&sup, &ctx_for_task, &id_for_task, e.to_string());
            }
        });

        Ok(())
    }

    pub async fn discard_agent(self: Arc<Self>, agent_id: &str) -> Result<()> {
        let record = self.workspace.agent(agent_id).ok();
        let repos = record.as_ref().map(|r| r.repos.clone()).unwrap_or_default();

        self.detach_runtime(agent_id);
        reap_agent_containers(agent_id, record.as_ref(), "discard");
        // Discard deletes the row, so there is nowhere to record failures;
        // the helper has already logged them.
        let _ = teardown_agent_checkouts(agent_id, &repos, "discard").await;

        self.workspace.remove_agent(agent_id)?;
        Ok(())
    }

    /// Detach every idle project agent without deleting its durable row. The
    /// project FK cascade is the single DB commit point; separating runtime
    /// detachment from row deletion prevents per-agent partial commits.
    pub(super) fn detach_project_agents(&self, agents: &[AgentRecord]) {
        for agent in agents {
            self.detach_runtime(&agent.id);
        }
    }

    /// Best-effort physical cleanup after the project row has committed. At
    /// this point no user-visible project can be left half-deleted; failures
    /// are logged by the shared checkout teardown helper as orphan cleanup.
    pub(super) async fn teardown_project_checkouts(&self, agents: &[AgentRecord]) {
        for agent in agents {
            let _ = teardown_agent_checkouts(&agent.id, &agent.repos, "project delete").await;
        }
    }

    /// Detach an agent's live runtime: shut down its process and drop its
    /// in-memory state (activity detector, status, native input buffer, shell,
    /// and run-panel session). Shared by archive and discard.
    fn detach_runtime(&self, agent_id: &str) {
        // Bump first: invalidates the watchdog/RPC-watcher loops and the
        // process-exit handler before `shutdown()` triggers the latter, so the
        // exit can't re-emit `Idle` for the agent we're tearing down.
        self.bump_generation(agent_id);
        let taken = self.agents.lock().remove(agent_id);
        if let Some(agent) = taken {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.statuses.lock().remove(agent_id);
        self.native_inputs.lock().remove(agent_id);
        self.rpc_dispatchers.lock().remove(agent_id);
        self.live_turns.lock().remove(agent_id);
        self.delivery_locks.lock().remove(agent_id);
        // Clear the in-memory queue and its durable mirror under one hold of
        // the queue lock. Dropping the mirror stops an archived agent's queue
        // from rehydrating on the next launch (discard also cascades via the FK
        // when the workspace row is removed; this covers archive, which keeps
        // it). Holding the lock across both clears means a concurrent
        // `persist_and_enqueue` — which writes its row and queue entry under the
        // same lock — is fully ordered before or after this teardown, so it
        // can't slip a new row in between the two clears only to have it deleted
        // with no in-memory entry left to deliver it. Lock order stays queue →
        // db (see `messaging::Supervisor::persist_and_enqueue`).
        {
            let mut queue = self.message_queue.lock();
            queue.clear(agent_id);
            if let Err(e) = self.workspace.clear_pending_messages(agent_id) {
                tracing::warn!(error = %e, agent_id, "clear persisted pending follow-ups failed");
            }
        }
        self.interrupted.lock().remove(agent_id);
        // The stale-base warning belongs to one provisioning of one workspace:
        // a restore re-provisions from scratch and re-decides for itself.
        self.stale_base.lock().remove(agent_id);
        self.shells.lock().remove(agent_id);
        if let Some(run) = self.runs.lock().remove(agent_id) {
            run.stop();
        }
    }
}

/// Snapshot each tracked repo's tip SHA + diff stats against its fork point.
/// (The aggregate totals History shows are summed from these rows when the
/// record is read back — see `build_archive_metadata`.)
///
/// Resolves SHAs without mutating anything, so callers can capture state before
/// any destructive teardown. The tip is the checkout's HEAD — works whether the
/// agent is on a branch or still detached (never pushed), so both restore from
/// the exact committed tip.
async fn capture_repo_snapshots(
    agent_id: &str,
    repos: &[TrackedRepo],
) -> Vec<ArchivedRepoSnapshot> {
    let mut snapshots: Vec<ArchivedRepoSnapshot> = Vec::with_capacity(repos.len());

    for repo in repos {
        let checkout = repo.checkout_path(agent_id).ok();
        let branch_tip_sha = match &checkout {
            Some(wt) => git::rev_parse(wt, "HEAD").await.ok(),
            None => None,
        };
        // Where this checkout actually diverged from its base — the merge-base
        // with the base's current tip, since the base can move (or be
        // redirected) after spawn, with the recorded `base_sha` as the fallback
        // (`git::resolve_base`). Without a checkout there is nothing to take a
        // merge-base against, so the recorded SHA is all there is.
        let parent_branch_sha = match &checkout {
            Some(wt) => repo.resolve_base(wt).await.fork_point,
            None => repo.base_sha.clone(),
        };

        let mut adds = 0u32;
        let mut dels = 0u32;
        // The diff runs inside the workspace, not the source repo: a clone's
        // commits exist only in the clone's object store, while a worktree
        // shares its store with the source — so the workspace resolves both.
        if let (Some(wt), Some(from), Some(to)) = (&checkout, &parent_branch_sha, &branch_tip_sha) {
            if from != to {
                if let Ok((a, d)) = git::diff_shortstat(wt, from, to).await {
                    adds = a;
                    dels = d;
                }
            }
        }
        snapshots.push(ArchivedRepoSnapshot {
            repo_path: repo.repo_path.clone(),
            subdir: repo.subdir.clone(),
            branch_name: repo.branch.clone(),
            branch_tip_sha,
            parent_branch: repo.parent_branch.clone(),
            parent_branch_sha,
            diff_stats: DiffStats {
                additions: adds,
                deletions: dels,
            },
        });
    }

    snapshots
}

/// Whether `repo` has an `origin` remote — the only source a clone-mode restore
/// can refetch a tip from once it's gone from the local object store.
async fn source_has_origin(repo: &Path) -> bool {
    git::git_output(repo, &["remote", "get-url", "origin"])
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Pick a free branch name for a restored agent: the archived name if it's
/// still free, otherwise `-restored` / `-restored-N` suffixed until one is.
async fn choose_restore_branch_name(repo_path: &Path, desired: &str) -> String {
    let mut chosen = desired.to_string();
    let mut bumps = 0;
    loop {
        let exists = git::branch_exists(repo_path, &chosen)
            .await
            .unwrap_or(false);
        if !exists {
            return chosen;
        }
        bumps += 1;
        chosen = if bumps == 1 {
            format!("{desired}-restored")
        } else {
            format!("{desired}-restored-{bumps}")
        };
    }
}

/// Best-effort teardown of any container still bearing this agent's
/// `fletch.agent-id` label. The container counterpart to
/// [`teardown_agent_checkouts`], and like it, failures never abort disposal —
/// the caller's intent is to get rid of the agent.
///
/// Why it exists: `Supervisor::detach_runtime` reaches the container only
/// *indirectly*, through the in-memory `agents` entry — no entry, no
/// `KillHandle`, no runtime command at all. `run --rm` covers the ordinary exit
/// and the startup pid-liveness sweep covers a crashed instance, but neither
/// makes disposal itself self-sufficient. The label sweep does: it needs nothing
/// but the agent id. (Container names can't serve here — they carry a
/// launch-time nonce, see `container::util::container_name` — which is why the
/// id label exists.)
///
/// Three gates, cheapest first:
/// 1. The stamped engine, which also picks the runtime: an agent is reaped by
///    the runtime that launched it, never by whichever is selected now. A
///    seatbelt agent never had a container, so it never pays for a probe.
///    `record: None` (discard tolerates a missing row) can't prove which
///    runtime launched it, so it asks both — the label match is exact, so
///    looking costs nothing but the queries.
/// 2. The runtime's own availability probe — a machine without that runtime
///    installed must never see one of its invocations, the same precedent the
///    startup sweeps set.
/// 3. Off the caller's path entirely. `spawn_blocking` rather than a bare
///    thread: the CLI call is a blocking process wait (up to the probe's 2s
///    plus the sweep's own timeouts) issued from inside the tokio runtime,
///    which is precisely the blocking pool's job — and unlike the startup
///    sweep, which runs before any runtime is a given, we always have one
///    here. Detached on purpose: archive must not wait on a container
///    round-trip to report success to the user.
fn reap_agent_containers(agent_id: &str, record: Option<&AgentRecord>, op: &'static str) {
    let engine = record.map(stamped_engine);
    if engine.is_some_and(|k| !k.is_container()) {
        return;
    }
    let agent_id = agent_id.to_string();
    tokio::task::spawn_blocking(move || {
        let report = |runtime: &'static str, removed: crate::error::Result<usize>| match removed {
            Ok(0) => {}
            Ok(n) => {
                tracing::info!(agent_id = %agent_id, op, runtime, removed = n, "removed agent containers")
            }
            Err(e) => {
                tracing::warn!(agent_id = %agent_id, op, runtime, error = %e, "agent container removal failed")
            }
        };
        // A stamped engine narrows to one runtime; a rowless agent asks both.
        if engine != Some(EngineKind::Podman)
            && matches!(
                docker::availability(),
                docker::DockerAvailability::Available { .. }
            )
        {
            report("docker", docker::remove_agent_containers(&agent_id));
        }
        if engine.map_or(true, |k| k == EngineKind::Podman)
            && matches!(
                podman::availability(),
                podman::PodmanAvailability::Available { .. }
            )
        {
            report("podman", podman::remove_agent_containers(&agent_id));
        }
    });
}

/// Best-effort teardown of every tracked repo's checkout + branch, plus the
/// agent's parent dir. Failures are logged (tagged with `op` for context) but
/// never abort the sweep — the caller's intent is to get rid of the agent.
/// Shared by archive and discard. Returns one message per failed step, so
/// archive can keep them on the row as its audit trail; empty means clean.
///
/// An *adopted* checkout is skipped: the agent was a tenant of a working tree
/// something else owns (the workflow kernel's shared run workspace, which the
/// run's own delete reclaims — see `workflow::scheduler`), so disposing of the
/// agent must leave the directory exactly where it is. Every step of a run
/// passes through here as it is archived, so the run's work would not survive
/// its first completed step otherwise.
async fn teardown_agent_checkouts(agent_id: &str, repos: &[TrackedRepo], op: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for repo in repos {
        if repo.is_adopted() {
            continue;
        }
        let checkout = match repo.checkout_path(agent_id) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, subdir = %repo.subdir, op, "checkout path resolution failed");
                failures.push(format!("{}: checkout path: {e}", repo.subdir));
                continue;
            }
        };
        // A clone is self-contained — `rm -rf` the checkout dir. The agent's
        // branch (if any) lived inside that clone and was never created in the
        // user's source repo, so there is nothing to `branch -D` here: doing so
        // would force-delete an unrelated same-named branch in the source repo.
        if let Err(e) = provision::teardown(&checkout).await {
            tracing::warn!(error = %e, subdir = %repo.subdir, op, "workspace teardown failed");
            failures.push(format!("{}: teardown: {e}", repo.subdir));
        }
        // Legacy safety net: an agent provisioned under the removed worktree
        // mode (pre-upgrade) left a `.git/worktrees/<name>` registration in the
        // source repo. The `rm -rf` above orphans that entry, which keeps its
        // branch marked checked-out and blocks later `checkout` / `branch -D` in
        // the source repo until pruned. A best-effort prune clears any now-
        // missing registration; it is a no-op for clone workspaces (nothing is
        // registered) and never touches the branch itself.
        let _ = git::worktree_prune(&repo.repo_path).await;
    }

    // Remove the parent dir (may still hold orphan files if any checkout
    // removal failed). Best-effort, retried + logged (see `remove_agent_dir`).
    if !remove_agent_dir(agent_id, op).await {
        failures.push("agent dir removal failed".to_string());
    }

    // The agent's RPC mailbox dies with it — nothing will ever read from it
    // again, and leaving it behind leaks one dir per agent ever spawned.
    if let Err(e) = crate::rpc::remove_mailbox(agent_id) {
        tracing::warn!(agent_id, op, error = %e, "rpc mailbox removal failed");
        failures.push(format!("rpc mailbox: {e}"));
    }
    failures
}

/// Remove an agent's parent checkout dir, retrying briefly. Returns `true` once
/// the dir is gone (removed now or already absent).
///
/// The common failure right after process shutdown is a still-open file handle
/// — a just-exited child, the codegraph indexer — that clears within a moment,
/// so a few spaced retries recover most cases. A dir that survives everything
/// is logged at `error`. It no longer reserves the agent's name — allocation is
/// DB-authoritative (per-build root) and provision clears any leftover at the
/// clone target — so a lingering dir is only wasted disk, not a correctness
/// problem; the log is the hygiene hook.
async fn remove_agent_dir(agent_id: &str, op: &str) -> bool {
    let parent = match agent_parent_dir(agent_id) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(agent_id, op, error = %e, "agent dir path resolution failed");
            return false;
        }
    };
    for attempt in 1..=3u32 {
        if !parent.exists() {
            return true;
        }
        match tokio::fs::remove_dir_all(&parent).await {
            Ok(()) => return true,
            Err(e) if attempt < 3 => {
                tracing::warn!(
                    agent_id, op, attempt, path = %parent.display(), error = %e,
                    "checkout dir removal failed; retrying"
                );
                tokio::time::sleep(std::time::Duration::from_millis(150 * attempt as u64)).await;
            }
            Err(e) => {
                tracing::error!(
                    agent_id, op, path = %parent.display(), error = %e,
                    "checkout dir removal failed after retries; the dir is wasted disk \
                     until the next spawn reuses this name and provision clears it"
                );
                return false;
            }
        }
    }
    !parent.exists()
}

/// Who asked for an archive. Decides one thing: what becomes of follow-ups
/// still queued for the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveTrigger {
    /// The user asked (a click, a remote command, a finished workflow step).
    /// Queued follow-ups are theirs to abandon and go with the runtime, as they
    /// always have.
    User,
    /// An unattended rule asked (`auto_archive`). Nothing the user wrote may be
    /// lost on their behalf, so a queued follow-up refuses the archive.
    Sweep,
}

/// Per-agent exclusion between archive and input (`Supervisor::disposal`).
///
/// Two things must never overlap on one agent: an archive tearing its runtime
/// and checkout down, and a message or keystroke on its way in. Both go
/// through here. A route opens with [`Supervisor::open_route`] and is refused
/// while the agent is reserved; an archive reserves with
/// [`Supervisor::reserve_disposal`] and is refused while any route is open.
/// Each check and its mark happen under the one lock, so neither side can slip
/// between the other's check and its act — the shape `deleting_projects`
/// already gives project deletion, per agent.
#[derive(Default)]
pub(crate) struct Disposal {
    /// Agents an archive holds, from reservation to the end of teardown.
    reserved: HashSet<String>,
    /// Agents with input mid-route — a send being routed, a queue flush being
    /// delivered, a native keystroke being written — by open count.
    routing: HashMap<String, usize>,
}

/// An agent's archive reservation (`Supervisor::reserve_disposal`); dropping it
/// lets input through again. Dropped on every exit from `archive_agent_as`, so a
/// failed archive — or a later restore under the same id — is never wedged.
pub(super) struct DisposalGuard<'a> {
    sup: &'a Supervisor,
    agent_id: String,
}

impl Drop for DisposalGuard<'_> {
    fn drop(&mut self) {
        self.sup.disposal.lock().reserved.remove(&self.agent_id);
    }
}

/// An open input route (`Supervisor::open_route`); dropping it closes the route.
pub(super) struct RouteGuard<'a> {
    sup: &'a Supervisor,
    agent_id: String,
}

impl Drop for RouteGuard<'_> {
    fn drop(&mut self) {
        let mut disposal = self.sup.disposal.lock();
        if let Some(open) = disposal.routing.get_mut(&self.agent_id) {
            *open -= 1;
            if *open > 0 {
                return;
            }
        }
        disposal.routing.remove(&self.agent_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_queue::PendingMsg;
    use crate::supervisor::tests::{record_with_status, test_supervisor};
    use std::path::Path;

    #[test]
    fn a_disposal_reservation_is_exclusive_and_released_on_drop() {
        let sup = test_supervisor();
        let record = record_with_status("denali", AgentStatus::Idle);
        let user = ArchiveTrigger::User;

        let held = sup.reserve_disposal("denali", &record, user).unwrap();
        assert!(
            sup.reserve_disposal("denali", &record, user).is_err(),
            "a second archive of the same agent must be refused"
        );
        drop(held);
        assert!(sup.reserve_disposal("denali", &record, user).is_ok());

        // A turn that won the race reads as Running here and wins outright.
        sup.statuses
            .lock()
            .insert("denali".to_string(), AgentStatus::Running);
        assert!(sup.reserve_disposal("denali", &record, user).is_err());
        assert!(
            !sup.disposal.lock().reserved.contains("denali"),
            "a refused reservation must not leave the agent reserved"
        );
    }

    #[test]
    fn an_open_route_and_a_reservation_exclude_each_other() {
        let sup = test_supervisor();
        let record = record_with_status("denali", AgentStatus::Idle);
        let sweep = ArchiveTrigger::Sweep;

        // Input mid-route keeps archive out — for as long as any route is open.
        let first = sup.open_route("denali").unwrap();
        let second = sup.open_route("denali").unwrap();
        assert!(sup.reserve_disposal("denali", &record, sweep).is_err());
        drop(first);
        assert!(
            sup.reserve_disposal("denali", &record, sweep).is_err(),
            "one of two routes closing is not enough"
        );
        drop(second);

        // And a reservation keeps input out, for this agent only.
        let held = sup.reserve_disposal("denali", &record, sweep).unwrap();
        assert!(sup.open_route("denali").is_err());
        assert!(sup.open_route("rainier").is_ok());
        drop(held);
        assert!(sup.open_route("denali").is_ok());
    }

    #[test]
    fn a_queued_follow_up_refuses_the_sweep_but_not_the_user() {
        let sup = test_supervisor();
        let record = record_with_status("denali", AgentStatus::Idle);
        sup.message_queue.lock().enqueue(
            "denali",
            PendingMsg {
                turn_id: "t1".into(),
                text: "and then this".into(),
                attachments: vec![],
            },
        );
        assert!(sup
            .reserve_disposal("denali", &record, ArchiveTrigger::Sweep)
            .is_err());
        assert!(sup
            .reserve_disposal("denali", &record, ArchiveTrigger::User)
            .is_ok());
    }

    fn git(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Every step of a workflow run is archived as it finishes, and each one
    /// runs in the *same* adopted tree — so a teardown that removed it would
    /// destroy the run's work at the first step boundary. The agent's own dirs
    /// still go; the directory the run owns stays.
    #[tokio::test]
    async fn teardown_spares_an_adopted_checkout() {
        let td = tempfile::tempdir().unwrap();
        let adopted = td.path().join("run-repo");
        std::fs::create_dir_all(adopted.join(".git")).unwrap();
        std::fs::write(adopted.join("step.txt"), "work so far").unwrap();

        let repo = TrackedRepo {
            repo_path: td.path().join("source"),
            subdir: "repo".into(),
            branch: None,
            parent_branch: None,
            base_sha: None,
            pr_number: None,
            pr_url: None,
            pr_title: None,
            pr_state: None,
            label: None,
            adopted_checkout: Some(adopted.clone()),
        };
        // An id no agent ever had: the agent-owned paths this resolves resolve
        // to nothing, which is the point — only the adopted tree exists.
        teardown_agent_checkouts("adoption-teardown-test", &[repo], "test").await;

        assert!(
            adopted.join("step.txt").is_file(),
            "the run's working tree must outlive the step agent"
        );
    }

    #[tokio::test]
    async fn source_has_origin_reflects_the_remote() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        git(repo, &["init", "-q", "-b", "main"]);
        // A fresh repo has no `origin` — a clone-mode tip that's gone from the
        // object store here is genuinely unrecoverable, so the restore
        // pre-flight must reject it rather than fail later inside `fetch_branch`.
        assert!(!source_has_origin(repo).await);
        // Adding a remote flips it.
        git(
            repo,
            &["remote", "add", "origin", "https://example.com/x.git"],
        );
        assert!(source_has_origin(repo).await);
    }
}
