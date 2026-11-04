//! The `AgentDriver` seam (spec §3.2). The kernel never talks to the supervisor
//! directly — it goes through this trait so every run behavior is unit-testable
//! against a stub driver with scripted status sequences. `SupervisorDriver` is
//! the one production implementation; it is a thin adapter over `Supervisor`.
//!
//! This is the *only* sanctioned abstraction layer in the workflow engine: it
//! exists for testability, not generality.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::error::Result;
use crate::host::EngineCtx;
use crate::supervisor::{SpawnRequest, StatusEvent, Supervisor};
use crate::workspace::{AgentStatus, AgentView};

/// A `Send` boxed future — the object-safe return of the async trait methods
/// below. This is the exact shape `#[async_trait]` desugars to; we spell it out
/// by hand so the workflow engine adds no new crate dependency (and its lockfile
/// stays in step with the pinned build cache).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Everything the kernel supplies to spawn one step agent (spec §3.2).
pub struct SpawnReq {
    /// The user's repo the run was cloned from.
    pub repo_path: PathBuf,
    /// Provider id (`claude` | `codex` | …).
    pub provider: String,
    pub model: Option<String>,
    /// Session-level reasoning effort (claude `--effort`) for this step. Resolved
    /// by the kernel from the alias's `effort` override, falling back to the
    /// linked custom agent's effort (§3.2); `None` uses the provider default.
    pub effort: Option<String>,
    pub instructions: Option<String>,
    /// Local custom-agent id, when the step's agent alias maps to one.
    pub custom_agent_id: Option<String>,
    pub skills: Vec<crate::agent_profile::SkillSnapshot>,
    pub mcp_servers: Vec<crate::agent_profile::McpServerSnapshot>,
    /// Where the step starts: the run's `base_sha` for step 1, the previous
    /// step's `refs/wf/steps/<exec>` after that. The adopted workspace is
    /// already there; this becomes the agent's recorded parent.
    pub fork_base: Option<String>,
    /// The run that owns this agent — persisted on the record so run-owned
    /// agents are hidden from the normal sidebar and cleaned up by
    /// `wf_delete_run`. The blackboard write-grant is derived from it at spawn,
    /// keeping the run directory the single source of truth.
    pub owner_run_id: String,
    /// The pre-provisioned checkout the agent adopts as its primary workspace
    /// instead of cloning one — the run's shared workspace, where every step
    /// works in the same tree. Teardown (archive / delete) must never remove the
    /// directory: the run owns it, not the agent.
    pub existing_workspace: Option<PathBuf>,
}

/// A freshly spawned step agent. It works in the workspace the request named,
/// so its id is all the kernel needs back.
pub struct SpawnedAgent {
    pub agent_id: String,
}

pub trait AgentDriver: Send + Sync {
    /// Spawn a step agent forked from `fork_base`.
    fn spawn(&self, req: SpawnReq) -> BoxFuture<'_, Result<SpawnedAgent>>;
    /// Current authoritative status of an agent (`None` once it's gone).
    fn status(&self, agent_id: &str) -> Option<AgentStatus>;
    /// Subscribe to status transitions. To avoid races the caller MUST
    /// subscribe first, then read `status()`, then loop on `recv()`.
    fn subscribe(&self) -> broadcast::Receiver<StatusEvent>;
    /// Deliver a prompt/message (routes through the persisted message queue:
    /// mid-turn injection where supported, else the turn boundary).
    fn send_message<'a>(&'a self, agent_id: &'a str, text: String) -> BoxFuture<'a, Result<()>>;
    fn stop<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, Result<()>>;
    /// Archive (never delete) a step agent so its chat stays replayable.
    fn archive<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, Result<()>>;
}

/// The production driver: a thin adapter over the agent [`Supervisor`].
pub struct SupervisorDriver {
    sup: Arc<Supervisor>,
    engine: Arc<EngineCtx>,
}

impl SupervisorDriver {
    pub fn new(sup: Arc<Supervisor>, engine: Arc<EngineCtx>) -> Self {
        Self { sup, engine }
    }
}

impl AgentDriver for SupervisorDriver {
    fn spawn(&self, req: SpawnReq) -> BoxFuture<'_, Result<SpawnedAgent>> {
        Box::pin(async move {
            let SpawnReq {
                repo_path,
                provider,
                model,
                effort,
                instructions,
                custom_agent_id,
                skills,
                mcp_servers,
                fork_base,
                owner_run_id,
                existing_workspace,
            } = req;

            let record = self
                .sup
                .clone()
                .spawn_agent(
                    self.engine.clone(),
                    SpawnRequest {
                        // Step agents render in the structured (Custom) view.
                        view: AgentView::Custom,
                        repo_path,
                        provider,
                        name: None,
                        effort,
                        model,
                        instructions,
                        // Workflow steps are not forks; no carried conversation.
                        handoff: None,
                        lineage: None,
                        custom_agent_id,
                        skills,
                        mcp_servers,
                        fork_base,
                        owner_run_id: Some(owner_run_id),
                        existing_workspace,
                        code_from: None,
                        // Workflow-step spawns aren't issue-intake spawns.
                        issue_ref: None,
                        // A step agent is hidden by its `owner_run_id`, not by a
                        // purpose tag — the run monitor owns it.
                        purpose: None,
                        // The step prompt is delivered as a message; the task is
                        // captured from it then.
                        task: None,
                        // Steps run under the account active in Settings.
                        account: None,
                        // A step works in its run's tree only.
                        folder_grants: Vec::new(),
                        // No per-session choice: a step's engine is the host's
                        // and the project's, or its adopted tree's.
                        workspace_type: None,
                    },
                )
                .await?;
            Ok(SpawnedAgent {
                agent_id: record.id,
            })
        })
    }

    fn status(&self, agent_id: &str) -> Option<AgentStatus> {
        self.sup.status_of(agent_id)
    }

    fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.sup.subscribe_status()
    }

    fn send_message<'a>(&'a self, agent_id: &'a str, text: String) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // A run-owned agent's prompts are engine-composed; each is one user
            // turn, so a fresh turn id per send is correct.
            let turn_id = uuid::Uuid::new_v4().to_string();
            self.sup
                .clone()
                .send_user_message(&self.engine, agent_id, &turn_id, &text, &[])
                .await?;
            Ok(())
        })
    }

    fn stop<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.sup
                .clone()
                .stop_agent(self.engine.clone(), agent_id)
                .await
        })
    }

    fn archive<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.sup
                .clone()
                .archive_agent(self.engine.clone(), agent_id)
                .await
        })
    }
}

/// A scriptable [`AgentDriver`] for service tests. Spawned agents start
/// `Spawning`; a test drives the status timeline with [`MockDriver::set_status`]
/// and reads back which agents were stopped.
#[cfg(test)]
pub(crate) struct MockDriver {
    state: parking_lot::Mutex<MockState>,
    status_tx: broadcast::Sender<StatusEvent>,
}

#[cfg(test)]
#[derive(Default)]
struct MockState {
    statuses: std::collections::HashMap<String, AgentStatus>,
    stopped: Vec<String>,
    spawn_count: usize,
    /// When set, `spawn` fails with this message (spawn-failure tests).
    fail_spawn: Option<String>,
}

#[cfg(test)]
impl MockDriver {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: parking_lot::Mutex::new(MockState::default()),
            status_tx: broadcast::channel(1024).0,
        })
    }

    /// Record a status for `agent_id` and broadcast the transition (exactly what
    /// the supervisor's choke point does in production).
    pub(crate) fn set_status(&self, agent_id: &str, status: AgentStatus) {
        self.state
            .lock()
            .statuses
            .insert(agent_id.to_string(), status.clone());
        let _ = self.status_tx.send(StatusEvent {
            agent_id: agent_id.to_string(),
            status,
        });
    }

    pub(crate) fn fail_next_spawn(&self, msg: &str) {
        self.state.lock().fail_spawn = Some(msg.to_string());
    }

    pub(crate) fn was_stopped(&self, agent_id: &str) -> bool {
        self.state.lock().stopped.iter().any(|a| a == agent_id)
    }
}

#[cfg(test)]
impl AgentDriver for MockDriver {
    fn spawn(&self, _req: SpawnReq) -> BoxFuture<'_, Result<SpawnedAgent>> {
        Box::pin(async move {
            let agent_id = {
                let mut st = self.state.lock();
                if let Some(msg) = st.fail_spawn.take() {
                    return Err(crate::error::Error::Other(msg));
                }
                st.spawn_count += 1;
                let agent_id = format!("mock-agent-{}", st.spawn_count);
                st.statuses.insert(agent_id.clone(), AgentStatus::Spawning);
                agent_id
            };
            Ok(SpawnedAgent { agent_id })
        })
    }

    fn status(&self, agent_id: &str) -> Option<AgentStatus> {
        self.state.lock().statuses.get(agent_id).cloned()
    }

    fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.status_tx.subscribe()
    }

    fn send_message<'a>(&'a self, _agent_id: &'a str, _text: String) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }

    fn stop<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.state.lock().stopped.push(agent_id.to_string());
            Ok(())
        })
    }

    fn archive<'a>(&'a self, _agent_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> SpawnReq {
        SpawnReq {
            repo_path: PathBuf::from("/r"),
            provider: "claude".into(),
            model: None,
            effort: None,
            instructions: None,
            custom_agent_id: None,
            skills: vec![],
            mcp_servers: vec![],
            fork_base: None,
            owner_run_id: "run-1".into(),
            existing_workspace: Some(PathBuf::from("/runs/run-1/repo")),
        }
    }

    #[tokio::test]
    async fn mock_spawn_starts_spawning_and_broadcasts() {
        let d = MockDriver::new();
        let mut rx = d.subscribe();
        let spawned = d.spawn(req()).await.unwrap();
        assert_eq!(d.status(&spawned.agent_id), Some(AgentStatus::Spawning));

        d.set_status(&spawned.agent_id, AgentStatus::Idle);
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.agent_id, spawned.agent_id);
        assert_eq!(ev.status, AgentStatus::Idle);
    }

    #[tokio::test]
    async fn mock_spawn_failure_surfaces() {
        let d = MockDriver::new();
        d.fail_next_spawn("boom");
        assert!(d.spawn(req()).await.is_err());
    }
}
