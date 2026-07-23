//! Agent disposition: archive, restore, and discard, plus the repo
//! snapshot/teardown helpers they share.

use std::path::Path;
use std::sync::Arc;
use tauri::AppHandle;

use crate::error::{Error, Result};
use crate::git;
use crate::sandbox::provision::{self, CheckoutSpec};
use crate::workspace::{
    agent_parent_dir, repo_checkout_path, AgentRecord, AgentStatus, ArchiveMetadata,
    ArchivedRepoSnapshot, DiffStats, TrackedRepo,
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
    pub async fn archive_agent(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        if record.archive.is_some() {
            return Err(Error::Other("agent is already archived".into()));
        }
        if matches!(
            self.effective_status(agent_id, &record),
            AgentStatus::Spawning | AgentStatus::Running
        ) {
            return Err(Error::Other(
                "agent must be idle, stopped, or in error before archiving".into(),
            ));
        }

        self.detach_runtime(agent_id);

        // Snapshot SHAs + diff stats before any destructive step, then tear
        // down the checkouts/branches (best-effort — a single git failure
        // shouldn't block archive, since the user's intent is "get rid of
        // this").
        let (snapshots, diff_stats) = capture_repo_snapshots(agent_id, &record.repos).await;

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

        teardown_agent_checkouts(agent_id, &record.repos, "archive").await;

        let archive = ArchiveMetadata {
            archived_at: chrono::Utc::now().to_rfc3339(),
            repos: snapshots,
            diff_stats,
        };

        self.workspace.archive_agent(agent_id, archive)?;
        // The frontend listens to `agent:status` to drive most UI;
        // archive is structurally a deeper change, so we re-emit the
        // workspace via a tiny event. Frontend already reloads on this
        // signal via `get_workspace`.
        emit_workspace_changed(&app);
        Ok(())
    }

    /// Pull an archived agent back into the live sidebar: recreate
    /// branches and checkouts from snapshot SHAs, clear archive
    /// metadata, transition to Spawning so the supervisor's start path
    /// attaches to the existing claude session.
    pub async fn restore_agent(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
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
            // no-op when indexing is off or under Docker).
            provision_codegraph_index(
                record.project_id.clone(),
                snap.repo_path.clone(),
                checkout.clone(),
                Some(tip_sha.to_string()),
                stamped_engine(&record),
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
            });
        }

        self.workspace.restore_agent(agent_id, restored)?;
        self.set_status(&app, agent_id, AgentStatus::Spawning, None);
        emit_workspace_changed(&app);

        // Restore is an explicit user action, so bring the process up now
        // (set_status(Spawning) above lets start_process promote to Idle).
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.to_string());
        let sup = self.clone();
        let app_for_task = app.clone();
        let id_for_task = agent_id.to_string();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = sup.start_process(&app_for_task, &id_for_task, false).await {
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
            }
        });

        Ok(())
    }

    pub async fn discard_agent(self: Arc<Self>, agent_id: &str) -> Result<()> {
        let record = self.workspace.agent(agent_id).ok();
        let repos = record.as_ref().map(|r| r.repos.clone()).unwrap_or_default();

        self.detach_runtime(agent_id);
        teardown_agent_checkouts(agent_id, &repos, "discard").await;

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
            teardown_agent_checkouts(&agent.id, &agent.repos, "project delete").await;
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
        self.shells.lock().remove(agent_id);
        if let Some(run) = self.runs.lock().remove(agent_id) {
            run.stop();
        }
    }
}

/// Snapshot each tracked repo's tip SHA + diff stats against its fork point,
/// returning the per-repo snapshots plus the aggregate add/delete totals.
///
/// Resolves SHAs without mutating anything, so callers can capture state before
/// any destructive teardown. The tip is the checkout's HEAD — works whether the
/// agent is on a branch or still detached (never pushed), so both restore from
/// the exact committed tip.
async fn capture_repo_snapshots(
    agent_id: &str,
    repos: &[TrackedRepo],
) -> (Vec<ArchivedRepoSnapshot>, DiffStats) {
    let mut snapshots: Vec<ArchivedRepoSnapshot> = Vec::with_capacity(repos.len());
    let mut total_adds: u32 = 0;
    let mut total_dels: u32 = 0;

    for repo in repos {
        let checkout = repo_checkout_path(agent_id, &repo.subdir).ok();
        let branch_tip_sha = match &checkout {
            Some(wt) => git::rev_parse(wt, "HEAD").await.ok(),
            None => None,
        };
        // Prefer the immutable fork point; only fall back to resolving the
        // parent branch name (which may have drifted) for pre-migration
        // agents that never captured a base_sha.
        let parent_branch_sha = match &repo.base_sha {
            Some(sha) => Some(sha.clone()),
            None => match &repo.parent_branch {
                Some(b) => git::rev_parse(&repo.repo_path, b).await.ok(),
                None => None,
            },
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
        total_adds = total_adds.saturating_add(adds);
        total_dels = total_dels.saturating_add(dels);

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

    (
        snapshots,
        DiffStats {
            additions: total_adds,
            deletions: total_dels,
        },
    )
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

/// Best-effort teardown of every tracked repo's checkout + branch, plus the
/// agent's parent dir. Failures are logged (tagged with `op` for context) but
/// never abort the sweep — the caller's intent is to get rid of the agent.
/// Shared by archive and discard.
async fn teardown_agent_checkouts(agent_id: &str, repos: &[TrackedRepo], op: &str) {
    for repo in repos {
        let checkout = match repo_checkout_path(agent_id, &repo.subdir) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, subdir = %repo.subdir, op, "checkout path resolution failed");
                continue;
            }
        };
        // A clone is self-contained — `rm -rf` the checkout dir. The agent's
        // branch (if any) lived inside that clone and was never created in the
        // user's source repo, so there is nothing to `branch -D` here: doing so
        // would force-delete an unrelated same-named branch in the source repo.
        if let Err(e) = provision::teardown(&checkout).await {
            tracing::warn!(error = %e, subdir = %repo.subdir, op, "workspace teardown failed");
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
    let _ = remove_agent_dir(agent_id, op).await;
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
                    "checkout dir removal failed after retries; it now orphans the agent \
                     name in the on-disk namespace the allocator consults"
                );
                return false;
            }
        }
    }
    !parent.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
