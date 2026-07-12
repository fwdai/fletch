//! The `AgentDriver` seam (spec §3.2). The scheduler never talks to the
//! supervisor directly — it goes through this trait so every scheduler and
//! attempt behavior is unit-testable against a `MockDriver` with scripted
//! status sequences. `SupervisorDriver` is the one production implementation;
//! it is a thin adapter over `Supervisor`.
//!
//! This is the *only* sanctioned abstraction layer in the workflow engine (see
//! SLICES.md guiding constraints): it exists for testability, not generality.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use tauri::AppHandle;
use tokio::sync::broadcast;

use crate::error::Result;
use crate::supervisor::{SpawnRequest, StatusEvent, Supervisor};
use crate::workspace::{AgentStatus, AgentView};

/// A `Send` boxed future — the object-safe return of the async trait methods
/// below. This is the exact shape `#[async_trait]` desugars to; we spell it out
/// by hand so the workflow engine adds no new crate dependency (and its lockfile
/// stays in step with the pinned build cache).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Everything the scheduler supplies to spawn one step agent (spec §3.2).
pub struct SpawnReq {
    /// Repo the step's workspace forks from.
    pub repo_path: PathBuf,
    /// Provider id (`claude` | `codex` | …).
    pub provider: String,
    pub model: Option<String>,
    pub instructions: Option<String>,
    /// Local custom-agent id, when the step's agent alias maps to one.
    pub custom_agent_id: Option<String>,
    pub skills: Vec<crate::agent_profile::SkillSnapshot>,
    pub mcp_servers: Vec<crate::agent_profile::McpServerSnapshot>,
    /// The fork source: a ref/commit-ish. For step 1 it is the run's `base_sha`
    /// (present in a fresh source clone); for step N it is `refs/wf/steps/<prev>`,
    /// resolvable only after fetching it from the run repo (§12.1).
    pub fork_base: Option<String>,
    /// The run repository (`~/.fletch/runs/<id>/repo`) the fork ref is fetched
    /// from before detaching (§12.1). `Some` for every workflow step.
    pub run_repo: Option<PathBuf>,
    /// The run that owns this agent — persisted on the record so run-owned
    /// agents are hidden from the normal sidebar and cleaned up by
    /// `wf_delete_run`. The blackboard write-grant is derived from it at spawn,
    /// keeping the run directory the single source of truth.
    pub owner_run_id: String,
}

/// A freshly spawned step agent.
pub struct SpawnedAgent {
    pub agent_id: String,
    /// The agent's primary checkout path — where gates read git/artifact facts.
    pub worktree: PathBuf,
}

/// Per-turn token usage, when the provider's transcript exposes it. The budget
/// *ledger* that consumes this is S5; `SupervisorDriver` returns `None` in S4
/// (turn counting is the universal unit the linear engine needs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
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
    /// Timestamp (ms) of the most recent ingested session record — stall clock.
    fn last_activity(&self, agent_id: &str) -> Option<i64>;
    /// Per-turn usage if the provider exposes it (else `None`).
    fn turn_usage(&self, agent_id: &str) -> Option<TurnUsage>;
}

/// The production driver: a thin adapter over the agent [`Supervisor`].
pub struct SupervisorDriver {
    sup: Arc<Supervisor>,
    app: AppHandle,
}

impl SupervisorDriver {
    pub fn new(sup: Arc<Supervisor>, app: AppHandle) -> Self {
        Self { sup, app }
    }
}

impl AgentDriver for SupervisorDriver {
    fn spawn(&self, req: SpawnReq) -> BoxFuture<'_, Result<SpawnedAgent>> {
        Box::pin(async move {
            let SpawnReq {
                repo_path,
                provider,
                model,
                instructions,
                custom_agent_id,
                skills,
                mcp_servers,
                fork_base,
                run_repo,
                owner_run_id,
            } = req;

            let record = self
                .sup
                .clone()
                .spawn_agent(
                    self.app.clone(),
                    SpawnRequest {
                        // Step agents render in the structured (Custom) view.
                        view: AgentView::Custom,
                        repo_path,
                        provider,
                        name: None,
                        effort: None,
                        model,
                        instructions,
                        custom_agent_id,
                        skills,
                        mcp_servers,
                        fork_base,
                        run_repo,
                        owner_run_id: Some(owner_run_id),
                    },
                )
                .await?;

            // The primary checkout path — provisioned by the spawn's background
            // task; the caller waits for `Idle` before reading it.
            let worktree = match record.repos.first() {
                Some(primary) => crate::workspace::repo_checkout_path(&record.id, &primary.subdir)?,
                None => {
                    return Err(crate::error::Error::Other(
                        "spawned step agent has no tracked repo".into(),
                    ))
                }
            };
            Ok(SpawnedAgent {
                agent_id: record.id,
                worktree,
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
                .send_user_message(&self.app, agent_id, &turn_id, &text, &[], None)?;
            Ok(())
        })
    }

    fn stop<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.sup
                .clone()
                .stop_agent(self.app.clone(), agent_id)
                .await
        })
    }

    fn archive<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.sup
                .clone()
                .archive_agent(self.app.clone(), agent_id)
                .await
        })
    }

    fn last_activity(&self, agent_id: &str) -> Option<i64> {
        self.sup.last_activity(agent_id)
    }

    fn turn_usage(&self, _agent_id: &str) -> Option<TurnUsage> {
        // Token extraction is the budget ledger's job (S5). Turn counting — the
        // universal unit — needs nothing from here.
        None
    }
}

/// What a `MockDriver` does to the agent's status when a prompt is sent —
/// letting attempt tests model a turn deterministically instead of racing to
/// inject transitions at the right instant. Because the transitions fire
/// *inside* `send_message` (after the attempt has already subscribed), they
/// exercise the subscribe-before-send discipline for real: a driver that
/// subscribed *after* sending would miss them and hang.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum TurnBehavior {
    /// The prompt lands but the agent never wakes (turn-start-timeout tests).
    #[default]
    Silent,
    /// A full turn: `Running` then `Idle`, faster than any poll (flap / happy
    /// path). Writes the configured verdict as it completes.
    Complete,
    /// The agent starts its turn and never finishes (stall tests).
    RunningOnly,
    /// The agent starts, then errors mid-turn (error→retry tests).
    ErrorOut,
}

/// A scriptable [`AgentDriver`] for scheduler/attempt unit tests. Tests either
/// drive the status timeline explicitly ([`MockDriver::set_status`]) or set a
/// [`TurnBehavior`] so `send_message` plays a turn out deterministically.
#[cfg(test)]
pub(crate) struct MockDriver {
    state: parking_lot::Mutex<MockState>,
    status_tx: broadcast::Sender<StatusEvent>,
}

#[cfg(test)]
#[derive(Default)]
struct MockState {
    statuses: std::collections::HashMap<String, AgentStatus>,
    activity: std::collections::HashMap<String, i64>,
    usage: std::collections::HashMap<String, TurnUsage>,
    sent: Vec<(String, String)>,
    stopped: Vec<String>,
    archived: Vec<String>,
    spawn_count: usize,
    /// When set, `spawn` fails with this message (spawn-failure tests).
    fail_spawn: Option<String>,
    /// Worktree path handed back from `spawn`.
    worktree: PathBuf,
    /// `true` → `spawn` reports the agent `Idle` immediately, so the ready
    /// wait passes on its snapshot (turn/gate tests that don't exercise spawn).
    ready_on_spawn: bool,
    behavior: TurnBehavior,
    /// On a `Complete` turn, write this JSON to `<dir>/verdict.json`, modelling
    /// the agent writing its verdict during the turn (so it survives the
    /// pre-send stale-verdict archival — spec §8.3).
    complete_verdict: Option<(PathBuf, String)>,
}

#[cfg(test)]
impl MockDriver {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: parking_lot::Mutex::new(MockState {
                worktree: PathBuf::from("/tmp/mock-worktree"),
                ..Default::default()
            }),
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

    /// Set the agent's last-activity timestamp (the stall clock's input).
    pub(crate) fn set_activity(&self, agent_id: &str, ts_ms: i64) {
        self.state
            .lock()
            .activity
            .insert(agent_id.to_string(), ts_ms);
    }

    pub(crate) fn set_worktree(&self, path: PathBuf) {
        self.state.lock().worktree = path;
    }

    pub(crate) fn set_ready_on_spawn(&self, ready: bool) {
        self.state.lock().ready_on_spawn = ready;
    }

    pub(crate) fn set_turn_behavior(&self, behavior: TurnBehavior) {
        self.state.lock().behavior = behavior;
    }

    /// Configure the verdict a `Complete` turn writes, and where.
    pub(crate) fn set_complete_verdict(&self, dir: PathBuf, json: &str) {
        self.state.lock().complete_verdict = Some((dir, json.to_string()));
    }

    pub(crate) fn fail_next_spawn(&self, msg: &str) {
        self.state.lock().fail_spawn = Some(msg.to_string());
    }

    pub(crate) fn sent_messages(&self) -> Vec<(String, String)> {
        self.state.lock().sent.clone()
    }

    pub(crate) fn was_stopped(&self, agent_id: &str) -> bool {
        self.state.lock().stopped.iter().any(|a| a == agent_id)
    }

    pub(crate) fn was_archived(&self, agent_id: &str) -> bool {
        self.state.lock().archived.iter().any(|a| a == agent_id)
    }
}

#[cfg(test)]
impl AgentDriver for MockDriver {
    fn spawn(&self, _req: SpawnReq) -> BoxFuture<'_, Result<SpawnedAgent>> {
        Box::pin(async move {
            let (agent_id, worktree, ready) = {
                let mut st = self.state.lock();
                if let Some(msg) = st.fail_spawn.take() {
                    return Err(crate::error::Error::Other(msg));
                }
                st.spawn_count += 1;
                let agent_id = format!("mock-agent-{}", st.spawn_count);
                let worktree = st.worktree.clone();
                let ready = st.ready_on_spawn;
                let status = if ready {
                    AgentStatus::Idle
                } else {
                    AgentStatus::Spawning
                };
                st.statuses.insert(agent_id.clone(), status);
                (agent_id, worktree, ready)
            };
            if ready {
                // Broadcast so a subscriber already waiting on readiness sees it.
                let _ = self.status_tx.send(StatusEvent {
                    agent_id: agent_id.clone(),
                    status: AgentStatus::Idle,
                });
            }
            Ok(SpawnedAgent { agent_id, worktree })
        })
    }

    fn status(&self, agent_id: &str) -> Option<AgentStatus> {
        self.state.lock().statuses.get(agent_id).cloned()
    }

    fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.status_tx.subscribe()
    }

    fn send_message<'a>(&'a self, agent_id: &'a str, text: String) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let (behavior, verdict) = {
                let mut st = self.state.lock();
                st.sent.push((agent_id.to_string(), text));
                (st.behavior, st.complete_verdict.clone())
            };
            match behavior {
                TurnBehavior::Silent => {}
                TurnBehavior::RunningOnly => self.set_status(agent_id, AgentStatus::Running),
                TurnBehavior::ErrorOut => {
                    self.set_status(agent_id, AgentStatus::Running);
                    self.set_status(agent_id, AgentStatus::Error);
                }
                TurnBehavior::Complete => {
                    self.set_status(agent_id, AgentStatus::Running);
                    if let Some((dir, json)) = verdict {
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::fs::write(dir.join("verdict.json"), json);
                    }
                    self.set_status(agent_id, AgentStatus::Idle);
                }
            }
            Ok(())
        })
    }

    fn stop<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.state.lock().stopped.push(agent_id.to_string());
            Ok(())
        })
    }

    fn archive<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.state.lock().archived.push(agent_id.to_string());
            Ok(())
        })
    }

    fn last_activity(&self, agent_id: &str) -> Option<i64> {
        self.state.lock().activity.get(agent_id).copied()
    }

    fn turn_usage(&self, agent_id: &str) -> Option<TurnUsage> {
        self.state.lock().usage.get(agent_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_spawn_starts_spawning_and_broadcasts() {
        let d = MockDriver::new();
        let mut rx = d.subscribe();
        let spawned = d
            .spawn(SpawnReq {
                repo_path: PathBuf::from("/r"),
                provider: "claude".into(),
                model: None,
                instructions: None,
                custom_agent_id: None,
                skills: vec![],
                mcp_servers: vec![],
                fork_base: None,
                run_repo: None,
                owner_run_id: "run-1".into(),
            })
            .await
            .unwrap();
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
        let err = d
            .spawn(SpawnReq {
                repo_path: PathBuf::from("/r"),
                provider: "claude".into(),
                model: None,
                instructions: None,
                custom_agent_id: None,
                skills: vec![],
                mcp_servers: vec![],
                fork_base: None,
                run_repo: None,
                owner_run_id: "run-1".into(),
            })
            .await;
        assert!(err.is_err());
    }
}
