//! Agent process lifecycle: spawning, resuming, view switches, binary-swap
//! respawns, and the shared output/exit/watchdog handlers.

use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;

use crate::activity::{Activity, ClaudeNativeActivity, ManagedActivity};
use crate::agent::{capabilities, per_turn_descriptor, Agent, PerTurnSpec, SpawnSpec};
use crate::error::{Error, Result};
use crate::git;
use crate::rpc;
use crate::sandbox::provision::{self, CheckoutSpec, WorkspaceMode};
use crate::sandbox::{self, EngineKind};
use crate::workspace::{
    agent_parent_dir, allocate_repo_subdir, is_per_turn_provider, new_agent_record,
    repo_checkout_path, AgentRecord, AgentStatus, AgentView, TrackedRepo,
};

use super::events::{emit_agent_event, emit_agent_output, emit_repo_added, emit_view};
use super::messaging::{
    drain_message_queue, flush_queued, mark_user_turn_started, on_first_user_message,
};
use super::rpc_watch::spawn_rpc_watcher;
use super::{transition_active, Supervisor};

const WATCHDOG_TICK: Duration = Duration::from_millis(500);
const SPAWN_TIMEOUT: Duration = Duration::from_secs(15);

/// The engine stamped on an agent's record at creation. Records from before
/// engine selection existed (NULL) always ran under sandbox-exec, so that
/// stays their permanent kind — never re-derived from the live setting.
pub(super) fn stamped_engine(record: &AgentRecord) -> EngineKind {
    record
        .sandbox_engine
        .as_deref()
        .and_then(EngineKind::from_setting)
        .unwrap_or(EngineKind::SandboxExec)
}

/// The provisioning mode an agent stamped with `engine` actually gets.
///
/// Docker forces `Clone` regardless of the `workspace_mode` dev flag: a linked
/// worktree's `.git` file points into the user's real repo, and a container
/// must never be able to reach that (invariant 2).
///
/// Seatbelt also defaults to `Clone` — both engines converge on one
/// self-contained-checkout model, made cheap by `git clone --shared` (objects
/// borrowed via alternates, not copied). An explicit `workspace_mode=worktree`
/// still opts back into the linked-worktree model.
///
/// Restore re-provisions each repo at its archived tip. Provisioning refetches
/// from `origin` only when that tip isn't already reachable in the source
/// repo's object store (see `provision_on_branch`); when it is, restore borrows
/// it back via alternates and checks out offline, no remote branch needed (see
/// the `provision.rs` restore tests).
///
/// A clone-native agent writes new commits into its *own* `.git/objects`, and
/// archive teardown `rm -rf`s the clone — so commits that never reached
/// `origin` or the source store are gone, and restore can only recover what
/// did. This is the accepted cost of Clone mode, and the reason the
/// `workspace_mode=worktree` opt-out remains.
///
/// One further worktree-mode tradeoff: the commit / update-branch delegation
/// signal is driven by git hooks installed only into a clone's `.git/hooks`
/// (`provision::install_delegation_hooks` — a linked worktree's hooks live in
/// the user's real repo and must never be touched), so under the worktree
/// opt-out the panel's delegation attribution for native in-container commits
/// silently doesn't fire. Acceptable for a hidden dev flag.
pub(super) fn effective_workspace_mode(engine: EngineKind, setting: Option<&str>) -> WorkspaceMode {
    match engine {
        EngineKind::Docker => WorkspaceMode::Clone,
        // Explicit opt-out to the linked-worktree model; everything else —
        // unset or "clone" — resolves to Clone.
        EngineKind::SandboxExec => match setting {
            Some("worktree") => WorkspaceMode::Worktree,
            Some("clone") | None => WorkspaceMode::Clone,
            Some(other) => {
                tracing::warn!(value = %other, "unrecognized workspace_mode setting; using clone");
                WorkspaceMode::Clone
            }
        },
    }
}

/// Which providers run in Docker sandboxes: those with a wired-up container
/// image + config-mount + auth (see [`sandbox::docker::DockerProvider`]) — claude,
/// codex, opencode, pi, and cursor so far. antigravity stays gated: its CLI has no
/// non-interactive credential path (browser-OAuth only, tokens in the host
/// keychain, no API-key env fallback), so a container can't authenticate. Checked
/// before every spawn — fresh spawns and, via `spawn_agent_process`,
/// resume/view-switch — so a docker-stamped record can never launch an
/// unsupported provider. Consults the capability lookup rather than string-
/// matching a single provider id.
fn ensure_engine_supports_provider(engine: EngineKind, provider: &str) -> Result<()> {
    if engine == EngineKind::Docker && sandbox::docker::DockerProvider::from_id(provider).is_none()
    {
        let label = per_turn_descriptor(provider)
            .map(|d| d.label())
            .unwrap_or(provider);
        return Err(Error::Other(format!(
            "{label} isn't available in Docker sandboxes yet"
        )));
    }
    Ok(())
}

/// Resolved, per-spawn inputs for `spawn_agent_process` — everything that
/// isn't already carried on the `AgentRecord` (paths, session id, and this
/// spawn's generation number).
struct ProcessLaunch {
    cwd: PathBuf,
    sandbox_root: PathBuf,
    rpc_dir: PathBuf,
    session_id: Option<String>,
    per_turn: bool,
    effective_fresh: bool,
    my_gen: u64,
    /// The run blackboard dir to grant a workflow step agent (§8), derived from
    /// the record's `owner_run_id`. `None` for a normal, non-run-owned agent.
    blackboard: Option<PathBuf>,
}

/// Pick the turn-end detector for an agent by provider class and view, and
/// reset it when this spawn begins a fresh turn.
///
/// Per-turn agents carry their detector in the descriptor table — but only for
/// the Custom (exec/JSON) view. In the native view they run their interactive
/// TUI in a PTY with no JSON stream, so turn-end is detected by silence, the
/// same as claude's native view. Claude (no descriptor) picks by view too.
fn build_activity(record: &AgentRecord, effective_fresh: bool) -> Box<dyn Activity> {
    let mut activity: Box<dyn Activity> = match per_turn_descriptor(&record.provider) {
        Some(desc) => match record.view {
            AgentView::Native => Box::new(ClaudeNativeActivity::new()),
            AgentView::Custom => (desc.activity)(),
        },
        None => match record.view {
            AgentView::Native => Box::new(ClaudeNativeActivity::new()),
            AgentView::Custom => Box::new(ManagedActivity::claude()),
        },
    };
    if effective_fresh {
        activity.reset_for_new_turn();
    }
    activity
}

/// Everything a caller supplies to spawn a fresh agent. Bundled so the (single)
/// call site reads as named fields rather than nine positional arguments.
pub struct SpawnRequest {
    /// Requested view; downgraded to `Custom` for providers without a native view.
    pub view: AgentView,
    /// Primary repo the agent forks its checkout from.
    pub repo_path: PathBuf,
    /// Provider id (e.g. `"claude"`, `"codex"`).
    pub provider: String,
    /// Pre-allocated agent id from the draft; `None`/blank allocates a fresh one.
    pub name: Option<String>,
    /// Session-level effort (claude `--effort`), reapplied on every spawn.
    pub effort: Option<String>,
    /// Session-level model override; `None` keeps the provider CLI default.
    pub model: Option<String>,
    /// Custom agent's standing brief, re-injected on every spawn/resume.
    pub instructions: Option<String>,
    /// Prior-conversation digest for a forked session, composed after
    /// `instructions` on every spawn. `None` for a non-fork spawn. Kept separate
    /// from `instructions` so the user brief is never parsed/mutated.
    pub forked_context: Option<String>,
    /// Custom agent identity; `None` for a plain built-in spawn.
    pub custom_agent_id: Option<String>,
    /// Custom agent's skills, snapshotted by value (see `agent_profile`).
    pub skills: Vec<crate::agent_profile::SkillSnapshot>,
    /// Custom agent's MCP servers, snapshotted by value (see `agent_profile`).
    pub mcp_servers: Vec<crate::agent_profile::McpServerSnapshot>,
    /// Base the checkout forks from, which also becomes the agent's recorded
    /// parent branch (PR base / ahead-behind). The new-agent screen passes the
    /// chosen base branch here; a workflow step instead passes the previous
    /// step's HEAD (a commit-ish). `None` falls back to the repo's current
    /// branch.
    pub fork_base: Option<String>,
    /// For a workflow step: the run repository (`~/.fletch/runs/<id>/repo`) to
    /// fetch `fork_base` from before detaching (§12.1), since a previous step's
    /// commit lives only there. When `Some`, provisioning forks from the run
    /// repo instead of resolving the base against the source repo. `None` for a
    /// normal user spawn.
    pub run_repo: Option<PathBuf>,
    /// The workflow run that owns this agent, when spawned as a workflow step.
    /// Persisted on the record so run-owned agents are filterable from the
    /// normal sidebar and cleaned up by `wf_delete_run`. `None` for a normal
    /// user spawn.
    pub owner_run_id: Option<String>,
    /// Fork "carry code": another workspace's primary checkout whose current
    /// working tree (incl. uncommitted work) is overlaid onto this fresh
    /// checkout after provisioning, so the fork starts from that workspace's
    /// state. `None` for a normal spawn or a clean fork.
    pub carry_from: Option<PathBuf>,
}

impl Supervisor {
    pub async fn spawn_agent(
        self: Arc<Self>,
        app: AppHandle,
        req: SpawnRequest,
    ) -> Result<AgentRecord> {
        let _lifecycle_guard = self.agent_lifecycle.lock().await;
        let SpawnRequest {
            view,
            repo_path,
            provider,
            name,
            effort,
            model,
            instructions,
            forked_context,
            custom_agent_id,
            skills,
            mcp_servers,
            fork_base,
            run_repo,
            owner_run_id,
            carry_from,
        } = req;
        if !repo_path.join(".git").exists() {
            return Err(Error::InvalidPath(format!(
                "not a git repository: {}",
                repo_path.display()
            )));
        }
        // Provisioning forks a workspace (a `--shared` clone by default) from
        // this repo, which needs a resolvable HEAD. Adopting a folder normally
        // seeds one, but a repo added while it had no commits — or before that
        // guarantee existed — still has an unborn HEAD; seed it here so the fork
        // can't fail with a missing working directory downstream.
        git::ensure_head_commit(&repo_path).await?;

        // The engine this agent will be stamped with, resolved once so the
        // provider gate, the stamp, and the provisioning mode below all agree.
        let engine_kind = sandbox::selected_engine_kind();
        ensure_engine_supports_provider(engine_kind, &provider)?;

        // Only agents with a wired native (PTY/TUI) view can honor a Native
        // request; the rest fall back to the structured Custom view. Native
        // views are being rolled out per agent (see `AgentCapabilities`).
        let view = if capabilities(&provider).native_view {
            view
        } else {
            AgentView::Custom
        };

        // Use the name the draft already showed in the sidebar so it locks in
        // rather than being regenerated; only allocate a fresh one when the
        // caller didn't supply it (the draft-less spawn path).
        let agent_id = match name {
            Some(n) if !n.trim().is_empty() => n,
            _ => self.workspace.allocate_agent_id()?,
        };
        let name = agent_id.clone();

        // The agent's parent branch — the base its checkout forks from and the
        // ref it later targets for PRs / ahead-behind. The base the user chose
        // on the new-agent screen (`fork_base`) wins; absent a choice, fall back
        // to the branch the repo was on when the user hit Spawn.
        let parent_branch = match &fork_base {
            Some(base) if !base.trim().is_empty() => Some(base.clone()),
            _ => git::current_branch(&repo_path).await.ok().flatten(),
        };
        let subdir = allocate_repo_subdir(&repo_path, &[]);
        // Cloned for the background fork task — `parent_branch`/`subdir` are
        // moved into `primary` below.
        let parent_for_fork = parent_branch.clone();
        let subdir_for_fork = subdir.clone();
        // A workflow step forks from its `fork_base` ref in this run repo.
        let run_repo_for_task = run_repo.clone();
        // Fork "carry code": the source checkout whose working tree is overlaid
        // onto the fresh checkout once it's provisioned.
        let carry_from_task = carry_from.clone();

        let primary = TrackedRepo {
            repo_path: repo_path.clone(),
            subdir: subdir.clone(),
            branch: None, // materialized at first push, named by the agent
            parent_branch,
            base_sha: None,  // captured by the fork task once HEAD is known
            pr_number: None, // set when a PR is opened for this branch
            pr_url: None,
            pr_title: None,
            pr_state: None,
        };

        let mut record = new_agent_record(
            agent_id.clone(),
            name,
            provider,
            primary,
            String::new(),
            view,
        );
        // Session-level effort (claude `--effort`); persisted so start_process
        // re-applies it on every spawn. Per-turn agents ignore it at spawn.
        record.effort = effort;
        // Session-level model selection. `None` preserves the provider CLI
        // default; selected values are reapplied on resume and view switches.
        record.model = model;
        // Custom agent identity + snapshotted brief. Both `None` for a plain
        // built-in spawn. The brief is re-injected on every spawn/resume.
        record.instructions = instructions;
        // Forked-conversation digest, kept separate from the brief and composed
        // after it at launch (see start_process). `None` for a non-fork spawn.
        record.forked_context = forked_context;
        record.custom_agent_id = custom_agent_id;
        // Skill/MCP snapshots, persisted like the brief so every process spawn
        // (fresh, view-switch, resume) re-materializes the same profile.
        record.skills = skills;
        record.mcp_servers = mcp_servers;
        // Stamp the sandbox engine at creation: the agent keeps this engine
        // for life (respawn, view-switch, restore), so a later settings change
        // never re-engines it — see `spawn_agent_process`, which reuses the
        // stored value instead of the live setting.
        record.sandbox_engine = Some(engine_kind.as_setting().to_string());
        // Tag the agent with its owning run (workflow step spawn) so it's
        // hidden from the normal sidebar and cascaded on run delete.
        record.owner_run_id = owner_run_id;
        let parent_dir = agent_parent_dir(&agent_id)?;
        let primary_checkout = repo_checkout_path(&agent_id, &subdir)?;

        self.workspace.add_agent(&mut record)?;
        crate::telemetry::track(
            "agent_spawned",
            serde_json::json!({
                "provider": record.provider,
                "model": record.model,
                "effort": record.effort,
            }),
        );
        self.set_status(&app, &agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.clone());

        // Workspace provisioning mode — the `workspace_mode` dev flag under
        // seatbelt, forced to `Clone` under docker. Read once, outside the
        // spawn task, so the whole spawn uses one consistent mode.
        let workspace_mode = effective_workspace_mode(
            engine_kind,
            self.workspace
                .setting(provision::WORKSPACE_MODE_SETTING)
                .as_deref(),
        );

        let sup = self.clone();
        let app_for_task = app.clone();
        let id_for_task = agent_id.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = tokio::fs::create_dir_all(&parent_dir).await {
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
                return;
            }

            // Fork point: prefer the freshest remote state of the parent branch
            // (the user's chosen base, or the repo's current branch) so the agent
            // never starts on stale local refs — as the fetched tip's SHA, which
            // resolves the same in the source repo and in a clone-mode workspace
            // (a symbolic `origin/<branch>` would resolve against the source's
            // stale local head inside the clone). Best-effort — if the remote is
            // unavailable (offline, no remote, a local-only branch, or a workflow
            // commit-ish), resolve the ref to its local SHA, otherwise fall
            // through to HEAD so an unresolvable base degrades instead of failing
            // the spawn. The SHA (never the branch name) matters even in the
            // fallback: a clone-mode workspace only has the source's HEAD branch
            // as a local branch, so checking out any *other* branch by name
            // trips git's remote-DWIM (an implicit `-b`), which is fatal
            // combined with `--detach`.
            let provision_result = match &run_repo_for_task {
                // Workflow step (§12.1): fork from `fork_base` in the run repo.
                // `base_ref` is used as-is — step 1's `base_sha` is already in
                // the fresh source clone, so the run-repo fetch is skipped; step
                // N's `refs/wf/steps/<prev>` is fetched from the run repo first.
                Some(run_repo) => {
                    let base_ref = parent_for_fork.as_deref().unwrap_or("HEAD");
                    let spec = CheckoutSpec {
                        source_repo: &repo_path,
                        base_ref,
                        dest: &primary_checkout,
                    };
                    provision::provision_forking_run_repo(&spec, run_repo).await
                }
                None => {
                    let base = match &parent_for_fork {
                        Some(b) => match git::fetch_fork_point(&repo_path, b).await {
                            Some(sha) => Some(sha),
                            None => git::rev_parse(&repo_path, b).await.ok(),
                        },
                        None => None,
                    };
                    let spec = CheckoutSpec {
                        source_repo: &repo_path,
                        base_ref: base.as_deref().unwrap_or("HEAD"),
                        dest: &primary_checkout,
                    };
                    provision::provision(workspace_mode, &spec).await
                }
            };
            if let Err(e) = provision_result {
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
                return;
            }

            // Record the fork point so diffs measure against the exact starting
            // commit rather than a branch name that can drift. Non-fatal: a
            // missing base_sha just falls back to the parent branch name.
            let base_sha = git::rev_parse(&primary_checkout, "HEAD").await.ok();
            if let Some(sha) = &base_sha {
                let _ = sup
                    .workspace
                    .set_repo_base_sha(&id_for_task, &subdir_for_fork, sha);
            }

            // Fork "carry code": overlay the source workspace's current working
            // tree onto the fresh checkout, so the fork starts from that
            // workspace's uncommitted work. Fatal on failure — the user asked to
            // carry, so silently producing a clean fork would drop their changes.
            // Tears down like the start_process failure path below (a workflow
            // step never carries, so this is always a non-run clone).
            if let Some(src) = &carry_from_task {
                let carried = match &base_sha {
                    Some(base) => match git::snapshot_worktree(src).await {
                        Ok(snap) => git::carry_worktree(&primary_checkout, src, &snap, base).await,
                        Err(e) => Err(e),
                    },
                    None => Err(Error::Other(
                        "cannot carry working tree without a base commit".into(),
                    )),
                };
                if let Err(e) = carried {
                    let teardown_spec = CheckoutSpec {
                        source_repo: &repo_path,
                        base_ref: "HEAD",
                        dest: &primary_checkout,
                    };
                    let _ = provision::teardown(workspace_mode, &teardown_spec).await;
                    let _ = tokio::fs::remove_dir_all(&parent_dir).await;
                    fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
                    return;
                }
            }

            tokio::time::sleep(Duration::from_millis(350)).await;

            if let Err(e) = sup.start_process(&app_for_task, &id_for_task, true).await {
                // A workflow step was always provisioned as a clone (forking from
                // the run repo), so tear it down as one regardless of the mode
                // setting; `teardown` only needs the dest for the clone arm.
                let teardown_mode = if run_repo_for_task.is_some() {
                    provision::WorkspaceMode::Clone
                } else {
                    workspace_mode
                };
                let teardown_spec = CheckoutSpec {
                    source_repo: &repo_path,
                    base_ref: "HEAD",
                    dest: &primary_checkout,
                };
                let _ = provision::teardown(teardown_mode, &teardown_spec).await;
                let _ = tokio::fs::remove_dir_all(&parent_dir).await;
                fail_spawn(&sup, &app_for_task, &id_for_task, e.to_string());
            }
        });

        Ok(record)
    }

    /// Bring a second (or third…) repo into a live agent. Creates a
    /// detached checkout at `~/.fletch/workspaces/<agent-id>/<subdir>/`
    /// and appends a TrackedRepo entry. The checkout stays detached until
    /// its first push, consistent with the primary repo.
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
        // Same forkability guarantee as the primary repo: a commit-less repo has
        // an unborn HEAD that the workspace clone/worktree can't fork.
        git::ensure_head_commit(&repo_path).await?;
        let record = self.workspace.agent(agent_id)?;
        if record.repos.iter().any(|r| r.repo_path == repo_path) {
            return Err(Error::Other(
                "this repo is already tracked by the agent".into(),
            ));
        }
        let used: Vec<String> = record.repos.iter().map(|r| r.subdir.clone()).collect();
        let subdir = allocate_repo_subdir(&repo_path, &used);
        let checkout = repo_checkout_path(agent_id, &subdir)?;
        let parent_branch = git::current_branch(&repo_path).await.ok().flatten();

        // Fork from the freshest remote state of the parent branch (best-effort,
        // falls back to local HEAD), then record the fork point as the diff base.
        let base = match &parent_branch {
            Some(b) => git::fetch_fork_point(&repo_path, b).await,
            None => None,
        };
        // Same clone-forcing rule as the primary repo: a docker-stamped agent
        // mounts its parent dir, so every workspace under it must be
        // self-contained.
        let engine = stamped_engine(&record);
        let workspace_mode = effective_workspace_mode(
            engine,
            self.workspace
                .setting(provision::WORKSPACE_MODE_SETTING)
                .as_deref(),
        );
        let spec = CheckoutSpec {
            source_repo: &repo_path,
            base_ref: base.as_deref().unwrap_or("HEAD"),
            dest: &checkout,
        };
        // A repo added to a *live* agent can't get a new bind mount: the Docker
        // container's mounts are fixed at `docker run`, so a `--shared` clone's
        // borrowed object store would be unreachable in-container. Provision it
        // self-contained (full object copy, no alternates) so it needs no mount
        // and in-container git works immediately. Seatbelt has no container and
        // uses the normal (`--shared`) clone path. A later restore relaunches
        // the container and re-provisions via `--shared` + mount.
        if engine == EngineKind::Docker {
            provision::provision_self_contained(&spec).await?;
        } else {
            provision::provision(workspace_mode, &spec).await?;
        }
        let base_sha = git::rev_parse(&checkout, "HEAD").await.ok();

        let repo = TrackedRepo {
            repo_path: repo_path.clone(),
            subdir: subdir.clone(),
            branch: None,
            parent_branch,
            base_sha,
            pr_number: None,
            pr_url: None,
            pr_title: None,
            pr_state: None,
        };
        self.workspace.append_tracked_repo(agent_id, repo.clone())?;
        emit_repo_added(&app, agent_id, repo.clone());

        // No branch is created here — the new repo's checkout stays detached
        // until its first push, when the agent names its branch (same as the
        // primary repo).
        Ok(repo)
    }

    pub(super) async fn start_process(
        self: &Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        fresh: bool,
    ) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        let per_turn = is_per_turn_provider(&record.provider);
        // Claude carries a session id we generated at create time; per-turn
        // agents (codex, cursor) are assigned one by the CLI on their first
        // turn, so it may be None until then.
        let session_id = record.session_id.clone();
        if !per_turn && session_id.is_none() {
            return Err(Error::Other("agent record missing session_id".into()));
        }
        let primary = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no tracked repos".into()))?;
        let cwd = repo_checkout_path(agent_id, &primary.subdir)?;
        // Sandbox writable root — the agent's parent dir. Every agent (claude
        // and per-turn alike) now runs under sandbox-exec rooted here.
        let sandbox_root = agent_parent_dir(agent_id)?;

        // A workflow step agent additionally gets a write grant to its run's
        // blackboard (§8), derived from the record's `owner_run_id` so the run
        // directory stays the single source of truth. The scheduler provisions
        // the dir at launch; here we only compute the path to grant.
        let blackboard = match &record.owner_run_id {
            Some(run_id) => Some(crate::workflow::blackboard::blackboard_dir(
                &crate::workflow::blackboard::run_dir(run_id)?,
            )),
            None => None,
        };

        // The agent's file-mailbox RPC dir, created before spawn so the watcher
        // (and the agent's `FLETCH_RPC_DIR`) have a target from turn one.
        let rpc_dir = rpc::mailbox_dir(agent_id)?;
        rpc::ensure_mailbox(&rpc_dir)?;
        // Base branch for the git dispatcher — the branch the agent was
        // forked from, same default the manual PR action uses.
        let base_branch = primary
            .parent_branch
            .clone()
            .unwrap_or_else(|| "main".to_string());
        let git_dispatcher = rpc::git::GitDispatcher::new(cwd.clone(), base_branch);
        // A run-owned step agent also gets the workflow comms ops (wf_report /
        // wf_ask / wf_notify, §10); everything else still falls through to the
        // git dispatcher. Plain agents keep the git dispatcher unchanged.
        let rpc_dispatcher: Arc<dyn rpc::RpcDispatcher> = match &record.owner_run_id {
            Some(run_id) => Arc::new(crate::workflow::comms::WorkflowCommsDispatcher::new(
                app.clone(),
                run_id.clone(),
                agent_id.to_string(),
                git_dispatcher,
            )),
            None => Arc::new(git_dispatcher),
        };

        // Claude only writes a session file once the first turn lands.
        // If task is still empty (no first user message has ever been
        // sent) `--resume <uuid>` will 404. So we treat that case as
        // fresh — same UUID, no replay attempt — and the eventual
        // first message creates the session file. Once that's
        // happened, switch / resume can safely `--resume`.
        let no_messages_yet = record.task.trim().is_empty();
        let effective_fresh = fresh || no_messages_yet;

        let agent_id_str = agent_id.to_string();

        let my_gen = {
            let mut g = self.generations.lock();
            let entry = g.entry(agent_id_str.clone()).or_insert(0);
            *entry += 1;
            *entry
        };

        self.activities.lock().insert(
            agent_id_str.clone(),
            build_activity(&record, effective_fresh),
        );

        let agent = self.spawn_agent_process(
            app,
            &agent_id_str,
            &record,
            ProcessLaunch {
                cwd,
                sandbox_root,
                rpc_dir: rpc_dir.clone(),
                session_id,
                per_turn,
                effective_fresh,
                my_gen,
                blackboard,
            },
        )?;

        self.agents
            .lock()
            .insert(agent_id_str.clone(), Arc::new(agent));

        // Initial status is always Idle now — at process start there's
        // never an in-flight turn (we no longer pass a task as a spawn
        // arg). The user's first send flips it to Running. Promote out of
        // the live Spawning state atomically (a turn that already started
        // mustn't be clobbered). If the swap fails because the spawn already
        // timed out (status Error), the timeout fired before we inserted the
        // process above — so its shutdown was a no-op and we'd leak a live
        // process shown as failed. Tear down what we just started instead.
        let promoted = self.claim_spawn_outcome(app, &agent_id_str, AgentStatus::Idle, None);
        if !promoted && matches!(self.live_status(&agent_id_str), Some(AgentStatus::Error)) {
            self.bump_generation(&agent_id_str);
            let taken = self.agents.lock().remove(&agent_id_str);
            if let Some(agent) = taken {
                let _ = agent.shutdown();
            }
            self.activities.lock().remove(&agent_id_str);
            return Err(Error::Other(
                "spawn aborted: timed out before the process became ready".into(),
            ));
        }

        // A message sent before the process finished coming up was enqueued —
        // a Spawning agent counts as busy, so `send_user_message` routes the
        // first send as a follow-up (Enqueue for per-turn, a retried
        // AgentNotFound for claude). Turn-end Idle drains the queue via
        // `transition_active`, but this spawn-completion Idle doesn't go through
        // that path, so drain here too. Without it a queued first message sits
        // undelivered until the *next* message flushes it (the user sees their
        // bubble + a spinner that clears with no reply). No-op when the queue is
        // empty — the common case — and `drain_coalesced` makes it safe against a
        // concurrent FlushNow from a racing second send.
        if promoted {
            drain_message_queue(self, app, &agent_id_str);
        }

        spawn_turn_watchdog(self.clone(), app.clone(), agent_id_str.clone(), my_gen);

        // Register the dispatcher so the mailbox can also be drained on demand
        // (`settle_agent_rpc`), not only on the watcher's tick. Overwrites any
        // prior generation's entry.
        self.rpc_dispatchers
            .lock()
            .insert(agent_id_str.clone(), rpc_dispatcher.clone());

        // Watch this agent's RPC mailbox for the life of this generation,
        // executing allowlisted ops and writing responses back.
        spawn_rpc_watcher(
            self.clone(),
            app.clone(),
            agent_id_str,
            rpc_dispatcher,
            rpc_dir,
            my_gen,
        );

        Ok(())
    }

    /// Spawn the agent's child process, dispatching on provider class
    /// (per-turn vs. claude) and view (Native PTY vs. Custom managed/exec).
    /// Returns the live `Agent` handle for the supervisor to track.
    fn spawn_agent_process(
        self: &Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        record: &AgentRecord,
        launch: ProcessLaunch,
    ) -> Result<Agent> {
        let ProcessLaunch {
            cwd,
            sandbox_root,
            rpc_dir,
            session_id,
            per_turn,
            effective_fresh,
            my_gen,
            blackboard,
        } = launch;
        let agent_id_str = agent_id.to_string();

        let engine = stamped_engine(record);
        // Re-checked on every launch path (resume, view switch, binary-swap
        // respawn), not just fresh spawns: only claude runs under docker.
        ensure_engine_supports_provider(engine, &record.provider)?;

        // Materialize the session's skill snapshot under the writable root and
        // fold its index into the injected instructions. Recomputed on every
        // launch path so the files always exist (e.g. after a checkout is
        // recreated) and always match the snapshot. `None` when the session has
        // neither a brief nor skills — the pre-profile behavior.
        let instructions = crate::agent_profile::effective_instructions(
            record.instructions.as_deref(),
            record.forked_context.as_deref(),
            &record.skills,
            &sandbox_root,
        )?;
        // Claude's generated MCP config, regenerated from the snapshot each
        // launch and passed via `--mcp-config` + `--strict-mcp-config`. Other
        // providers take their MCP delivery from the spec's snapshot instead
        // (codex `-c` overrides); see `agent_profile`.
        let mcp_config = if record.provider == "claude" {
            crate::agent_profile::write_claude_mcp_config(&record.mcp_servers, &sandbox_root)?
        } else {
            None
        };

        if per_turn {
            match record.view {
                // Native view: launch the agent's interactive TUI in a PTY,
                // resuming the session the Custom view established. The
                // switch_view guard guarantees a session id is present before
                // we ever route a per-turn agent here.
                AgentView::Native => {
                    let session_id = session_id.as_deref().ok_or_else(|| {
                        Error::Other("native view requires an established session id".into())
                    })?;
                    let spec = SpawnSpec {
                        agent_id: &agent_id_str,
                        cwd,
                        sandbox_root,
                        session_id,
                        // Per-turn native always resumes (the agent built its
                        // session in the Custom view first).
                        fresh: false,
                        // Per-turn agents take effort per-turn (build-args),
                        // not at spawn.
                        effort: None,
                        model: record.model.as_deref(),
                        instructions: instructions.as_deref(),
                        mcp_servers: &record.mcp_servers,
                        mcp_config: None,
                        rpc_dir,
                        cols: 120,
                        rows: 32,
                        engine,
                        blackboard: blackboard.as_deref(),
                    };
                    spawn_pty_per_turn_agent(
                        spec,
                        record.provider.clone(),
                        app.clone(),
                        agent_id_str.clone(),
                        self.clone(),
                        my_gen,
                    )
                }
                // Custom view: per-turn runner — no process spawns until the
                // first user message. No sandbox profile: the agent sandboxes
                // itself rather than running under sandbox-exec.
                AgentView::Custom => spawn_per_turn_agent(
                    &record.provider,
                    PerTurnSpec {
                        agent_id: agent_id_str.clone(),
                        cwd,
                        sandbox_root,
                        session_id,
                        model: record.model.clone(),
                        instructions: instructions.clone(),
                        mcp_servers: record.mcp_servers.clone(),
                        rpc_dir,
                        engine,
                        blackboard: blackboard.clone(),
                    },
                    app.clone(),
                    agent_id_str.clone(),
                    self.clone(),
                    my_gen,
                ),
            }
        } else {
            let session_id = session_id
                .as_deref()
                .expect("non-codex agents always have a session id");
            let spec = SpawnSpec {
                agent_id: &agent_id_str,
                cwd,
                sandbox_root,
                session_id,
                fresh: effective_fresh,
                // Claude's session-level effort, persisted on the record so it
                // re-applies on every spawn (fresh, view-switch, resume).
                effort: record.effort.as_deref(),
                model: record.model.as_deref(),
                instructions: instructions.as_deref(),
                mcp_servers: &record.mcp_servers,
                mcp_config: mcp_config.as_deref(),
                rpc_dir,
                cols: 120,
                rows: 32,
                engine,
                blackboard: blackboard.as_deref(),
            };
            match record.view {
                AgentView::Native => spawn_pty_agent(
                    spec,
                    app.clone(),
                    agent_id_str.clone(),
                    self.clone(),
                    my_gen,
                ),
                AgentView::Custom => spawn_managed_agent(
                    spec,
                    app.clone(),
                    agent_id_str.clone(),
                    self.clone(),
                    my_gen,
                ),
            }
        }
    }

    pub async fn resume_agent(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        let _lifecycle_guard = self.agent_lifecycle.lock().await;
        let record = self.workspace.agent(agent_id)?;
        if self.agents.lock().contains_key(agent_id) {
            return Ok(());
        }
        // Per-turn agents are assigned a session id on their first turn, so
        // a missing one is only an error for providers that generate it up
        // front.
        if !is_per_turn_provider(&record.provider) && record.session_id.is_none() {
            return Err(Error::Other(
                "Agent has no session id; remove and respawn.".into(),
            ));
        }
        self.set_status(&app, agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.to_string());

        self.start_process(&app, agent_id, false).await?;
        Ok(())
    }

    pub fn write_to_agent(
        self: Arc<Self>,
        app: &AppHandle,
        agent_id: &str,
        bytes: &[u8],
    ) -> Result<()> {
        let project_id = self.workspace.agent(agent_id)?.project_id;
        let deletion_guard = self.deleting_projects.lock();
        if deletion_guard.contains(&project_id) {
            return Err(Error::Other("project deletion is in progress".into()));
        }
        self.live_agent(agent_id)?.write_pty(bytes)?;
        let submitted = self
            .native_inputs
            .lock()
            .entry(agent_id.to_string())
            .or_default()
            .observe(bytes);
        for submitted in submitted {
            mark_user_turn_started(&self, app, agent_id, None);
            on_first_user_message(self.clone(), app.clone(), agent_id.to_string(), submitted);
        }
        drop(deletion_guard);
        Ok(())
    }

    pub fn resize_agent(&self, agent_id: &str, cols: u16, rows: u16) -> Result<()> {
        self.live_agent(agent_id)?.resize(cols, rows)
    }

    pub async fn switch_view(
        self: Arc<Self>,
        app: AppHandle,
        agent_id: &str,
        new_view: AgentView,
    ) -> Result<()> {
        let _lifecycle_guard = self.agent_lifecycle.lock().await;
        let record = self.workspace.agent(agent_id)?;
        if record.view == new_view {
            return Ok(());
        }
        // Reject switching to native for agents whose native view isn't
        // wired yet (rolling out per agent — see `AgentCapabilities`).
        if new_view == AgentView::Native && !capabilities(&record.provider).native_view {
            return Err(Error::Other(
                "The native view isn't available for this agent yet".into(),
            ));
        }

        // Per-turn agents assign their own session id on the first turn, and
        // the native TUI gives us no event stream to capture it. So we only
        // allow switching to native once that id exists — the TUI then
        // resumes the same session, and switching back to Custom can resume
        // it too. (claude generates its id up front, so this never blocks it.)
        if new_view == AgentView::Native
            && is_per_turn_provider(&record.provider)
            && record.session_id.is_none()
        {
            return Err(Error::Other(
                "Switch to the native view after the agent's first turn".into(),
            ));
        }

        let taken = self.agents.lock().remove(agent_id);
        if let Some(agent) = taken {
            let _ = agent.shutdown();
        }
        self.activities.lock().remove(agent_id);
        self.native_inputs.lock().remove(agent_id);

        self.workspace.update_agent_view(agent_id, new_view)?;
        emit_view(&app, agent_id, new_view);
        self.set_status(&app, agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.to_string());

        tokio::time::sleep(Duration::from_millis(150)).await;

        if let Err(e) = self.start_process(&app, agent_id, false).await {
            let err = e.to_string();
            self.set_status(&app, agent_id, AgentStatus::Error, Some(err));
            return Err(e);
        }
        Ok(())
    }

    /// Respawn every live agent using `provider_id` so it picks up a freshly
    /// changed binary path. The binary is resolved only inside `start_process`
    /// (spawn / resume / view-switch), so a live agent keeps the old binary —
    /// baked into its running process (claude, persistent) or frozen spawn args
    /// (per-turn) — until torn down and restarted. Callers must refresh the
    /// `bin_resolve` override registry *before* calling this so the restarted
    /// processes resolve the new path.
    ///
    /// Only currently-live agents need this; anything not in the `agents` map
    /// will resolve the new binary on its next spawn anyway.
    pub async fn respawn_provider(self: &Arc<Self>, app: &AppHandle, provider_id: &str) {
        // Snapshot ids under a short-lived lock; never hold a guard across the
        // `start_process` await in `respawn_agent_for_bin` (parking_lot guards
        // aren't Send, and `start_process` re-locks these maps → deadlock).
        let ids: Vec<String> = self.agents.lock().keys().cloned().collect();
        for id in ids {
            match self.workspace.agent(&id) {
                Ok(r) if r.provider == provider_id => {}
                _ => continue, // wrong provider, or removed out from under us
            }
            self.respawn_agent_for_bin(app, &id).await;
        }
    }

    /// Tear down and restart one live agent so it execs the freshly resolved
    /// binary, resuming its existing session (`fresh = false`) so the
    /// transcript/conversation is preserved.
    ///
    /// The idle-check and the `agents` removal happen atomically under a single
    /// `agents` lock: a concurrent send can flip an agent Idle→Running on
    /// another thread (`transition_active` touches only `statuses`), so a
    /// separate check-then-remove would risk shutting down an in-flight turn.
    /// If the agent is mid-turn (Spawning/Running) we leave it running and flag
    /// it in `respawn_pending`; the next turn-end Idle transition retries it
    /// (see `transition_active`). This is what keeps the "swap binary → keep
    /// going" flow working for an agent that's busy at swap time.
    pub(super) async fn respawn_agent_for_bin(self: &Arc<Self>, app: &AppHandle, agent_id: &str) {
        let record = match self.workspace.agent(agent_id) {
            Ok(r) => r,
            Err(_) => {
                self.respawn_pending.lock().remove(agent_id);
                return;
            }
        };
        // Atomic idle-check + remove. `busy` distinguishes "left running" from
        // "already gone" when no agent is taken.
        let mut busy = false;
        let taken = {
            let mut agents = self.agents.lock();
            if !agents.contains_key(agent_id) {
                None // gone — next spawn resolves the new binary anyway
            } else if matches!(
                self.effective_status(agent_id, &record),
                AgentStatus::Spawning | AgentStatus::Running
            ) {
                busy = true;
                None
            } else {
                agents.remove(agent_id)
            }
        };
        let agent = match taken {
            Some(agent) => agent,
            None if busy => {
                self.respawn_pending.lock().insert(agent_id.to_string());
                tracing::info!(agent_id, "binary-swap respawn deferred: agent busy");
                return;
            }
            None => {
                self.respawn_pending.lock().remove(agent_id);
                return;
            }
        };
        self.respawn_pending.lock().remove(agent_id);
        let _ = agent.shutdown();
        self.activities.lock().remove(agent_id);
        self.native_inputs.lock().remove(agent_id);

        self.set_status(app, agent_id, AgentStatus::Spawning, None);
        arm_spawn_timeout(self.clone(), app.clone(), agent_id.to_string());

        // Let the old process fully release its session before resuming it
        // (mirrors `switch_view`).
        tokio::time::sleep(Duration::from_millis(150)).await;

        if let Err(e) = self.start_process(app, agent_id, false).await {
            let err = e.to_string();
            tracing::warn!(agent_id, error = %err, "binary-swap respawn failed");
            self.set_status(app, agent_id, AgentStatus::Error, Some(err));
            return;
        }

        // This respawn passed through a turn-end Idle where the normal queue
        // drain deferred to us (see `drain_message_queue`). Now that the agent
        // is back, deliver any follow-ups queued during that turn — unless the
        // user stopped (A2-A), which we own the interrupt check for here.
        if !self.interrupted.lock().remove(agent_id) {
            if let Err(e) = flush_queued(self, app, agent_id) {
                tracing::warn!(agent_id, error = %e, "post-respawn queue flush failed");
            }
        }
    }

    pub async fn stop_agent(self: Arc<Self>, app: AppHandle, agent_id: &str) -> Result<()> {
        // Interrupt the current turn. How it returns to Idle depends on
        // the runner: claude (managed) emits a `result` event and, if it
        // exits, `apply_exit_if_current` moves it to Idle; codex's
        // per-turn `exec` exits on SIGINT and its `on_turn_exit` handler
        // ends the turn (it emits no `turn.completed` when interrupted).
        let _ = app;
        // Mark the stop so the turn-end Idle transition keeps the queue intact
        // instead of auto-flushing it (A2-A). Cleared when a new turn starts.
        self.interrupted.lock().insert(agent_id.to_string());
        // Clone the handle out and interrupt with the `agents` lock released
        // (interrupt writes deny responses over stdin, which can block). An
        // agent not in the map is a silent no-op, as before.
        if let Ok(agent) = self.live_agent(agent_id) {
            agent.interrupt();
        }
        Ok(())
    }
}

/// Process-exit outcomes that feed `apply_exit_if_current`. `PtyExit` and
/// `ManagedExit` are distinct types but carry the same fields, so this trait
/// lets `make_exit_handler` cover both spawners with one closure.
trait ExitOutcome {
    fn into_parts(self) -> (bool, String);
}

impl ExitOutcome for crate::pty_session::PtyExit {
    fn into_parts(self) -> (bool, String) {
        (self.success, self.message)
    }
}

impl ExitOutcome for crate::managed_session::ManagedExit {
    fn into_parts(self) -> (bool, String) {
        (self.success, self.message)
    }
}

/// Raw-byte output callback shared by the PTY spawners: record activity, then
/// emit `agent:output`.
fn make_output_handler(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
) -> impl Fn(Vec<u8>) + Send + Sync + 'static {
    move |bytes: Vec<u8>| {
        if let Some(activity) = sup.activities.lock().get_mut(&agent_id) {
            activity.observe_bytes(&bytes);
        }

        emit_agent_output(&app, &agent_id, bytes);
    }
}

/// Parsed-JSON event callback shared by the managed + per-turn spawners:
/// record activity, then emit `agent:event`.
fn make_event_handler(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
) -> impl Fn(Value) + Send + Sync + 'static {
    move |event: Value| {
        if let Some(activity) = sup.activities.lock().get_mut(&agent_id) {
            activity.observe_event(&event);
        }

        emit_agent_event(&app, &agent_id, event);
    }
}

/// Process-exit callback shared by the pty/managed spawners: hand the outcome
/// to `apply_exit_if_current`, which ignores exits from a stale generation.
fn make_exit_handler<E: ExitOutcome>(
    sup: Arc<Supervisor>,
    app: AppHandle,
    agent_id: String,
    gen: u64,
) -> impl Fn(E) + Send + Sync + 'static {
    move |exit: E| {
        let (success, message) = exit.into_parts();
        apply_exit_if_current(&sup, &app, &agent_id, gen, success, message);
    }
}

fn spawn_pty_agent(
    spec: SpawnSpec<'_>,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    Agent::spawn_pty(
        spec,
        make_output_handler(sup.clone(), app.clone(), agent_id.clone()),
        make_exit_handler(sup, app, agent_id, gen),
    )
}

/// Native (PTY/TUI) view for a per-turn agent. Same byte/exit wiring as
/// `spawn_pty_agent` (claude), but launches the agent's own binary via
/// `Agent::spawn_pty_native` rather than running claude under sandbox-exec.
fn spawn_pty_per_turn_agent(
    spec: SpawnSpec<'_>,
    provider: String,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    Agent::spawn_pty_native(
        spec,
        &provider,
        make_output_handler(sup.clone(), app.clone(), agent_id.clone()),
        make_exit_handler(sup, app, agent_id, gen),
    )
}

fn spawn_managed_agent(
    spec: SpawnSpec<'_>,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    Agent::spawn_managed(
        spec,
        make_event_handler(sup.clone(), app.clone(), agent_id.clone()),
        make_exit_handler(sup, app, agent_id, gen),
    )
}

/// Build a per-turn agent (codex, cursor). Their process exits at the end
/// of *every* turn — that's normal, not the agent dying — so unlike the
/// pty/managed spawners we don't wire `apply_exit_if_current` (which would
/// remove the agent from the map). Instead the per-turn exit is reported
/// via `on_turn_exit`, which ends the turn (Idle) without tearing the
/// agent down. This covers turns that exit without an in-band turn-end
/// event (interrupt, crash) so the agent doesn't sit Running until the
/// silence backstop. The session-id callback persists the id the agent
/// assigns on its first turn so later turns (and re-attach after restart)
/// resume it.
fn spawn_per_turn_agent(
    provider: &str,
    spec: PerTurnSpec,
    app: AppHandle,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    let id_for_sid = agent_id.clone();
    let sup_for_sid = sup.clone();
    let app_for_exit = app.clone();
    let id_for_exit = agent_id.clone();
    let sup_for_exit = sup.clone();

    let on_event = make_event_handler(sup, app, agent_id);
    let on_session_id = move |sid: String| {
        if let Err(e) = sup_for_sid
            .workspace
            .set_agent_session_id(&id_for_sid, &sid)
        {
            tracing::warn!(error = %e, agent_id = %id_for_sid, "persist session id failed");
        }
    };
    let on_turn_exit = move |exit: crate::exec_session::ExecExit| {
        // The turn's process exited. Ignore if a respawn/teardown has since
        // bumped the generation (e.g. the session was dropped).
        let current = sup_for_exit
            .generations
            .lock()
            .get(&id_for_exit)
            .copied()
            .unwrap_or(0);
        if current != gen {
            return;
        }
        // End the turn. Idempotent with the in-band turn-end watchdog path.
        // User Stop is an expected non-zero exit; a non-interrupted failure
        // before the CLI emits JSON is a real crash and must be surfaced.
        if exit.success || exit.interrupted {
            transition_active(
                &sup_for_exit,
                &app_for_exit,
                &id_for_exit,
                AgentStatus::Idle,
            );
        } else {
            // Bind the removed handle so the `agents` guard drops before the
            // (last-`Arc`) session teardown, keeping child kill/reap off the
            // global lock.
            let taken = sup_for_exit.agents.lock().remove(&id_for_exit);
            drop(taken);
            sup_for_exit.activities.lock().remove(&id_for_exit);
            sup_for_exit.native_inputs.lock().remove(&id_for_exit);
            sup_for_exit.trigger_session_sync(app_for_exit.clone(), id_for_exit.clone());
            sup_for_exit.set_status(
                &app_for_exit,
                &id_for_exit,
                AgentStatus::Error,
                Some(format!("Agent process exited: {}", exit.message)),
            );
        }
    };

    let desc = per_turn_descriptor(provider)
        .ok_or_else(|| Error::Other(format!("unknown per-turn agent provider: {provider}")))?;
    Agent::spawn_per_turn(desc, spec, on_event, on_session_id, on_turn_exit)
}

fn spawn_turn_watchdog(sup: Arc<Supervisor>, app: AppHandle, agent_id: String, gen: u64) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(WATCHDOG_TICK).await;

            let current_gen = sup.generations.lock().get(&agent_id).copied().unwrap_or(0);
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

pub(super) fn arm_spawn_timeout(sup: Arc<Supervisor>, app: AppHandle, agent_id: String) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SPAWN_TIMEOUT).await;
        // Atomically claim the timeout outcome. Only an agent still in the
        // live Spawning state may be timed out; if the swap fails the spawn
        // already left Spawning (completed, or failed on its own) and must
        // not be killed. The compare-and-swap also closes the race with
        // start_process: if the spawn task inserts its process concurrently,
        // exactly one of us flips the status, and the loser tears down.
        let err = "Spawn timed out after 15s — process did not become ready.".to_string();
        if !sup.claim_spawn_outcome(&app, &agent_id, AgentStatus::Error, Some(err)) {
            return;
        }
        // Invalidate any gen-guarded loop / exit handler from this spawn before
        // killing the process (same reason as `detach_runtime`).
        sup.bump_generation(&agent_id);
        let taken = sup.agents.lock().remove(&agent_id);
        if let Some(agent) = taken {
            let _ = agent.shutdown();
        }
        sup.activities.lock().remove(&agent_id);
    });
}

pub(super) fn fail_spawn(sup: &Supervisor, app: &AppHandle, agent_id: &str, err: String) {
    sup.set_status(app, agent_id, AgentStatus::Error, Some(err));
}

fn apply_exit_if_current(
    sup: &Supervisor,
    app: &AppHandle,
    agent_id: &str,
    gen: u64,
    success: bool,
    message: String,
) {
    let current = sup.generations.lock().get(agent_id).copied().unwrap_or(0);
    if current != gen {
        tracing::debug!(
            agent_id = %agent_id,
            stale_gen = gen,
            current_gen = current,
            "ignoring exit from prior generation"
        );
        return;
    }

    // The process is gone and we don't restart here (a clean exit stays Idle/
    // resumable). Invalidate this generation so the gen-guarded turn watchdog
    // and RPC watcher stop polling instead of spinning for the app's lifetime.
    sup.bump_generation(agent_id);

    // Bind the removed handle so the `agents` guard drops before the
    // (last-`Arc`) session teardown runs its child kill/reap off the lock.
    let taken = sup.agents.lock().remove(agent_id);
    drop(taken);
    sup.activities.lock().remove(agent_id);
    sup.native_inputs.lock().remove(agent_id);

    let (status, err) = if success {
        // Clean exit means the agent is resumable — keep it Idle so the
        // user can send follow-up messages without a manual Resume step.
        // The Idle entry stays in the `statuses` map (the agent is
        // resumable for the life of the session).
        (AgentStatus::Idle, None)
    } else {
        (
            AgentStatus::Error,
            Some(format!("Agent process exited: {message}")),
        )
    };
    // Only apply the exit transition if the agent was still live (not
    // already moved to a terminal disposition by another path).
    let was_live = matches!(
        sup.live_status(agent_id).unwrap_or(AgentStatus::Spawning),
        AgentStatus::Running | AgentStatus::Idle | AgentStatus::Spawning
    );
    if was_live {
        sup.set_status(app, agent_id, status.clone(), err);
        if matches!(status, AgentStatus::Idle) {
            sup.fetch_and_emit_pr_state(app.clone(), agent_id.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invariant 2: a docker agent's workspace is always a self-contained
    /// clone, whatever the `workspace_mode` dev flag says.
    #[test]
    fn docker_forces_clone_workspaces() {
        for setting in [None, Some("worktree"), Some("clone"), Some("bogus")] {
            assert_eq!(
                effective_workspace_mode(EngineKind::Docker, setting),
                WorkspaceMode::Clone,
                "docker must force clone for setting {setting:?}",
            );
        }
    }

    /// Seatbelt now defaults to `Clone` (cheap via `--shared`); an explicit
    /// `workspace_mode=worktree` is the only way back to the historical linked
    /// worktree, which trades away offline restore of never-pushed branches.
    #[test]
    fn seatbelt_defaults_to_clone_with_worktree_opt_out() {
        for setting in [None, Some("clone"), Some("bogus")] {
            assert_eq!(
                effective_workspace_mode(EngineKind::SandboxExec, setting),
                WorkspaceMode::Clone,
                "seatbelt must default to clone for setting {setting:?}",
            );
        }
        assert_eq!(
            effective_workspace_mode(EngineKind::SandboxExec, Some("worktree")),
            WorkspaceMode::Worktree,
        );
    }

    #[test]
    fn docker_supports_wired_providers_but_refuses_the_rest() {
        // All wired-up providers launch under docker.
        for provider in ["claude", "codex", "opencode", "pi", "cursor"] {
            assert!(
                ensure_engine_supports_provider(EngineKind::Docker, provider).is_ok(),
                "{provider} should be docker-supported",
            );
        }
        // antigravity is the last still-gated provider — no container auth path.
        let err = ensure_engine_supports_provider(EngineKind::Docker, "antigravity")
            .expect_err("antigravity must refuse under docker");
        assert!(
            err.to_string()
                .ends_with("isn't available in Docker sandboxes yet"),
            "unexpected refusal copy: {err}",
        );
        // The copy uses the human-facing product name, not the provider id.
        assert!(err.to_string().starts_with("Antigravity "), "{err}");

        // Seatbelt is unaffected.
        for provider in ["claude", "codex", "cursor", "antigravity"] {
            assert!(ensure_engine_supports_provider(EngineKind::SandboxExec, provider).is_ok());
        }
    }
}
