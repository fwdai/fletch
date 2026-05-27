//! Coordinator between Tauri IPC commands and the running agents.

use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use crate::activity::{Activity, ClaudeManagedActivity, ClaudeNativeActivity};
use crate::agent::{Agent, SpawnSpec};
use crate::branding;
use crate::error::{Error, Result};
use crate::git;
use crate::workspace::{
    agent_parent_dir, allocate_repo_subdir, new_agent_record, repo_worktree_path, AgentRecord,
    AgentStatus, AgentView, ArchiveMetadata, ArchivedRepoSnapshot, DiffStats, TrackedRepo,
    Workspace, WorkspaceManager,
};

#[derive(Clone, serde::Serialize)]
pub struct AgentOutputPayload {
    pub agent_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentEventPayload {
    pub agent_id: String,
    pub event: Value,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentStatusPayload {
    pub agent_id: String,
    pub status: AgentStatus,
    pub last_error: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentViewPayload {
    pub agent_id: String,
    pub view: AgentView,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentTaskPayload {
    pub agent_id: String,
    pub task: String,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentBranchPayload {
    pub agent_id: String,
    pub subdir: String,
    pub branch: String,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentRepoAddedPayload {
    pub agent_id: String,
    pub repo: TrackedRepo,
}

const WATCHDOG_TICK: Duration = Duration::from_millis(500);
const SPAWN_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Supervisor {
    pub workspace: Arc<WorkspaceManager>,
    pub agents: Mutex<HashMap<String, Agent>>,
    pub generations: Mutex<HashMap<String, u64>>,
    pub activities: Mutex<HashMap<String, Box<dyn Activity>>>,
    pub native_input_lines: Mutex<HashMap<String, String>>,
}

impl Supervisor {
    pub fn new(workspace: Arc<WorkspaceManager>) -> Self {
        Self {
            workspace,
            agents: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            activities: Mutex::new(HashMap::new()),
            native_input_lines: Mutex::new(HashMap::new()),
        }
    }

    pub fn current_workspace(&self) -> Option<Workspace> {
        self.workspace.current()
    }

    pub fn add_workspace_repo(&self, repo_path: PathBuf) -> Result<Workspace> {
        self.workspace.add_workspace_repo(repo_path)
    }

    pub fn remove_workspace_repo(&self, repo_path: PathBuf) -> Result<Workspace> {
        self.workspace.remove_workspace_repo(&repo_path)
    }

    pub async fn spawn_agent(
        self: Arc<Self>,
        app: AppHandle,
        view: AgentView,
        repo_path: PathBuf,
        provider: String,
    ) -> Result<AgentRecord> {
        if !repo_path.join(".git").exists() {
            return Err(Error::InvalidPath(format!(
                "not a git repository: {}",
                repo_path.display()
            )));
        }

        let agent_id = self.workspace.allocate_agent_id()?;
        let name = agent_id.clone();

        // Parent_branch captured per-repo; primary's parent is the
        // branch the user was on when they hit Spawn.
        let parent_branch = git::current_branch(&repo_path).await.ok().flatten();
        let subdir = allocate_repo_subdir(&repo_path, &[]);

        let primary = TrackedRepo {
            repo_path: repo_path.clone(),
            subdir: subdir.clone(),
            branch: None, // created later from first user message
            parent_branch,
        };

        let record = new_agent_record(
            agent_id.clone(),
            name,
            provider,
            primary,
            String::new(),
            view,
        );
        let parent_dir = agent_parent_dir(&agent_id)?;
        let primary_worktree = repo_worktree_path(&agent_id, &subdir)?;

        self.workspace.add_agent(record.clone())?;
        emit_status(&app, &agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.clone());

        let sup = self.clone();
        let app_for_task = app.clone();
        let id_for_task = agent_id.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = std::fs::create_dir_all(&parent_dir) {
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
                return;
            }

            if let Err(e) = git::worktree_add_detached(&repo_path, &primary_worktree).await {
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
                return;
            }

            tokio::time::sleep(Duration::from_millis(350)).await;

            if let Err(e) = sup.start_process(&app_for_task, &id_for_task, true).await {
                let _ = git::worktree_remove(&repo_path, &primary_worktree, true).await;
                let _ = std::fs::remove_dir_all(&parent_dir);
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
            }
        });

        Ok(record)
    }

    /// Bring a second (or third…) repo into a live agent. Creates a
    /// detached worktree at `~/.quorum/worktrees/<agent-id>/<subdir>/`
    /// and appends a TrackedRepo entry. If the agent already has a
    /// task set, a branch is created in the new repo immediately;
    /// otherwise we defer (consistent with the primary).
    pub async fn add_repo_to_agent(
        self: Arc<Self>,
        app: AppHandle,
        agent_id: &str,
        repo_path: PathBuf,
    ) -> Result<TrackedRepo> {
        if !repo_path.join(".git").exists() {
            return Err(Error::InvalidPath(format!(
                "not a git repository: {}",
                repo_path.display()
            )));
        }
        let record = self.workspace.agent(agent_id)?;
        if record.repos.iter().any(|r| r.repo_path == repo_path) {
            return Err(Error::Other(
                "this repo is already tracked by the agent".into(),
            ));
        }
        let used: Vec<String> = record.repos.iter().map(|r| r.subdir.clone()).collect();
        let subdir = allocate_repo_subdir(&repo_path, &used);
        let worktree = repo_worktree_path(agent_id, &subdir)?;
        let parent_branch = git::current_branch(&repo_path).await.ok().flatten();

        git::worktree_add_detached(&repo_path, &worktree).await?;

        let repo = TrackedRepo {
            repo_path: repo_path.clone(),
            subdir: subdir.clone(),
            branch: None,
            parent_branch,
        };
        self.workspace
            .append_tracked_repo(agent_id, repo.clone())?;
        let _ = app.emit(
            "agent:repo_added",
            AgentRepoAddedPayload {
                agent_id: agent_id.to_string(),
                repo: repo.clone(),
            },
        );

        // If the agent already has a task (slug), create the branch
        // in the freshly-added repo right away.
        let task = record.task.trim().to_string();
        if !task.is_empty() {
            create_branches_for_branchless_repos(
                self.clone(),
                app.clone(),
                agent_id.to_string(),
                task,
            );
        }

        Ok(repo)
    }

    async fn start_process(
        self: &Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        fresh: bool,
    ) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        let session_id = record
            .session_id
            .clone()
            .ok_or_else(|| Error::Other("agent record missing session_id".into()))?;
        let primary = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no tracked repos".into()))?;
        let cwd = repo_worktree_path(agent_id, &primary.subdir)?;
        let sandbox_root = agent_parent_dir(agent_id)?;

        // Claude only writes a session file once the first turn lands.
        // If task is still empty (no first user message has ever been
        // sent) `--resume <uuid>` will 404. So we treat that case as
        // fresh — same UUID, no replay attempt — and the eventual
        // first message creates the session file. Once that's
        // happened, switch / resume can safely `--resume`.
        let no_messages_yet = record.task.trim().is_empty();
        let effective_fresh = fresh || no_messages_yet;

        let app = app.clone();
        let agent_id_str = agent_id.to_string();

        let my_gen = {
            let mut g = self.generations.lock();
            let entry = g.entry(agent_id_str.clone()).or_insert(0);
            *entry += 1;
            *entry
        };

        let mut activity: Box<dyn Activity> = match record.view {
            AgentView::Native => Box::new(ClaudeNativeActivity::new()),
            AgentView::Custom => Box::new(ClaudeManagedActivity::new()),
        };
        if effective_fresh {
            activity.reset_for_new_turn();
        }
        self.activities.lock().insert(agent_id_str.clone(), activity);

        let spec = SpawnSpec {
            agent_id: &agent_id_str,
            cwd,
            sandbox_root,
            session_id: &session_id,
            fresh: effective_fresh,
            cols: 120,
            rows: 32,
        };

        let agent = match record.view {
            AgentView::Native => spawn_pty_agent(
                spec,
                app.clone(),
                agent_id_str.clone(),
                self.clone(),
                my_gen,
            )?,
            AgentView::Custom => spawn_managed_agent(
                spec,
                app.clone(),
                agent_id_str.clone(),
                self.clone(),
                my_gen,
            )?,
        };

        self.agents.lock().insert(agent_id_str.clone(), agent);

        // Initial status is always Idle now — at process start there's
        // never an in-flight turn (we no longer pass a task as a spawn
        // arg). The user's first send flips it to Running.
        let changed = self.workspace.update_agent_status_if(
            &agent_id_str,
            AgentStatus::Idle,
            None,
            |status| matches!(status, AgentStatus::Spawning),
        );
        if matches!(changed, Ok(true)) {
            emit_status(&app, &agent_id_str, AgentStatus::Idle, None);
        }

        spawn_turn_watchdog(self.clone(), app, agent_id_str, my_gen);

        Ok(())
    }

    pub fn resume_persisted_agents(self: Arc<Self>, app: AppHandle) {
        let agents = match self.workspace.current() {
            Some(ws) => ws.agents,
            None => return,
        };

        for record in agents {
            if !matches!(record.status, AgentStatus::Spawning) {
                continue;
            }
            if record.session_id.is_none() || record.repos.is_empty() {
                let err = "Agent record incomplete (no session id / no repos). Remove and respawn.".to_string();
                let _ = self.workspace.update_agent_status(
                    &record.id,
                    AgentStatus::Error,
                    Some(err.clone()),
                );
                emit_status(&app, &record.id, AgentStatus::Error, Some(err));
                continue;
            }

            let sup = self.clone();
            let app = app.clone();
            let id = record.id.clone();
            arm_spawn_timeout(sup.clone(), app.clone(), id.clone());
            tauri::async_runtime::spawn(async move {
                if let Err(e) = sup.start_process(&app, &id, false).await {
                    fail_spawn(&sup, &app, &id, e.to_string());
                }
            });
        }
    }

    pub async fn resume_agent(
        self: Arc<Self>,
        app: AppHandle,
        agent_id: &str,
    ) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        if self.agents.lock().contains_key(agent_id) {
            return Ok(());
        }
        if record.session_id.is_none() {
            return Err(Error::Other(
                "Agent has no session id; remove and respawn.".into(),
            ));
        }
        self.workspace
            .update_agent_status(agent_id, AgentStatus::Spawning, None)?;
        emit_status(&app, agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.to_string());

        self.start_process(&app, agent_id, false).await
    }

    pub fn write_to_agent(
        self: Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        bytes: &[u8],
    ) -> Result<()> {
        {
            let agents = self.agents.lock();
            let agent = agents
                .get(agent_id)
                .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
            agent.write_pty(bytes)?;
        }
        for submitted in observe_native_input(&self, agent_id, bytes) {
            mark_user_turn_started(&self, app, agent_id);
            on_first_user_message(self.clone(), app.clone(), agent_id.to_string(), submitted);
        }
        Ok(())
    }

    pub fn send_user_message(
        self: Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        text: &str,
    ) -> Result<()> {
        {
            let agents = self.agents.lock();
            let agent = agents
                .get(agent_id)
                .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
            agent.send_user_message(text)?;
        }
        mark_user_turn_started(&self, app, agent_id);
        on_first_user_message(self.clone(), app.clone(), agent_id.to_string(), text.to_string());
        Ok(())
    }

    pub fn resize_agent(&self, agent_id: &str, cols: u16, rows: u16) -> Result<()> {
        let agents = self.agents.lock();
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| Error::AgentNotFound(agent_id.to_string()))?;
        agent.resize(cols, rows)
    }

    pub async fn switch_view(
        self: Arc<Self>,
        app: AppHandle,
        agent_id: &str,
        new_view: AgentView,
    ) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        if record.view == new_view {
            return Ok(());
        }

        if let Some(agent) = self.agents.lock().remove(agent_id) {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.native_input_lines.lock().remove(agent_id);

        self.workspace.update_agent_view(agent_id, new_view)?;
        let _ = app.emit(
            "agent:view",
            AgentViewPayload {
                agent_id: agent_id.to_string(),
                view: new_view,
            },
        );
        self.workspace
            .update_agent_status(agent_id, AgentStatus::Spawning, None)?;
        emit_status(&app, agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.to_string());

        tokio::time::sleep(Duration::from_millis(150)).await;

        if let Err(e) = self.start_process(&app, agent_id, false).await {
            let err = e.to_string();
            let _ = self.workspace.update_agent_status(
                agent_id,
                AgentStatus::Error,
                Some(err.clone()),
            );
            emit_status(&app, agent_id, AgentStatus::Error, Some(err));
            return Err(e);
        }
        Ok(())
    }

    pub async fn stop_agent(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        // Interrupt the current turn without exiting the process.
        // The natural result-event + turn-watchdog path will transition
        // the agent to Idle once the interrupt is processed. If the
        // process does exit (e.g. it doesn't survive SIGINT), the exit
        // handler in apply_exit_if_current will also move it to Idle.
        let _ = app;
        let agents = self.agents.lock();
        if let Some(agent) = agents.get(agent_id) {
            agent.interrupt();
        }
        Ok(())
    }

    /// Move an agent into the History view: stop the process if any,
    /// snapshot each tracked repo's SHA + diff stats, then tear down
    /// the worktrees and branches. The claude session JSONL is left
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
        if matches!(record.status, AgentStatus::Spawning | AgentStatus::Running) {
            return Err(Error::Other(
                "agent must be idle, stopped, or in error before archiving".into(),
            ));
        }

        if let Some(agent) = self.agents.lock().remove(agent_id) {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.native_input_lines.lock().remove(agent_id);

        let mut snapshots: Vec<ArchivedRepoSnapshot> = Vec::with_capacity(record.repos.len());
        let mut total_adds: u32 = 0;
        let mut total_dels: u32 = 0;

        for repo in &record.repos {
            // Resolve SHAs first so we capture state before any
            // destructive step.
            let branch_tip_sha = if let Some(b) = &repo.branch {
                git::rev_parse(&repo.repo_path, b).await.ok()
            } else {
                None
            };
            let parent_branch_sha = if let Some(b) = &repo.parent_branch {
                git::rev_parse(&repo.repo_path, b).await.ok()
            } else {
                None
            };

            let mut adds = 0u32;
            let mut dels = 0u32;
            if let (Some(from), Some(to)) = (&parent_branch_sha, &branch_tip_sha) {
                if from != to {
                    if let Ok((a, d)) = git::diff_shortstat(&repo.repo_path, from, to).await {
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

        // Tear down worktrees + branches (best-effort: a single failure
        // shouldn't block archive, since the user's intent is "get rid
        // of this", but we surface git errors via tracing).
        for repo in &record.repos {
            let worktree = match repo_worktree_path(agent_id, &repo.subdir) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, subdir = %repo.subdir, "archive: worktree_path failed");
                    continue;
                }
            };
            let _ = git::worktree_prune(&repo.repo_path).await;
            if let Err(e) = git::worktree_remove(&repo.repo_path, &worktree, true).await {
                tracing::warn!(error = %e, subdir = %repo.subdir, "archive: worktree remove failed");
            }
            if let Some(branch) = &repo.branch {
                if let Err(e) = git::branch_delete(&repo.repo_path, branch).await {
                    tracing::warn!(%branch, error = %e, "archive: branch delete failed");
                }
            }
        }

        // Best-effort parent dir cleanup.
        if let Ok(parent) = agent_parent_dir(agent_id) {
            if parent.exists() {
                let _ = std::fs::remove_dir_all(&parent);
            }
        }

        let archive = ArchiveMetadata {
            archived_at: chrono::Utc::now().to_rfc3339(),
            repos: snapshots,
            diff_stats: DiffStats {
                additions: total_adds,
                deletions: total_dels,
            },
        };

        self.workspace.archive_agent(agent_id, archive)?;
        // The frontend listens to `agent:status` to drive most UI;
        // archive is structurally a deeper change, so we re-emit the
        // workspace via a tiny event. Frontend already reloads on this
        // signal via `get_workspace`.
        let _ = app.emit("workspace:changed", ());
        Ok(())
    }

    /// Pull an archived agent back into the live sidebar: recreate
    /// branches and worktrees from snapshot SHAs, clear archive
    /// metadata, transition to Spawning so the supervisor's start path
    /// attaches to the existing claude session.
    pub async fn restore_agent(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
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

        // Pre-flight: every snapshot must have a tip SHA, and that SHA
        // must still be reachable. We do this before any mutation so
        // we don't leave a half-restored agent on failure.
        for snap in &archive.repos {
            let sha = snap.branch_tip_sha.as_deref().ok_or_else(|| {
                Error::Other(format!(
                    "snapshot for repo `{}` has no branch tip SHA",
                    snap.subdir
                ))
            })?;
            git::rev_parse(&snap.repo_path, sha).await.map_err(|e| {
                Error::Other(format!(
                    "branch tip {} no longer reachable in {}: {e}",
                    sha,
                    snap.repo_path.display()
                ))
            })?;
        }

        // Ensure the agent parent dir exists.
        let parent_dir = agent_parent_dir(agent_id)?;
        std::fs::create_dir_all(&parent_dir)
            .map_err(|e| Error::Other(format!("create parent dir: {e}")))?;

        let mut restored: Vec<TrackedRepo> = Vec::with_capacity(archive.repos.len());
        for snap in &archive.repos {
            let tip_sha = snap.branch_tip_sha.as_deref().expect("checked above");
            let desired_name = snap
                .branch_name
                .clone()
                .unwrap_or_else(|| format!("quorum/{}-restored", agent_id));

            // Resolve branch name collisions by appending -restored / -restored-N.
            let mut chosen = desired_name.clone();
            let mut bumps = 0;
            loop {
                let exists = git::branch_exists(&snap.repo_path, &chosen).await.unwrap_or(false);
                if !exists {
                    break;
                }
                bumps += 1;
                chosen = if bumps == 1 {
                    format!("{desired_name}-restored")
                } else {
                    format!("{desired_name}-restored-{bumps}")
                };
            }

            git::branch_create_at(&snap.repo_path, &chosen, tip_sha).await?;

            let worktree = repo_worktree_path(agent_id, &snap.subdir)?;
            if let Some(parent) = worktree.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| Error::Other(format!("create worktree parent: {e}")))?;
            }
            git::worktree_add_branch(&snap.repo_path, &worktree, &chosen).await?;

            restored.push(TrackedRepo {
                repo_path: snap.repo_path.clone(),
                subdir: snap.subdir.clone(),
                branch: Some(chosen),
                parent_branch: snap.parent_branch.clone(),
            });
        }

        self.workspace.restore_agent(agent_id, restored)?;
        emit_status(&app, agent_id, AgentStatus::Spawning, None);
        let _ = app.emit("workspace:changed", ());

        // Kick the resume path. start_process is the same one that
        // resume_persisted_agents uses on app boot.
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

    /// Read the persisted claude session JSONL for an archived (or
    /// live) agent and return the events as a Vec<Value>. The frontend
    /// feeds these through the same event handler it uses for live
    /// stream-json output, so we don't need a parallel renderer.
    ///
    /// Returns an empty vec if the JSONL is missing (claude pruned it,
    /// user deleted it, session never reached the first turn).
    pub fn read_session_transcript(&self, agent_id: &str) -> Result<Vec<Value>> {
        let record = self.workspace.agent(agent_id)?;
        let session_id = record
            .session_id
            .as_deref()
            .ok_or_else(|| Error::Other("agent has no session id".into()))?;
        let path = match find_session_jsonl(session_id) {
            Some(p) => p,
            None => return Ok(Vec::new()),
        };
        let file = std::fs::File::open(&path)
            .map_err(|e| Error::Other(format!("open transcript: {e}")))?;
        let reader = std::io::BufReader::new(file);
        let mut out = Vec::new();
        use std::io::BufRead;
        for line in reader.lines().map_while(std::result::Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                out.push(v);
            }
        }
        Ok(out)
    }

    pub async fn discard_agent(self: Arc<Self>, agent_id: &str) -> Result<()> {
        let record = self.workspace.agent(agent_id).ok();
        let repos = record.as_ref().map(|r| r.repos.clone()).unwrap_or_default();
        let parent_dir = agent_parent_dir(agent_id).ok();

        if let Some(agent) = self.agents.lock().remove(agent_id) {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.native_input_lines.lock().remove(agent_id);

        // Tear down each tracked repo's worktree + branch.
        for repo in &repos {
            let worktree = match repo_worktree_path(agent_id, &repo.subdir) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, subdir = %repo.subdir, "discard: worktree_path failed");
                    continue;
                }
            };
            let _ = git::worktree_prune(&repo.repo_path).await;
            if let Err(e) = git::worktree_remove(&repo.repo_path, &worktree, true).await {
                tracing::warn!(error = %e, subdir = %repo.subdir, "discard: worktree remove failed");
            }
            if let Some(branch) = &repo.branch {
                if let Err(e) = git::branch_delete(&repo.repo_path, branch).await {
                    tracing::warn!(%branch, error = %e, "discard: branch delete failed");
                }
            }
        }

        // Remove the parent dir (may contain orphan files if any
        // worktree removal failed). Best-effort.
        if let Some(parent) = parent_dir {
            if parent.exists() {
                let _ = std::fs::remove_dir_all(&parent);
            }
        }

        self.workspace.remove_agent(agent_id)?;
        Ok(())
    }
}

/// Locate the claude session JSONL by scanning `~/.claude/projects/*/`
/// for `<session-id>.jsonl`. Claude's path-encoding scheme isn't part
/// of its public API, so we glob instead of recomputing the encoded
/// directory name from the worktree path.
fn find_session_jsonl(session_id: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let projects = home.join(".claude").join("projects");
    let entries = std::fs::read_dir(&projects).ok()?;
    let filename = format!("{session_id}.jsonl");
    for entry in entries.flatten() {
        let path = entry.path().join(&filename);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn spawn_pty_agent(
    spec: SpawnSpec<'_>,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    let app_for_output = app.clone();
    let id_for_output = agent_id.clone();
    let sup_for_output = sup.clone();
    let sup_for_exit = sup;
    let app_for_exit = app.clone();
    let id_for_exit = agent_id.clone();
    Agent::spawn_pty(
        spec,
        move |bytes| {
            if let Some(activity) = sup_for_output
                .activities
                .lock()
                .get_mut(&id_for_output)
            {
                activity.observe_bytes(&bytes);
            }

            if let Err(e) = app_for_output.emit(
                "agent:output",
                AgentOutputPayload {
                    agent_id: id_for_output.clone(),
                    bytes,
                },
            ) {
                tracing::warn!(error = %e, agent_id = %id_for_output, "emit agent:output failed");
            }
        },
        move |exit| {
            apply_exit_if_current(&sup_for_exit, &app_for_exit, &id_for_exit, gen, exit.success, exit.message);
        },
    )
}

fn spawn_managed_agent(
    spec: SpawnSpec<'_>,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    let app_for_event = app.clone();
    let id_for_event = agent_id.clone();
    let sup_for_event = sup.clone();
    let sup_for_exit = sup;
    let app_for_exit = app.clone();
    let id_for_exit = agent_id.clone();
    Agent::spawn_managed(
        spec,
        move |event| {
            if let Some(activity) = sup_for_event
                .activities
                .lock()
                .get_mut(&id_for_event)
            {
                activity.observe_event(&event);
            }

            if let Err(e) = app_for_event.emit(
                "agent:event",
                AgentEventPayload {
                    agent_id: id_for_event.clone(),
                    event,
                },
            ) {
                tracing::warn!(error = %e, agent_id = %id_for_event, "emit agent:event failed");
            }
        },
        move |exit| {
            apply_exit_if_current(&sup_for_exit, &app_for_exit, &id_for_exit, gen, exit.success, exit.message);
        },
    )
}

fn observe_native_input(sup: &Supervisor, agent_id: &str, bytes: &[u8]) -> Vec<String> {
    let mut submitted = Vec::new();
    let mut lines = sup.native_input_lines.lock();
    let line = lines.entry(agent_id.to_string()).or_default();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'\r' | b'\n' => {
                let trimmed = line.trim().to_string();
                line.clear();
                if !trimmed.is_empty() {
                    submitted.push(trimmed);
                }
                i += 1;
            }
            0x7f | 0x08 => {
                line.pop();
                i += 1;
            }
            0x03 | 0x15 => {
                line.clear();
                i += 1;
            }
            0x1b => {
                i = skip_escape_sequence(bytes, i);
            }
            b if b < 0x20 => {
                i += 1;
            }
            _ => match std::str::from_utf8(&bytes[i..]) {
                Ok(rest) => {
                    if let Some(ch) = rest.chars().next() {
                        line.push(ch);
                        i += ch.len_utf8();
                    } else {
                        break;
                    }
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if valid > 0 {
                        if let Ok(s) = std::str::from_utf8(&bytes[i..i + valid]) {
                            if let Some(ch) = s.chars().next() {
                                line.push(ch);
                                i += ch.len_utf8();
                            } else {
                                i += valid;
                            }
                        } else {
                            i += valid;
                        }
                    } else {
                        i += 1;
                    }
                }
            },
        }
    }

    submitted
}

fn skip_escape_sequence(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    if i < bytes.len() && bytes[i] == b'[' {
        i += 1;
        while i < bytes.len() {
            let b = bytes[i];
            i += 1;
            if (0x40..=0x7e).contains(&b) {
                break;
            }
        }
        return i;
    }
    if i < bytes.len() {
        i + 1
    } else {
        i
    }
}

/// Fire-and-forget handler for the user's first message: persists it
/// as the agent's `task` and kicks branch creation for every
/// branchless tracked repo.
fn on_first_user_message(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    text: String,
) {
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return;
    }
    if trimmed.starts_with('/') {
        return;
    }

    match sup.workspace.set_agent_task_if_empty(&agent_id, &trimmed) {
        Ok(true) => {
            let _ = app.emit(
                "agent:task",
                AgentTaskPayload {
                    agent_id: agent_id.clone(),
                    task: trimmed.clone(),
                },
            );
        }
        Ok(false) => {} // task already set
        Err(e) => {
            tracing::warn!(error = %e, agent_id = %agent_id, "set_agent_task_if_empty failed");
        }
    }

    create_branches_for_branchless_repos(sup, app, agent_id, trimmed);
}

/// For every tracked repo on the agent that doesn't have a branch yet,
/// derive a slug from the agent's task and create `quorum/<slug>` inside
/// that repo's worktree. Runs in a background task. Idempotent —
/// `set_repo_branch_if_empty` guards each write.
fn create_branches_for_branchless_repos(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    task: String,
) {
    tauri::async_runtime::spawn(async move {
        let record = match sup.workspace.agent(&agent_id) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, agent_id = %agent_id, "branch creation: agent lookup failed");
                return;
            }
        };

        let slug_base = branding::slugify_task(&task);
        let slug = if slug_base.is_empty() {
            agent_id.clone()
        } else {
            slug_base
        };

        for repo in record.repos.iter() {
            if repo.branch.is_some() {
                continue;
            }
            let worktree = match repo_worktree_path(&agent_id, &repo.subdir) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, subdir = %repo.subdir, "branch creation: worktree_path failed");
                    continue;
                }
            };
            let mut branch = branding::branch_for(&slug);
            if git::branch_exists(&repo.repo_path, &branch)
                .await
                .unwrap_or(false)
            {
                branch = format!("{branch}-{agent_id}");
            }
            if let Err(e) = git::checkout_new_branch(&worktree, &branch).await {
                tracing::warn!(
                    error = %e,
                    agent_id = %agent_id,
                    subdir = %repo.subdir,
                    branch = %branch,
                    "checkout_new_branch failed"
                );
                continue;
            }
            match sup
                .workspace
                .set_repo_branch_if_empty(&agent_id, &repo.subdir, &branch)
            {
                Ok(true) => {
                    let _ = app.emit(
                        "agent:branch",
                        AgentBranchPayload {
                            agent_id: agent_id.clone(),
                            subdir: repo.subdir.clone(),
                            branch: branch.clone(),
                        },
                    );
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "set_repo_branch_if_empty failed");
                }
            }
        }
    });
}

fn mark_user_turn_started(sup: &Supervisor, app: &AppHandle, agent_id: &str) {
    if let Some(activity) = sup.activities.lock().get_mut(agent_id) {
        activity.reset_for_new_turn();
    }
    transition_active(sup, app, agent_id, AgentStatus::Running);
}

fn spawn_turn_watchdog(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    gen: u64,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(WATCHDOG_TICK).await;

            let current_gen = sup
                .generations
                .lock()
                .get(&agent_id)
                .copied()
                .unwrap_or(0);
            if current_gen != gen {
                return;
            }

            let ended = sup
                .activities
                .lock()
                .get(&agent_id)
                .map(|a| a.turn_ended())
                .unwrap_or(false);

            if ended {
                transition_active(&sup, &app, &agent_id, AgentStatus::Idle);
            }
        }
    });
}

fn arm_spawn_timeout(sup: Arc<Supervisor>, app: AppHandle, agent_id: String) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SPAWN_TIMEOUT).await;
        let still_spawning = sup
            .workspace
            .agent(&agent_id)
            .map(|r| matches!(r.status, AgentStatus::Spawning))
            .unwrap_or(false);
        if !still_spawning {
            return;
        }
        let err = "Spawn timed out after 15s — process did not become ready.".to_string();
        let _ = sup.workspace.update_agent_status(
            &agent_id,
            AgentStatus::Error,
            Some(err.clone()),
        );
        emit_status(&app, &agent_id, AgentStatus::Error, Some(err));
        if let Some(agent) = sup.agents.lock().remove(&agent_id) {
            let _ = agent.shutdown();
        }
        sup.activities.lock().remove(&agent_id);
    });
}

fn fail_spawn(sup: &Supervisor, app: &AppHandle, agent_id: &str, err: String) {
    let _ = sup
        .workspace
        .update_agent_status(agent_id, AgentStatus::Error, Some(err.clone()));
    emit_status(app, agent_id, AgentStatus::Error, Some(err));
}

fn transition_active(
    sup: &Supervisor,
    app: &AppHandle,
    agent_id: &str,
    new: AgentStatus,
) {
    let changed = sup.workspace.update_agent_status_if(
        agent_id,
        new.clone(),
        None,
        |cur| {
            matches!(
                cur,
                AgentStatus::Spawning | AgentStatus::Running | AgentStatus::Idle
            ) && *cur != new
        },
    );
    if matches!(changed, Ok(true)) {
        emit_status(app, agent_id, new, None);
    }
}

fn apply_exit_if_current(
    sup: &Supervisor,
    app: &AppHandle,
    agent_id: &str,
    gen: u64,
    success: bool,
    message: String,
) {
    let current = sup
        .generations
        .lock()
        .get(agent_id)
        .copied()
        .unwrap_or(0);
    if current != gen {
        tracing::debug!(
            agent_id = %agent_id,
            stale_gen = gen,
            current_gen = current,
            "ignoring exit from prior generation"
        );
        return;
    }

    sup.agents.lock().remove(agent_id);
    sup.activities.lock().remove(agent_id);
    sup.native_input_lines.lock().remove(agent_id);

    let (status, err) = if success {
        // Clean exit means the agent is resumable — keep it Idle so the
        // user can send follow-up messages without a manual Resume step.
        (AgentStatus::Idle, None)
    } else {
        (AgentStatus::Error, Some(format!("Agent process exited: {message}")))
    };
    let changed = sup.workspace.update_agent_status_if(
        agent_id,
        status.clone(),
        err.clone(),
        |status| {
            matches!(
                status,
                AgentStatus::Running | AgentStatus::Idle | AgentStatus::Spawning
            )
        },
    );
    if matches!(changed, Ok(true)) {
        emit_status(app, agent_id, status, err);
    }
}

fn emit_status(
    app: &AppHandle,
    agent_id: &str,
    status: AgentStatus,
    last_error: Option<String>,
) {
    let _ = app.emit(
        "agent:status",
        AgentStatusPayload {
            agent_id: agent_id.to_string(),
            status,
            last_error,
        },
    );
}
