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
    AgentStatus, AgentView, TrackedRepo, Workspace, WorkspaceManager,
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
}

impl Supervisor {
    pub fn new(workspace: Arc<WorkspaceManager>) -> Self {
        Self {
            workspace,
            agents: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            activities: Mutex::new(HashMap::new()),
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
        mark_user_turn_started(&self, app, agent_id);
        let text = String::from_utf8_lossy(bytes);
        let trimmed = text.trim_end_matches(['\r', '\n']);
        on_first_user_message(self.clone(), app.clone(), agent_id.to_string(), trimmed.to_string());
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
        let live = self.agents.lock().remove(agent_id);
        if let Some(agent) = live {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        match self
            .workspace
            .update_agent_status(agent_id, AgentStatus::Stopped, None)
        {
            Ok(_) | Err(Error::AgentNotFound(_)) => {}
            Err(e) => return Err(e),
        }
        emit_status(&app, agent_id, AgentStatus::Stopped, None);
        Ok(())
    }

    pub async fn discard_agent(self: Arc<Self>, agent_id: &str) -> Result<()> {
        let record = self.workspace.agent(agent_id).ok();
        let repos = record.as_ref().map(|r| r.repos.clone()).unwrap_or_default();
        let parent_dir = agent_parent_dir(agent_id).ok();

        if let Some(agent) = self.agents.lock().remove(agent_id) {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);

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

    let (status, err) = if success {
        (AgentStatus::Stopped, None)
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
