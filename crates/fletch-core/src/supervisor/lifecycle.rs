//! Agent process lifecycle: spawning, resuming, binary-swap respawns, and the
//! shared event/exit handlers.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::activity::{Activity, ManagedActivity};
use crate::agent::{
    accounts, claude_session_has_messages, per_turn_descriptor, Agent, PerTurnSpec, SessionStart,
    SpawnSpec,
};
use crate::error::{Error, Result};
use crate::git;
use crate::host::EngineCtx;
use crate::rpc;
use crate::sandbox;
use crate::sandbox::provision::{self, CheckoutSpec};
use crate::workspace::{
    agent_parent_dir, allocate_repo_subdir,
    folder_grants::{FolderGranted, FolderGrantsUpdate},
    is_per_turn_provider, new_agent_record, repo_checkout_path, AgentRecord, AgentStatus,
    AgentView, FolderGrant, TrackedRepo,
};

use super::events::{
    emit_agent_event, emit_effort, emit_folder_grants, emit_model, emit_repo_added,
    emit_spawn_progress, emit_workspace_changed, SpawnStage,
};
use super::messaging::{drain_message_queue, flush_queued, mark_user_turn_started};
use super::rpc_watch::spawn_rpc_watcher;
use super::{transition_active, Launch, Supervisor};

const SPAWN_TIMEOUT: Duration = Duration::from_secs(15);

/// The account a new agent of `provider` is stamped with: the active-account
/// setting, when it names a managed account that still exists (see
/// `accounts::account_for_new_agent`); else the default (`None`).
fn active_account(ctx: &EngineCtx, provider: &str) -> Option<String> {
    if !accounts::supports_accounts(provider) {
        return None;
    }
    let setting =
        crate::database::get_setting(&ctx.db.lock(), &accounts::active_setting_key(provider));
    accounts::account_for_new_agent(provider, setting.as_deref())
}

/// Wire codegraph indexing into a freshly-provisioned checkout — best-effort and
/// never fatal to a spawn. Skipped entirely when indexing is off.
///
/// The cheap, synchronous part runs inline before the agent starts: register
/// the `.git/info/exclude` entry, and — if the repo's mirror already holds a
/// built index — copy `codegraph.db` into `<checkout>/.codegraph/` so the MCP
/// server has an index from turn one. The expensive part (install + clone +
/// index/advance the mirror to this checkout's base commit) is detached to a
/// background task. When no index existed to copy synchronously, that task
/// copies the freshly-built db into the checkout afterward if it still exists —
/// codegraph's MCP server picks up a late-appearing index on a later tool call.
pub(super) async fn provision_codegraph_index(
    project_id: String,
    source_repo: PathBuf,
    checkout: PathBuf,
    base_sha: Option<String>,
    provider: &str,
    session_servers: &[crate::agent_profile::McpServerSnapshot],
) {
    use crate::codegraph;

    // Don't build an index this session will never be able to query. Shares
    // `codegraph::session_wants_codegraph` with the injection gate rather than
    // re-stating its conditions, so the two can't drift: a provider with no MCP
    // delivery, indexing switched off, or a session that
    // already defines its own `"codegraph"` server all mean the built-in server
    // is never injected — and cloning the mirror, indexing it, and copying the
    // db in would be minutes of background git and parse work per spawn for a
    // tool the agent doesn't have.
    //
    // The one condition deliberately *not* shared is `binary_installed`: this is
    // the path that installs it (see `ensure_installed` below), so requiring it
    // up front would mean the first run never bootstraps.
    if !codegraph::session_wants_codegraph(provider, session_servers) {
        return;
    }
    let mirror = match codegraph::mirror_dir(&project_id, &source_repo) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, repo = %source_repo.display(), "codegraph mirror path failed");
            return;
        }
    };

    // Always register the exclude entry when enabled, even if no index exists
    // yet — so the index dir never surfaces in diffs once it lands.
    if let Err(e) = codegraph::append_git_exclude(&checkout).await {
        tracing::warn!(error = %e, "codegraph exclude entry failed");
    }
    // Synchronously copy an already-built index in, if the mirror has one.
    let db = mirror.join(".codegraph").join("codegraph.db");
    let copied_sync = db.exists();
    if copied_sync {
        if let Err(e) = codegraph::copy_index_into(&mirror, &checkout).await {
            tracing::warn!(error = %e, "codegraph index copy failed");
        }
    }

    // Prefer the resolved fork-point SHA; fall back to the checkout's HEAD when
    // the base was a local/HEAD fallback so the mirror still advances to the
    // exact commit the agent starts on.
    let sha = match base_sha {
        Some(s) => Some(s),
        None => git::rev_parse(&checkout, "HEAD").await.ok(),
    };

    crate::host::spawn(async move {
        let bin = match codegraph::ensure_installed().await {
            Ok(bin) => bin,
            Err(e) => {
                tracing::warn!(error = %e, "codegraph install failed; skipping mirror advance");
                return;
            }
        };
        if let Err(e) = codegraph::ensure_mirror(&source_repo, &mirror, &bin).await {
            tracing::warn!(error = %e, "codegraph ensure_mirror failed");
            return;
        }
        if let Some(sha) = &sha {
            if let Err(e) = codegraph::advance_mirror(&mirror, &source_repo, sha, &bin).await {
                tracing::warn!(error = %e, "codegraph advance_mirror failed");
            }
        }
        // Late index: nothing was copied synchronously, so copy the db the
        // mirror just built — provided the checkout wasn't torn down meanwhile.
        if !copied_sync && checkout_still_reachable(&checkout) {
            if let Err(e) = codegraph::copy_index_into(&mirror, &checkout).await {
                tracing::warn!(error = %e, "codegraph late index copy failed");
            }
        }
    });
}

/// Whether a late index copy should still go to `checkout`: it exists on this
/// host, or it lives in a box that is running. A stopped box is not booted
/// for it — the index is best-effort, and the agent searches files without
/// one — and a discarded box's root is no longer routed, so its leftover
/// host-side placeholder is gone with the agent's dir.
fn checkout_still_reachable(checkout: &Path) -> bool {
    match crate::exec::for_path(checkout) {
        crate::exec::Executor::Local => checkout.exists(),
        crate::exec::Executor::Remote(_) => sandbox::checkout_awake(checkout),
    }
}

/// Run a spawn's `provisioning`, then release the pin its code came from (a
/// fork's, see `PinnedCode::release`) — however provisioning ended: with the
/// code carried over, failed, or abandoned before it got that far. Nothing
/// fetches it after this.
pub(super) async fn provision_then_release(
    provisioning: impl std::future::Future<Output = ()>,
    pin: Option<super::PinnedCode>,
) {
    provisioning.await;
    if let Some(code) = pin {
        code.release().await;
    }
}

/// The repos of its project that `record` has no checkout of: what a spawn
/// attaches after the primary, and what a launch finishing that spawn still
/// has to (`Supervisor::ensure_checkouts`).
fn unattached_repos(record: &AgentRecord, project_repos: Vec<PathBuf>) -> Vec<PathBuf> {
    project_repos
        .into_iter()
        .filter(|path| !record.repos.iter().any(|r| &r.repo_path == path))
        .collect()
}

/// Undo what a failed spawn created: the checkout it provisioned, then the
/// agent's dir. `checkout` is `None` when the spawn *adopted* its working tree
/// — that directory belongs to the run, not the agent, so a step whose process
/// never came up must not take the run's work down with it. The agent dir is
/// always removed: it holds only Fletch-generated per-agent artifacts, and an
/// adopted tree is never inside it (adoption records the real path rather than
/// linking to it, so no recursive delete can reach through).
async fn discard_failed_spawn(agent_id: &str, checkout: Option<&Path>, parent_dir: &Path) {
    // A box-stamped spawn provisioned into its guest: the box goes first, data
    // disk and all, so the host paths below are only its placeholders and a
    // later attempt never finds a half-made checkout on a surviving disk.
    let _ = sandbox::discard_agent_state(agent_id).await;
    if let Some(checkout) = checkout {
        let _ = provision::teardown(checkout).await;
    }
    let _ = tokio::fs::remove_dir_all(parent_dir).await;
}

/// `discard_failed_spawn`, but only if nobody took the agent back first.
///
/// Both paths are keyed by agent id, not by attempt, so an *abandoned* spawn
/// task that deletes them unconditionally deletes whatever a later attempt put
/// there: the agent times out, the user resumes it (`resume_agent`,
/// `respawn_agent_preserving_session`, `restore_agent` — all of which reuse
/// the same checkout), and the stale task then removes the live
/// workspace out from under the running process.
///
/// So the ownership check happens under the lifecycle lock, the same lock every
/// one of those paths holds while it sets `Spawning`: a live status
/// (`Spawning`/`Running`/`Idle`) means a newer attempt owns the paths and we
/// must not touch them. Anything else (`Error`, `Stopped`, or no entry at all)
/// means the attempt we belong to is still the last word, and its leftovers
/// are ours to clean up.
///
/// The lock is supervisor-wide, so the slow part must not run under it: a
/// large checkout's recursive delete would stall every other agent's spawn,
/// resume, switch and restore meanwhile. Instead the agent dir is renamed into
/// the discard root beside the checkouts root (`workspace::discard_root`, so it
/// leaves the agent namespace entirely) while the lock is held — one atomic,
/// cheap `rename` — and the tombstone is removed after the lock is released,
/// or by the next boot's sweep if that fails. A resume that comes
/// after the rename finds no checkout at the agent's path, exactly what it
/// would have found had the whole delete happened under the lock.
async fn discard_if_still_dead(sup: &Supervisor, agent_id: &str, parent_dir: &Path) {
    let tombstone = {
        let _lifecycle_guard = sup.agent_lifecycle.lock().await;
        if matches!(
            sup.live_status(agent_id),
            Some(AgentStatus::Spawning | AgentStatus::Running | AgentStatus::Idle)
        ) {
            tracing::debug!(
                agent_id,
                "abandoned spawn: agent is live again; leaving its checkout in place"
            );
            return;
        }
        if !parent_dir.exists() {
            // Nothing there: the attempt never got as far as creating it.
            return;
        }
        // Set aside under the discard root beside the checkouts root; the
        // move is a rename on the same filesystem. Neither step should fail,
        // and if one does, correctness beats speed and the delete runs under
        // the lock as a one-off.
        let moved = match crate::workspace::discard_tombstone(parent_dir) {
            Ok(tombstone) => tokio::fs::rename(parent_dir, &tombstone)
                .await
                .map(|()| tombstone),
            Err(e) => Err(e),
        };
        match moved {
            Ok(tombstone) => tombstone,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!(
                    agent_id,
                    error = %e,
                    "abandoned spawn: could not set its dir aside; removing in place"
                );
                discard_failed_spawn(agent_id, None, parent_dir).await;
                return;
            }
        }
    };
    // A box-stamped spawn's checkout is on its data disk, which no tombstone
    // sweep reaches: the box goes with the attempt, outside the lock like the
    // delete below (a stop ladder is slow).
    let _ = sandbox::discard_agent_state(agent_id).await;
    // Once set aside the tree is invisible to every other path, so a failed
    // delete costs disk, not correctness: `boot` sweeps leftover tombstones
    // (`workspace::sweep_discarded_checkouts`), which is the retry.
    if let Err(e) = tokio::fs::remove_dir_all(&tombstone).await {
        tracing::warn!(
            agent_id,
            path = %tombstone.display(),
            error = %e,
            "abandoned spawn: could not remove its set-aside dir; the next start will"
        );
    }
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
    start: SessionStart,
    my_gen: u64,
    /// The run blackboard dir to grant a workflow step agent (§8), derived from
    /// the record's `owner_run_id`. `None` for a normal, non-run-owned agent.
    blackboard: Option<PathBuf>,
    /// The adopted checkout is the agent's to read, not write: the local
    /// reviewer (`workspace::PURPOSE_REVIEWER`) reading a coder's tree.
    adopted_read_only: bool,
}

/// Pick the turn-end detector for an agent by provider class, and reset it
/// when this spawn begins a fresh turn. Per-turn agents carry their detector
/// in the descriptor table; claude (no descriptor) has its own.
fn build_activity(record: &AgentRecord, start: &SessionStart) -> Box<dyn Activity> {
    let mut activity: Box<dyn Activity> = match per_turn_descriptor(&record.provider) {
        Some(desc) => (desc.activity)(),
        None => Box::new(ManagedActivity::claude()),
    };
    if *start == SessionStart::Fresh {
        activity.reset_for_new_turn();
    }
    activity
}

/// The session a launch attaches to, read from the session itself: resumed
/// once it has a conversation to resume, fresh (same id) until then. Claude's
/// `--resume` of a session with no message would fail, and its `--session-id`
/// of one that has a transcript is refused ("already in use"), so the
/// transcript decides, never the caller: a session started in place (a
/// rewind) has nothing of its own until its first turn lands, however long
/// the workspace has run, and a session whose transcript Fletch wrote for it
/// (`Supervisor::materialize`) has its conversation from the first launch.
///
/// A per-turn agent resumes nothing at launch (each turn resumes its session
/// by id), so for it a first prompt will do.
fn session_start(record: &AgentRecord, cwd: &Path) -> SessionStart {
    let started = match record.session_id.as_deref() {
        Some(own) if !is_per_turn_provider(&record.provider) => {
            claude_session_has_messages(own, cwd)
        }
        _ => !record.task.trim().is_empty(),
    };
    if started {
        SessionStart::Resume
    } else {
        SessionStart::Fresh
    }
}

/// How a new session's agent learns the conversation it continues (its
/// `lineage`), made ready while the spawn provisions, before the first launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnHandoff {
    /// Natively: that history is written as the session's own transcript
    /// (`Supervisor::materialize`) for its CLI to resume, and the agent is
    /// told `context`, what the session it continues was told.
    Native { context: Option<String> },
    /// From a summary of this client-rendered transcript of it, written in the
    /// Summarizing stage (`crate::handoff`).
    Summary(String),
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
    /// The name the draft showed; `None`/blank draws a fresh one. The id is
    /// never the caller's to choose.
    pub name: Option<String>,
    /// Session-level effort (claude `--effort`), reapplied on every spawn.
    pub effort: Option<String>,
    /// Session-level model override; `None` keeps the provider CLI default.
    pub model: Option<String>,
    /// Custom agent's standing brief, re-injected on every spawn/resume.
    pub instructions: Option<String>,
    /// How the agent learns the conversation the new session continues (a
    /// fork's parent, up to its anchor). `None` tells it nothing.
    pub handoff: Option<SpawnHandoff>,
    /// The history the new session continues (a fork's parent up to its
    /// anchor), written with the session row. `None` starts it empty.
    pub lineage: Option<crate::workspace::SessionLineage>,
    /// Custom agent identity; `None` for a plain built-in spawn.
    pub custom_agent_id: Option<String>,
    /// Custom agent's skills, snapshotted by value (see `agent_profile`).
    pub skills: Vec<crate::agent_profile::SkillSnapshot>,
    /// Custom agent's MCP servers, snapshotted by value (see `agent_profile`).
    pub mcp_servers: Vec<crate::agent_profile::McpServerSnapshot>,
    /// Base the checkout forks from, which also becomes the agent's recorded
    /// parent branch (PR base / ahead-behind). The new-agent screen passes the
    /// chosen base branch here; a workflow step instead passes the previous
    /// step's pinned ref (a commit-ish). `None` falls back to the repo's current
    /// branch.
    pub fork_base: Option<String>,
    /// The workflow run that owns this agent, when spawned as a workflow step.
    /// Persisted on the record so run-owned agents are filterable from the
    /// normal sidebar and cleaned up by `wf_delete_run`. `None` for a normal
    /// user spawn.
    pub owner_run_id: Option<String>,
    /// A pre-provisioned checkout this agent adopts as its primary workspace
    /// instead of cloning one — the workflow kernel's shared run workspace.
    /// When set, provisioning is skipped (`fork_base` only names the parent)
    /// and teardown must never remove the directory: the run owns it, not the
    /// agent. `None` for every non-kernel spawn.
    pub existing_workspace: Option<PathBuf>,
    /// A fork's code: the pinned code of the workspace the new checkouts start
    /// from once they are provisioned, each from the snapshot of its repo
    /// (`Supervisor::start_from`). `None` for a normal spawn or a clean fork.
    pub code_from: Option<super::PinnedCode>,
    /// The GitHub issue this spawn originates from (bare issue number as text),
    /// set by the Home inbox's "Start work". Persisted on the workspace so the
    /// agent's PR closes it. `None` for a spawn not tied to an issue.
    pub issue_ref: Option<String>,
    /// What this workspace is for, when a surface other than the sidebar owns
    /// it (`crate::workspace::PURPOSE_ROADMAP_PM`). Persisted on the record; it
    /// hides the workspace from the sidebar and narrows its capability grant.
    /// `None` for a normal user or workflow spawn.
    pub purpose: Option<String>,
    /// The first prompt, when the caller already has it (the desktop's draft
    /// spawn). Persisted as the record's `task` at creation, so the sidebar row
    /// reads it from the first snapshot instead of waiting for the message to
    /// be delivered once the process is up. `None` leaves the task to
    /// `on_first_user_message`, which captures it at send time.
    pub task: Option<String>,
    /// The provider account this agent runs under, when the spawn picked one
    /// (an id, or `default`) instead of the active account in Settings — the
    /// composer's per-chat override. `None` follows Settings.
    pub account: Option<String>,
    /// Folders on this machine to grant the new agent (the composer's folder
    /// chips, a fork's parent's grants), as requested: each is checked
    /// (`workspace::folder_grants::check`) against the engine the agent is
    /// stamped with before the record is written, so the first launch already
    /// has them and a refused folder fails the spawn with the reason.
    pub folder_grants: Vec<FolderGrant>,
    /// This session's engine override, standing over the host's engine
    /// choice and the project's placement (`sandbox::engine_for_spawn`): the
    /// composer's `Host` or `Box`, or a fork's parent's exact engine
    /// (`sandbox::WorkspaceType::stamped`). `None` lets those decide, as
    /// every other spawn does, after the host's own default
    /// (`sandbox::default_workspace_type`: a cloud box where the host has no
    /// sandbox of its own).
    pub workspace_type: Option<sandbox::WorkspaceType>,
}

impl Supervisor {
    /// The stored `sandbox.placement` of the project `repo_path` belongs to,
    /// if any. A repo no project owns yet (the row is created by the spawn
    /// below) reads as unset.
    fn project_placement(&self, repo_path: &std::path::Path) -> Option<String> {
        let project_id = self
            .workspace
            .project_id_for_repo(&repo_path.to_string_lossy())
            .ok()?;
        self.workspace
            .project_setting(&project_id, sandbox::placement::KEY)
    }

    pub async fn spawn_agent(
        self: Arc<Self>,
        ctx: Arc<EngineCtx>,
        mut req: SpawnRequest,
    ) -> Result<AgentRecord> {
        // The probe can read the Keychain, so it runs before the lifecycle
        // lock every agent shares.
        let chosen_account = match req.account.as_deref() {
            Some(requested) => {
                Some(super::account_switch::chosen_account(&req.provider, requested).await?)
            }
            None => None,
        };
        // Adopting a pre-provisioned tree must be decided before anything else:
        // it is the one mode in which no clone happens, so the base the caller
        // asked for is only ever the *recorded* parent branch, never a fork
        // point — and it decides the engine below. The directory must already
        // be a working tree — the run provisions it before the first step (see
        // `SpawnRequest`), the reviewer's coder has one — and discovering
        // otherwise later means a spawn that failed after the record was
        // written. Asked where the tree lives: a coder's checkout in a box
        // exists only in its guest, and the host path is an empty placeholder;
        // asking may boot that box, so it too runs before the lifecycle lock.
        let adopted = match req.existing_workspace.take() {
            Some(path) => {
                let is_repo = crate::exec::for_path(&path)
                    .exists_async(&path.join(".git"))
                    .await
                    .map_err(|e| {
                        Error::InvalidPath(format!(
                            "adopted workspace {} could not be read: {e}",
                            path.display()
                        ))
                    })?;
                if !is_repo {
                    return Err(Error::InvalidPath(format!(
                        "adopted workspace is not a git repository: {}",
                        path.display()
                    )));
                }
                Some(path)
            }
            None => None,
        };
        // The engine this agent will be stamped with, resolved once: the folder
        // check, the provider gate, the stamp and the provisioning mode below
        // must all see the same one, and the setting can change while the
        // check waits on a privacy prompt. A second read under the lock could
        // stamp an engine the folders were never checked for. An adopting
        // agent runs where its tree lives (`engine_for_adopted_tree`): in the
        // box that holds it, or never in a box for a host tree. Anyone else
        // gets the session's workspace type when it picked one, else the
        // host's default for a spawn that names none (a cloud box, on a host
        // with no sandbox of its own: `sandbox::default_workspace_type`),
        // else the automatic choice over the project's placement — read here,
        // before the lock, since the automatic choice stats the repository
        // and either may run the runtime probe once. A workspace type that
        // cannot be had (a box on a machine that cannot boot one, say) fails
        // the spawn here, before anything is written.
        let placement = match &adopted {
            Some(_) => None,
            None => self.project_placement(&req.repo_path),
        };
        let (tree, provider, repo) = (adopted.clone(), req.provider.clone(), req.repo_path.clone());
        let asked = req.workspace_type;
        let (engine_kind, workspace_type) = tokio::task::spawn_blocking(move || {
            let workspace_type = match (&tree, asked) {
                (None, None) => sandbox::default_workspace_type(&provider),
                (_, asked) => asked,
            };
            sandbox::engine_for_spawn(
                tree.as_deref(),
                placement.as_deref(),
                &repo,
                &provider,
                workspace_type,
            )
            .map(|kind| (kind, workspace_type))
        })
        .await
        .map_err(|e| Error::Other(e.to_string()))??;
        // A box whose machine is at a cloud provider, when the workspace type
        // is one (an adopted tree runs where it lives, whatever was asked):
        // stamped below, and given no folder of this machine, which it could
        // not mount.
        let cloud_provider = match &adopted {
            Some(_) => None,
            None => workspace_type.and_then(sandbox::WorkspaceType::cloud),
        };
        sandbox::ensure_folders_can_be_shared(cloud_provider, &req.folder_grants)?;
        // The folder check may wait on a privacy prompt, so it too runs before
        // the lifecycle lock.
        let folder_grants = crate::workspace::folder_grants::check_all(
            std::mem::take(&mut req.folder_grants),
            engine_kind,
        )
        .await?;
        let _lifecycle_guard = self.agent_lifecycle.lock().await;
        let SpawnRequest {
            view,
            repo_path,
            provider,
            name,
            effort,
            model,
            instructions,
            handoff,
            lineage,
            custom_agent_id,
            skills,
            mcp_servers,
            fork_base,
            owner_run_id,
            // Taken above, where the adopted tree decided the engine.
            existing_workspace: _,
            code_from,
            issue_ref,
            purpose,
            task,
            account: _,
            folder_grants: _,
            // Read above, where it decided the engine.
            workspace_type: _,
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

        sandbox::ensure_supports_provider(engine_kind, &provider)?;

        // The name the draft already showed in the sidebar locks in rather
        // than being regenerated; an empty one (the draft-less spawn path) is
        // drawn with the row. The id is always chosen there, under the same
        // lock as the insert, so neither is final until the row is written,
        // below.
        let supplied_name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_default();

        // The agent's parent branch — the base its checkout forks from and the
        // ref it later targets for PRs / ahead-behind. The base the user chose
        // on the new-agent screen (`fork_base`) wins; absent a choice, resolve
        // the repo's *default* branch.
        //
        // Explicitly NOT the branch the source repo happens to have checked out.
        // Forking a new agent onto the user's in-progress local work is never
        // the intent, and as an implicit default it is invisible: the frontend's
        // draft path passes a base, but every other caller silently inherited
        // whatever the repo was on. Resolving the default here makes the rule
        // hold structurally instead of depending on each caller remembering.
        let parent_branch = match &fork_base {
            Some(base) if !base.trim().is_empty() => base.trim().to_string(),
            _ => git::default_branch(&repo_path).await,
        };
        let subdir = allocate_repo_subdir(&repo_path, &[]);
        // Cloned for the background fork task — `subdir` is moved into
        // `primary` below.
        let subdir_for_fork = subdir.clone();

        let primary = TrackedRepo {
            repo_path: repo_path.clone(),
            subdir: subdir.clone(),
            branch: None, // materialized at first push, named by the agent
            parent_branch: Some(parent_branch),
            pr_base: None,
            base_sha: None,  // captured by the fork task once HEAD is known
            pr_number: None, // set when a PR is opened for this branch
            pr_url: None,
            pr_title: None,
            pr_state: None,
            label: None,
            // Set only by the kernel's adopting spawn; every other spawn owns
            // the checkout the fork task provisions below.
            adopted_checkout: adopted.clone(),
        };

        // Same filter as `on_first_user_message`: a bare slash command is not
        // the task, and the send-time capture will skip it too.
        let initial_task = task
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty() && !t.starts_with('/'))
            .unwrap_or_default();
        let mut record = new_agent_record(
            String::new(),
            supplied_name,
            provider,
            primary,
            initial_task,
            view,
        );
        // Session-level effort (claude `--effort`); persisted so start_process
        // re-applies it on every spawn. Per-turn agents ignore it at spawn.
        record.effort = effort;
        // Session-level model selection. `None` preserves the provider CLI
        // default; selected values are reapplied on resume.
        record.model = model;
        // Custom agent identity + snapshotted brief. Both `None` for a plain
        // built-in spawn. The brief is re-injected on every spawn/resume.
        record.instructions = instructions;
        // The history the session continues; inserted with the session row.
        record.lineage = lineage;
        // A native continuation is told what the session it continues was
        // told; a summary is written while the spawn provisions.
        if let Some(SpawnHandoff::Native { context }) = &handoff {
            record.handoff_context = context.clone();
        }
        record.custom_agent_id = custom_agent_id;
        // Skill/MCP snapshots, persisted like the brief so every process spawn
        // (fresh, resume) re-materializes the same profile.
        record.skills = skills;
        record.mcp_servers = mcp_servers;
        // Stamp the sandbox engine at creation: the agent keeps this engine
        // for life (respawn, restore), so a later settings change
        // never re-engines it — see `spawn_agent_process`, which reuses the
        // stored value instead of the live setting.
        record.sandbox_engine = Some(engine_kind.as_setting().to_string());
        // And where that box's machine is, for life too: a box at a cloud
        // provider keeps the `box` engine and names its provider here.
        record.cloud_provider = cloud_provider.map(|p| p.as_str().to_string());
        // Stamp the provider account: the one the spawn picked, else the
        // active one in Settings. Later moves go through the switch, a
        // Settings change (`follow_active_account`) or a limit retry.
        record.account = chosen_account.unwrap_or_else(|| active_account(&ctx, &record.provider));
        // Tag the agent with its owning run (workflow step spawn) so it's
        // hidden from the normal sidebar and cascaded on run delete.
        record.owner_run_id = owner_run_id;
        // Carry the originating issue (Home inbox "Start work") onto the record
        // so the git dispatcher can close it from the agent's PR.
        record.issue_ref = issue_ref;
        // Tag a workspace owned by a surface other than the sidebar (a Roadmap
        // PM chat) so it's hidden from the sidebar, listed by that surface, and
        // stamped with the narrower capability grant below.
        record.purpose = purpose;
        // Written with the row, so the provisioning box and the first launch
        // see them (`add_agent`).
        record.folder_grants = folder_grants;
        // Insert first: this both gives the workspace its id (and its name,
        // unless the draft supplied one) and writes the row under one lock, so
        // `record.id` is final only afterward.
        self.workspace.add_agent_allocating(&mut record)?;
        let agent_id = record.id.clone();
        let parent_dir = agent_parent_dir(&agent_id)?;
        // Resolved through the record's own repo entry so the adopted tree and
        // the derived layout can't disagree here and in `start_process`.
        let primary_checkout = record.repos[0].checkout_path(&agent_id)?;
        crate::telemetry::track(
            "agent_spawned",
            serde_json::json!({
                "provider": record.provider,
                "model": record.model,
                "effort": record.effort,
            }),
        );
        // This attempt, for everything below that acts on the agent: once it
        // is ended from outside (the watchdog, a teardown, a stop of its
        // machine) the abandoned task can neither go on under a later attempt
        // nor report its own end as the agent's.
        let launch = self.begin_launch(&ctx, &agent_id);
        // A new row is a structural change: `agent:status` alone is dropped by
        // any view that doesn't have the agent yet (the desktop window when a
        // paired phone spawned it, and vice versa). Announce it so every other
        // client refetches the workspace now rather than on its next focus. The
        // caller that issued the spawn refetches on its own; the generation
        // guard in `refreshWorkspace` makes the extra fetch harmless.
        emit_workspace_changed(ctx.sink.as_ref());
        let sup = self.clone();
        let ctx_for_task = ctx.clone();
        let id_for_task = agent_id.clone();
        let project_id_for_task = record.project_id.clone();
        let provider_for_task = record.provider.clone();
        // The box a box-stamped spawn provisions into boots with the shares
        // its first launch will ask for, so that launch need not restart it.
        let record_for_task = record.clone();
        let model_for_task = record.model.clone();
        let mcp_servers_for_task = record.mcp_servers.clone();
        let adopted_for_task = adopted.is_some();
        let pin = code_from.clone();
        // The login resolves alongside provisioning rather than after it, so a
        // refresh doesn't add to the spawn's wait (`prefetch_login`).
        {
            let (sup, id) = (self.clone(), agent_id.clone());
            crate::host::spawn(async move {
                let _ = sup.prefetch_login(&id).await;
            });
        }
        let provisioning = async move {
            // Stage markers for the clients, emitted ahead of each step so the
            // spinner behind `spawning` can say what it is waiting on. Progress
            // only: a dropped one costs a label.
            //
            // Doubles as this task's cooperation point with the spawn watchdog,
            // which claims the `Spawning` status and kills the process but
            // cannot cancel us. `false` means *this attempt's* outcome was
            // already claimed elsewhere (timeout, teardown) — or the agent has
            // since been restarted under a later attempt: the caller must clean
            // up what it owns and return, so no stage lands after the terminal
            // status and no provisioning keeps running for a dead agent or over
            // a live one.
            //
            // Cleaning up goes through `discard_if_still_dead`, never
            // `discard_failed_spawn` directly: the second of those two reasons
            // is exactly the case where the checkout is no longer ours to
            // delete, and only that helper can tell them apart.
            let progress = |stage, detail| {
                if !sup.launch_is_live(&launch) {
                    tracing::debug!(
                        agent_id = %id_for_task,
                        ?stage,
                        "spawn outcome already claimed; abandoning provisioning"
                    );
                    return false;
                }
                emit_spawn_progress(ctx_for_task.sink.as_ref(), &id_for_task, stage, detail);
                true
            };
            // Nothing created yet, so there is nothing to discard here.
            if !progress(SpawnStage::Preparing, None) {
                return;
            }
            if let Err(e) = tokio::fs::create_dir_all(&parent_dir).await {
                fail_spawn(&sup, &ctx_for_task, &launch, e.to_string());
                return;
            }

            // Fork point: the workspace fetches its base branch from `origin`
            // itself and detaches at the tip that came back, so the agent starts
            // from the branch as GitHub has it — see
            // `provision::provision_at_remote_base` for why fetching in the
            // *clone* (not the source) is what makes that true.
            //
            // `base_ref` is the offline fallback: the base as this machine
            // already knows it, resolved to a local SHA and finally to HEAD so
            // an unresolvable base degrades instead of failing the spawn. A SHA,
            // never the branch name — a clone-mode workspace only has the
            // source's HEAD branch as a local branch, so checking out any
            // *other* branch by name trips git's remote-DWIM (an implicit `-b`),
            // which is fatal combined with `--detach`.
            //
            // An adopted tree skips every rung of this: it already exists at the
            // commit the run put it on, so there is no fork to make, no base to
            // fetch, and no freshness to degrade — `fork_base` is an input to
            // provisioning only, and provisioning is what adoption replaces.
            // The agent dir exists from here on, but no checkout does yet — so
            // that dir is the whole of what this task has to undo.
            if !adopted_for_task && !progress(SpawnStage::Cloning, None) {
                discard_if_still_dead(&sup, &id_for_task, &parent_dir).await;
                return;
            }
            let provision_result = if adopted_for_task {
                Ok(())
            } else {
                sup.provision_repo(&record_for_task, &record_for_task.repos[0])
                    .await
            };
            // Nothing is undone: the agent's next launch makes the checkout
            // this one could not (`ensure_checkouts`).
            if let Err(e) = provision_result {
                fail_spawn(&sup, &ctx_for_task, &launch, e.to_string());
                return;
            }

            // What a failed spawn below is allowed to remove — never an adopted
            // tree (see `discard_failed_spawn`).
            let owned_checkout = (!adopted_for_task).then(|| primary_checkout.clone());

            // Record the fork point so diffs measure against the exact starting
            // commit rather than a branch name that can drift. Non-fatal: a
            // missing base_sha just falls back to the parent branch name. Read
            // from the checkout either way, so an adopted tree's base is the
            // commit the run left it on rather than a fork that never happened.
            let base_sha = git::rev_parse(&primary_checkout, "HEAD").await.ok();
            if let Some(sha) = &base_sha {
                let _ = sup
                    .workspace
                    .set_repo_base_sha(&id_for_task, &subdir_for_fork, sha);
            }

            // Warm the codegraph index for this checkout (best-effort; no-op when
            // indexing is off). Runs the cheap copy inline and advances the
            // mirror in the background.
            if !progress(SpawnStage::Indexing, None) {
                discard_if_still_dead(&sup, &id_for_task, &parent_dir).await;
                return;
            }
            provision_codegraph_index(
                project_id_for_task.clone(),
                repo_path.clone(),
                primary_checkout.clone(),
                base_sha.clone(),
                &provider_for_task,
                &mcp_servers_for_task,
            )
            .await;

            // Multi-repo project: fork a checkout of every *other* repo of the
            // project too, so the agent works across all of them from turn one
            // (untouched repos are just clean clones). Runs before
            // start_process so the git RPC dispatcher sees the full repo set. Workflow step agents stay
            // single-repo — a run targets one repo (see `wf_launch`), and a step
            // agent adopts that run's tree.
            if !adopted_for_task {
                for source in sup.workspace.project_repo_paths(&project_id_for_task) {
                    if source == repo_path {
                        continue;
                    }
                    if !progress(
                        SpawnStage::AttachingRepos,
                        source.file_name().map(|n| n.to_string_lossy().into_owned()),
                    ) {
                        discard_if_still_dead(&sup, &id_for_task, &parent_dir).await;
                        return;
                    }
                    if let Err(e) = sup
                        .attach_repo_checkout(&ctx_for_task, &id_for_task, source.clone())
                        .await
                    {
                        discard_failed_spawn(&id_for_task, owned_checkout.as_deref(), &parent_dir)
                            .await;
                        fail_spawn(
                            &sup,
                            &ctx_for_task,
                            &launch,
                            format!("checkout of {} failed: {e}", source.display()),
                        );
                        return;
                    }
                }
            }

            // A fork's code: the checkouts start from it, once all of them
            // exist. Fatal on failure — the user asked for this code, so
            // silently producing a clean fork would drop it. A workflow step
            // never forks code, so this is always a non-run clone, torn down
            // like the start_process failure path below.
            if let Some(code) = &code_from {
                if !progress(SpawnStage::Carrying, None) {
                    discard_if_still_dead(&sup, &id_for_task, &parent_dir).await;
                    return;
                }
                if let Err(e) = sup.start_from(&id_for_task, code).await {
                    discard_failed_spawn(&id_for_task, owned_checkout.as_deref(), &parent_dir)
                        .await;
                    fail_spawn(&sup, &ctx_for_task, &launch, e.to_string());
                    return;
                }
            }

            // The conversation the session continues, in place before the first
            // launch reads it: written as the session's own transcript, once
            // the checkouts it is found from exist, or condensed into what its
            // agent is told. A copy that fails fails the spawn, since the user
            // asked for that conversation; a summary that can't be made falls
            // back to the transcript's tail (`handoff::context`), so only
            // failing to store it does.
            match &handoff {
                Some(SpawnHandoff::Native { .. }) => {
                    if let Err(e) = sup.materialize(&id_for_task) {
                        discard_failed_spawn(&id_for_task, owned_checkout.as_deref(), &parent_dir)
                            .await;
                        fail_spawn(
                            &sup,
                            &ctx_for_task,
                            &launch,
                            format!("Couldn't carry the conversation over: {e}"),
                        );
                        return;
                    }
                }
                Some(SpawnHandoff::Summary(transcript)) => {
                    if !progress(SpawnStage::Summarizing, None) {
                        discard_if_still_dead(&sup, &id_for_task, &parent_dir).await;
                        return;
                    }
                    let context = crate::handoff::context(
                        &provider_for_task,
                        model_for_task.as_deref(),
                        transcript,
                    )
                    .await;
                    if let Err(e) = sup.workspace.set_handoff_context(&id_for_task, &context) {
                        discard_failed_spawn(&id_for_task, owned_checkout.as_deref(), &parent_dir)
                            .await;
                        fail_spawn(&sup, &ctx_for_task, &launch, e.to_string());
                        return;
                    }
                }
                None => {}
            }

            if !progress(SpawnStage::Starting, None) {
                discard_if_still_dead(&sup, &id_for_task, &parent_dir).await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(350)).await;

            // Armed for the process only. Everything above is bounded step by
            // step and fails with its own reason; a clone that takes its time
            // (a VM boot queued behind others, a slow network) is not a
            // process that did not come up.
            arm_spawn_timeout(sup.clone(), ctx_for_task.clone(), launch.clone());
            // A process that fails to start leaves its workspace in place: it
            // is whole, and the agent's next launch starts in it.
            if let Err(e) = sup.start_process(&ctx_for_task, &launch).await {
                fail_spawn(&sup, &ctx_for_task, &launch, e.to_string());
            }
        };
        crate::host::spawn(provision_then_release(provisioning, pin));

        Ok(record)
    }

    /// Bring a second (or third…) repo into a live agent. Creates a
    /// detached checkout at `~/.fletch/workspaces/<agent-id>/<subdir>/`
    /// and appends a TrackedRepo entry. The checkout stays detached until
    /// its first push, consistent with the primary repo.
    pub async fn add_repo_to_agent(
        self: Arc<Self>,
        ctx: Arc<EngineCtx>,
        agent_id: &str,
        repo_path: PathBuf,
    ) -> Result<TrackedRepo> {
        self.attach_repo_checkout(&ctx, agent_id, repo_path).await
    }

    /// Shared core of "give this agent a checkout of another repo": validate,
    /// allocate a sibling subdir, fork a checkout, and append the TrackedRepo.
    async fn attach_repo_checkout(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
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
        // Secondary repos have no base picker of their own, so they take the
        // repo's default branch — never the branch it happens to have checked
        // out, for the same reason the primary repo doesn't (see `spawn_agent`).
        let parent_branch = git::default_branch(&repo_path).await;

        // Fork from the base as `origin` has it, falling back to this machine's
        // best-known tip for that branch when the fetch can't happen — never
        // `HEAD`, which is an unrelated branch (see `git::base_tip`). Then
        // record the fork point as the diff base.
        let fallback = git::base_tip(&repo_path, &parent_branch).await;
        let spec = CheckoutSpec {
            source_repo: &repo_path,
            base_ref: fallback.as_deref().unwrap_or(&parent_branch),
            dest: &checkout,
        };
        sandbox::provision_checkout(
            sandbox::stamped_engine(&record),
            &record,
            &spec,
            &parent_branch,
        )
        .await?;
        let base_sha = git::rev_parse(&checkout, "HEAD").await.ok();

        // Warm the codegraph index for this additional checkout too, so a
        // multi-repo workspace (and a live "+ Repo") indexes every repo.
        provision_codegraph_index(
            record.project_id.clone(),
            repo_path.clone(),
            checkout.clone(),
            base_sha.clone(),
            &record.provider,
            &record.mcp_servers,
        )
        .await;

        let repo = TrackedRepo {
            repo_path: repo_path.clone(),
            subdir: subdir.clone(),
            branch: None,
            parent_branch: Some(parent_branch),
            pr_base: None,
            base_sha,
            pr_number: None,
            pr_url: None,
            pr_title: None,
            pr_state: None,
            label: None,
            adopted_checkout: None,
        };
        self.workspace.append_tracked_repo(agent_id, repo.clone())?;
        emit_repo_added(ctx.sink.as_ref(), agent_id, repo.clone());

        // No branch is created here — the new repo's checkout stays detached
        // until its first push, when the agent names its branch (same as the
        // primary repo).
        Ok(repo)
    }

    /// Fork `repo`'s checkout for `record` from its parent branch as `origin`
    /// has it. A checkout that could not be finished is removed, so one that
    /// is there is one that was made whole.
    async fn provision_repo(&self, record: &AgentRecord, repo: &TrackedRepo) -> Result<()> {
        let checkout = repo.checkout_path(&record.id)?;
        let parent_branch = match &repo.parent_branch {
            Some(branch) => branch.clone(),
            None => git::default_branch(&repo.repo_path).await,
        };
        // Offline fallback: the base as this machine already knows it, never
        // `HEAD` — that is the source repo's checked-out branch, unrelated to
        // the base the user picked (see `git::base_tip`). With no local copy
        // at all, pass the branch name so an unreachable `origin` fails the
        // provisioning instead of opening somewhere arbitrary.
        let fallback = git::base_tip(&repo.repo_path, &parent_branch).await;
        let spec = CheckoutSpec {
            source_repo: &repo.repo_path,
            base_ref: fallback.as_deref().unwrap_or(&parent_branch),
            dest: &checkout,
        };
        let engine = sandbox::stamped_engine(record);
        match sandbox::provision_checkout(engine, record, &spec, &parent_branch).await {
            Ok(freshness) => {
                let primary = record
                    .repos
                    .first()
                    .is_some_and(|p| p.subdir == repo.subdir);
                if primary && freshness == provision::BaseFreshness::Stale {
                    // The workspace couldn't reach `origin`, so it starts at
                    // whatever this machine last had. Flag it so the agent's
                    // brief carries a heads-up and it can tell the user (see
                    // `start_process`) — silently working off a stale base is
                    // how an agent rebuilds something that already landed.
                    self.stale_base.lock().insert(record.id.clone());
                }
                Ok(())
            }
            Err(e) => {
                provision::discard(&checkout).await;
                Err(e)
            }
        }
    }

    /// Make the checkouts `agent_id` should have and does not: a spawn that
    /// failed before it had cloned (no network, a VM that did not start)
    /// leaves its agent without one, and the launch that resumes the agent
    /// finishes the job. Nothing to do for any other agent.
    ///
    /// A missing primary checkout is the mark of a spawn that never finished,
    /// whichever step it failed at, so making it also brings in the project's
    /// repos the spawn never got to attach — and only then: a repo the project
    /// gained later is not pushed into an agent that was set up without it.
    ///
    /// Not for a session that continues another (a fork, a handoff): what its
    /// spawn was to carry over is not here to be carried again.
    async fn ensure_checkouts(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
    ) -> Result<()> {
        let record = self.workspace.agent(agent_id)?;
        let mut unfinished_spawn = None;
        for repo in record.repos.iter().filter(|r| r.adopted_checkout.is_none()) {
            let checkout = repo.checkout_path(agent_id)?;
            if checkout.exists() {
                continue;
            }
            if record.lineage.is_some() {
                return Err(Error::Other(
                    "this workspace was never set up, and what it was to start from is gone. \
                     Discard it and start again."
                        .into(),
                ));
            }
            tracing::info!(agent_id, subdir = %repo.subdir, "making the checkout the agent's spawn did not");
            tokio::fs::create_dir_all(agent_parent_dir(agent_id)?).await?;
            self.provision_repo(&record, repo).await?;
            if record
                .repos
                .first()
                .is_some_and(|p| p.subdir == repo.subdir)
            {
                unfinished_spawn = Some(checkout.clone());
            }
            let base_sha = git::rev_parse(&checkout, "HEAD").await.ok();
            if let Some(sha) = &base_sha {
                let _ = self
                    .workspace
                    .set_repo_base_sha(agent_id, &repo.subdir, sha);
            }
            provision_codegraph_index(
                record.project_id.clone(),
                repo.repo_path.clone(),
                checkout,
                base_sha,
                &record.provider,
                &record.mcp_servers,
            )
            .await;
        }
        let Some(primary_checkout) = unfinished_spawn else {
            return Ok(());
        };
        for source in unattached_repos(
            &record,
            self.workspace.project_repo_paths(&record.project_id),
        ) {
            if let Err(e) = self
                .attach_repo_checkout(ctx, agent_id, source.clone())
                .await
            {
                // The mark goes back, so the next launch attaches what this
                // one could not rather than starting without it.
                provision::discard(&primary_checkout).await;
                return Err(Error::Other(format!(
                    "checkout of {} failed: {e}",
                    source.display()
                )));
            }
        }
        Ok(())
    }

    /// Launch `agent_id`'s current session: fresh or resumed, as the session
    /// itself says (`session_start`).
    pub(super) async fn start_process(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        launch: &Launch,
    ) -> Result<()> {
        let agent_id = launch.agent_id();
        // Never into checkouts a move has, or left unsettled: what a move
        // left is finished first, when it can be, and the checkouts it runs
        // in are held until it is up, so no move starts under the launch
        // (`sandbox_move`).
        self.settle_before_acting(agent_id).await?;
        let record = self.workspace.agent(agent_id)?;
        let _held = self.hold_checkouts_of(&record)?;
        let per_turn = is_per_turn_provider(&record.provider);
        // `None` only for a per-turn agent before its first turn
        // (`AgentRecord::launch_session`).
        let session_id = record.launch_session()?.map(str::to_string);
        let primary = record
            .repos
            .first()
            .ok_or_else(|| Error::Other("agent has no tracked repos".into()))?;
        let cwd = primary.checkout_path(agent_id)?;
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
        let base_branch = primary.base_branch().await;
        // Every tracked checkout is addressable by its subdir via `args.repo`;
        // the primary stays the default when the arg is absent, so single-repo
        // agents (and old prompts) behave exactly as before.
        let mut repo_targets = Vec::new();
        for r in &record.repos {
            let checkout = r.checkout_path(agent_id)?;
            // The branches this checkout's pushes created, recorded as each
            // push's event was applied. Threaded here so a force push is fenced
            // to them and a push to one of them is never "the base"; a branch
            // born this turn joins when this dispatcher is rebuilt on the next
            // resume/turn (see `rpc::caps::refuses_publish`). A read that fails
            // owns nothing, which only refuses more.
            let owned = self
                .workspace
                .owned_branches(agent_id, &r.subdir)
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, agent_id = %agent_id, subdir = %r.subdir,
                        "reading the checkout's own branches failed; forcing is refused");
                    Vec::new()
                });
            repo_targets.push(rpc::git::RepoTarget {
                subdir: r.subdir.clone(),
                cwd: checkout,
                source_repo: r.repo_path.clone(),
                base_branch: r.base_branch().await,
                spawn_base: r.spawn_base().await,
                owned,
            });
        }
        // A workspace started from an issue carries its ref here ("123" for
        // GitHub, "ENG-123" for Linear); it seeds the agent's live issue ref,
        // which the dispatcher reads at open_pr time to append the matching
        // closing trailer to the primary repo's PR body (a mid-session
        // composer pick replaces the seed).
        let close_issue = record.issue_ref.clone().filter(|s| !s.trim().is_empty());
        // A run-owned step agent must never publish: the run's own finalize is
        // the only push path for a workflow, and it is `wf/`-namespace guarded.
        // Stamped here, at spawn, so a later policy change can't widen an agent
        // that is already running (`rpc::caps`).
        // An advisory chat (the Roadmap PM) must never publish either: its
        // whole mandate is to read the codebase and propose tickets, so the
        // grant matches the mandate rather than relying on the brief to hold.
        // The local reviewer (`workspace::PURPOSE_REVIEWER`) likewise: it
        // reads a coder's checkout and reports findings, nothing more.
        let caps = match (record.purpose.as_deref(), record.owner_run_id.as_deref()) {
            (Some(crate::workspace::PURPOSE_ROADMAP_PM), _) => rpc::caps::AgentCaps::advisory(),
            (Some(crate::workspace::PURPOSE_REVIEWER), _) => rpc::caps::AgentCaps::reviewer(),
            (_, Some(_)) => rpc::caps::AgentCaps::run_owned(),
            _ => rpc::caps::AgentCaps::interactive(),
        };
        let git_dispatcher =
            rpc::git::GitDispatcher::new(cwd.clone(), primary.repo_path.clone(), base_branch, caps)
                .with_repos(repo_targets)
                .with_close_issue(agent_id, close_issue)
                .with_approval(ctx.sink.clone(), agent_id)
                .with_review(ctx.clone(), agent_id);
        // A roadmap PM chat gets the roadmap ops (roadmap_list /
        // roadmap_propose), scoped to its own project; a local reviewer gets
        // `review_submit` (`rpc::review`), the one op its mandate needs. Both
        // wrap the git dispatcher, and everything else still falls through to
        // it. Every other agent, a run-owned step agent included, keeps the git
        // dispatcher unchanged (its `caps` above already fence what it may
        // publish).
        let rpc_dispatcher: Arc<dyn rpc::RpcDispatcher> = match record.purpose.as_deref() {
            Some(crate::workspace::PURPOSE_ROADMAP_PM) => {
                Arc::new(rpc::roadmap::RoadmapDispatcher::new(
                    ctx.clone(),
                    ctx.db.clone(),
                    record.project_id.clone(),
                    git_dispatcher,
                ))
            }
            Some(crate::workspace::PURPOSE_REVIEWER) => Arc::new(
                rpc::review::ReviewDispatcher::new(ctx.clone(), agent_id, git_dispatcher),
            ),
            _ => Arc::new(git_dispatcher),
        };
        // Every agent can drive its own Run-panel process and raise a
        // notification (`rpc::run`): the ops publish nothing, so there is no
        // grant to check, and a step agent verifying its change by running
        // the app is as legitimate as an interactive one.
        let rpc_dispatcher: Arc<dyn rpc::RpcDispatcher> = Arc::new(rpc::run::RunDispatcher::new(
            Arc::new(super::run::RunControlHandle::new(self.clone(), ctx.clone())),
            agent_id,
            rpc_dispatcher,
        ));
        // Every agent of a project with the context layer on also gets the
        // `context_*` ops, whichever dispatcher it was given above.
        let rpc_dispatcher = rpc::context::wrap(
            ctx,
            rpc_dispatcher,
            &record.project_id,
            agent_id,
            &record.provider,
            session_id.as_deref(),
        );

        // A session from before claude accounts became token sources lives in
        // the account's dir; this launch resumes in the default one. Only a
        // seatbelt agent's sessions are in the host's dirs at all: a box keeps
        // them inside its guest.
        let sessions_on_host = sandbox::stamped_engine(&record).caps().shares_host_fs;
        if record.provider == "claude" && sessions_on_host {
            if let Some(session) = record.session_id.as_deref() {
                match crate::transcripts::adopt_account_session(session, &cwd) {
                    Ok(Some(path)) => tracing::info!(
                        agent_id,
                        path = %path.display(),
                        "moved a claude session out of its account dir to resume it"
                    ),
                    Ok(None) => {}
                    Err(e) => tracing::warn!(
                        agent_id,
                        error = %e,
                        "could not move a claude session out of its account dir"
                    ),
                }
            }
        }
        let start = session_start(&record, &cwd);

        let agent_id_str = agent_id.to_string();
        // The process runs under its launch's generation, so whatever retires
        // the one retires the other.
        let my_gen = launch.gen;
        // Ended from outside while it was getting ready: the agent's runtime
        // is no longer this launch's to touch.
        if self.launch_superseded(launch) {
            return Err(launch_ended());
        }

        // A fresh process must never inherit a run the old one opened.
        self.provider_runs.lock().remove(&agent_id_str);
        self.activities
            .lock()
            .insert(agent_id_str.clone(), build_activity(&record, &start));

        let agent = self
            .spawn_agent_process(
                ctx,
                &agent_id_str,
                &record,
                ProcessLaunch {
                    cwd,
                    sandbox_root,
                    rpc_dir: rpc_dir.clone(),
                    session_id,
                    per_turn,
                    start,
                    my_gen,
                    blackboard,
                    // Derived from the purpose, like the caps above: a reviewer
                    // reads the coder's checkout it adopts and must not be able
                    // to change it, whatever its brief or a persuaded model says.
                    adopted_read_only: record.purpose.as_deref()
                        == Some(crate::workspace::PURPOSE_REVIEWER),
                },
            )
            .await?;

        // Kept only by a launch that is still its agent's: one ended while
        // the process was coming up (a stop of its machine, the spawn timeout,
        // a teardown) lets the process go and reports nothing, since whoever
        // ended it already left the agent as it should be. Checked under the
        // `agents` lock, so an end that comes after this finds the process in
        // the map and takes it down itself.
        let agent = Arc::new(agent);
        {
            let mut agents = self.agents.lock();
            if self.launch_superseded(launch) {
                drop(agents);
                let _ = agent.shutdown();
                return Err(launch_ended());
            }
            agents.insert(agent_id_str.clone(), agent.clone());
        }

        // Initial status is always Idle now — at process start there's
        // never an in-flight turn (we no longer pass a task as a spawn
        // arg). The user's first send flips it to Running. The launch settles
        // atomically, so a turn its process already started on its own is not
        // clobbered: that one keeps the status it raised, under the launch's
        // generation. A launch ended between the check above and here keeps
        // nothing, whether or not its end already took the process.
        let promoted = self.settle_launch(ctx, launch, AgentStatus::Idle, None);
        if !promoted && self.launch_superseded(launch) {
            let ours = {
                let mut agents = self.agents.lock();
                match agents.get(&agent_id_str) {
                    Some(current) if Arc::ptr_eq(current, &agent) => agents.remove(&agent_id_str),
                    _ => None,
                }
            };
            if let Some(agent) = ours {
                let _ = agent.shutdown();
            }
            return Err(launch_ended());
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
            drain_message_queue(self, ctx, &agent_id_str);
        }

        // Watch this agent's RPC mailbox for the life of this generation,
        // executing allowlisted ops and writing responses back.
        spawn_rpc_watcher(
            self.clone(),
            ctx.clone(),
            agent_id_str,
            rpc_dispatcher,
            rpc_dir,
            my_gen,
        );

        Ok(())
    }

    /// Launch `agent_id`: `Spawning` under the spawn watchdog, then
    /// [`Self::start_process`], after `pause` when given (time for a process
    /// it replaces to let go of its session). A failed launch leaves the
    /// agent in `Error` with the reason, and hands the error back; one ended
    /// from outside (`Launch`) hands back that it was, and leaves the agent as
    /// whoever ended it did.
    pub(super) async fn launch(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        pause: Option<Duration>,
    ) -> Result<()> {
        let launch = self.begin_launch(ctx, agent_id);
        // Ahead of the watchdog, which times the process: a move left
        // unsettled is finished, then a checkout the agent's spawn never made
        // is made, neither on the process's clock.
        let ready = async {
            self.settle_before_acting(agent_id).await?;
            self.ensure_checkouts(ctx, agent_id).await
        };
        if let Err(e) = ready.await {
            fail_spawn(self, ctx, &launch, e.to_string());
            return Err(e);
        }
        arm_spawn_timeout(self.clone(), ctx.clone(), launch.clone());
        if let Some(pause) = pause {
            tokio::time::sleep(pause).await;
        }
        if let Err(e) = self.start_process(ctx, &launch).await {
            fail_spawn(self, ctx, &launch, e.to_string());
            return Err(e);
        }
        Ok(())
    }

    /// Spawn the agent's child process, dispatching on provider class
    /// (per-turn vs. claude).
    /// Returns the live `Agent` handle for the supervisor to track.
    async fn spawn_agent_process(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
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
            start,
            my_gen,
            blackboard,
            adopted_read_only,
        } = launch;
        let agent_id_str = agent_id.to_string();

        let engine = sandbox::stamped_engine(record);
        // Re-checked on every launch path (resume, binary-swap
        // respawn), not just fresh spawns: not every provider runs in a box.
        sandbox::ensure_supports_provider(engine, &record.provider)?;
        // Before the profile is materialized under `sandbox_root` below, so
        // those writes land where the agent will run (a tenant's, in the box
        // that holds its tree) rather than on the host.
        sandbox::before_launch(record)?;
        // Every launch path, so a checkout older than the attribution hook
        // gets it on the agent's next start.
        crate::attribution::refresh_hooks(agent_id, &record.repos).await;

        // Dynamically fold the codegraph MCP server into the session's snapshot
        // for this launch, when code indexing is on, the binary is there for
        // this agent's sandbox (the
        // host's install, or the box's tools disk), and this provider can
        // actually receive an MCP server at all. Injected here (not persisted
        // onto the session snapshot) so this runs identically for fresh spawns
        // and resumes and the toggle's *current* state always wins. A
        // user-defined "codegraph" server suppresses injection (see
        // `codegraph`).
        // Not into a read-only adoption (the local reviewer): the in-tree
        // index is SQLite in WAL mode, which needs writes beside the db even to
        // read, so the server would fail in a tree the reviewer may not write —
        // and the instructions would advertise a tool that doesn't work.
        let crate::codegraph::McpInjection {
            servers: mcp_servers,
            codegraph_available,
        } = if adopted_read_only {
            crate::codegraph::McpInjection {
                servers: record.mcp_servers.clone(),
                codegraph_available: false,
            }
        } else {
            crate::codegraph::inject_mcp_server(
                &record.mcp_servers,
                engine,
                agent_id,
                &record.provider,
            )
        };

        // Materialize the session's skill snapshot under the writable root and
        // fold its index into the injected instructions. Recomputed on every
        // launch path so the files always exist (e.g. after a checkout is
        // recreated) and always match the snapshot. `None` when the session has
        // neither a brief nor skills — the pre-profile behavior.
        // Multi-repo workspaces get a layout note composed ahead of the custom
        // brief, so the agent knows about its sibling checkouts from turn one.
        // Recomputed every launch, like the rest of the instruction layers.
        let workspace_note = crate::instructions::multi_repo_workspace_note(&record.repos);
        // A workspace whose base fetch failed at provision time carries a
        // heads-up so the agent can warn the user its starting point may be
        // behind (see `spawn_agent`). Keyed on the primary repo's base — the one
        // the user picked and the one the flag was set for.
        let stale_primary = self
            .stale_base
            .lock()
            .contains(agent_id)
            .then(|| record.repos.first())
            .flatten();
        let stale_note = match stale_primary {
            Some(primary) => Some(crate::instructions::stale_base_note(
                &primary.base_branch().await,
            )),
            None => None,
        };
        // Which env keys reach app-run processes vs. exist but were withheld —
        // key names only, never values (see `run_env`; a value in the
        // instructions would defeat the membrane). Keyed on the primary repo,
        // where the (gitignored) `.env` lives. Recomputed every launch, like
        // the other layers, so a sharing change lands on the next spawn/resume.
        let env_note = record.repos.first().and_then(|primary| {
            let (shared, unshared) = self
                .workspace
                .run_env_key_names(&record.project_id, &primary.repo_path);
            crate::instructions::env_awareness_note(&shared, &unshared)
        });
        let notes = [stale_note, workspace_note, env_note]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n\n");
        let brief = match (notes.is_empty(), record.instructions.as_deref()) {
            (false, Some(brief)) => Some(format!("{notes}\n\n{brief}")),
            (false, None) => Some(notes),
            (true, brief) => brief.map(str::to_string),
        };
        // The conditional playbooks ride the same suffix, each gated on the
        // session actually having what it describes: codegraph on the server
        // having really landed above, and the roadmap ops on this being a
        // project-manager chat (the only purpose given the
        // `rpc::roadmap::RoadmapDispatcher` — see `launch_agent_process`).
        let roadmap_pm = record.purpose.as_deref() == Some(crate::workspace::PURPOSE_ROADMAP_PM);
        // The PM's not-doing digest (`roadmap::not_doing::digest`), read here
        // because this is the site that knows *which project* this chat
        // belongs to — the same stamp `launch_agent_process` gives the RPC
        // dispatcher. Read on every launch path, like every other instruction
        // layer, so a resumed chat whose board rejected something comes back
        // with the current log. A read failure degrades to no digest: an agent
        // with one section missing is worth more than a spawn that fails.
        let not_doing = roadmap_pm
            .then(|| {
                let conn = ctx.db.lock();
                crate::roadmap::not_doing::digest(&conn, &record.project_id).ok()
            })
            .flatten()
            .flatten();
        // The context layer's overview, the same for every session on the
        // project, the PM's included; `None` when the layer is off for this
        // project, in which case the session has no `context_*` ops either
        // (`rpc::context::wrap` wraps every dispatcher, the roadmap one too).
        let context_overview = rpc::context::spawn_overview(ctx, &record.project_id);
        let blocks = crate::agent_profile::Blocks {
            codegraph: codegraph_available,
            roadmap_pm,
            reviewer: record.purpose.as_deref() == Some(crate::workspace::PURPOSE_REVIEWER),
            not_doing: not_doing.as_deref(),
            context: context_overview.as_deref(),
        };
        let instructions = crate::agent_profile::effective_instructions(
            brief.as_deref(),
            record.handoff_context.as_deref(),
            &record.skills,
            &sandbox_root,
            blocks,
        )?;
        // MCP delivery is resolved per provider at spawn from this snapshot
        // (`agent::mcp_delivery`), so there's no provider fork here: the spawn
        // path writes whatever config file the provider reads and adds the
        // argv/env pointing at it. See `agent_profile`.

        // The authoritative source repos each checkout was forked from — the
        // user's own repos, which the agent cannot write. Threaded into the
        // sandbox engine, which derives what it reads back from these, never
        // from the agent-writable checkout alternates. See
        // `sandbox::engine::AgentLaunchCtx`.
        let source_repos: Vec<PathBuf> = record.repos.iter().map(|r| r.repo_path.clone()).collect();
        // The folders the user granted, minus any that are gone since.
        let folder_grants = crate::workspace::folder_grants::present(&record.folder_grants);

        if per_turn {
            // No process spawns until the first user message. No sandbox
            // profile here: each turn's launch is confined when it starts.
            spawn_per_turn_agent(
                &record.provider,
                PerTurnSpec {
                    agent_id: agent_id_str.clone(),
                    project_id: record.project_id.clone(),
                    cwd,
                    sandbox_root,
                    source_repos: source_repos.clone(),
                    session_id,
                    // model/effort aren't spawn-time for per-turn agents:
                    // the supervisor resolves them from the record and passes
                    // them into each turn's dispatch (see deliver_user_message).
                    instructions: instructions.clone(),
                    mcp_servers: mcp_servers.clone(),
                    rpc_dir,
                    engine,
                    account: record.account.clone(),
                    blackboard: blackboard.clone(),
                    adopted_read_only,
                    folder_grants: folder_grants.clone(),
                },
                ctx.clone(),
                agent_id_str.clone(),
                self.clone(),
                my_gen,
            )
        } else {
            let session_id = session_id
                .as_deref()
                .expect("non-codex agents always have a session id");
            // Every launch path signs in here, so a resume always starts on a
            // token with the full refresh margin ahead of it — usually the one
            // its caller resolved before taking the lifecycle lock
            // (`prefetch_login`). A token the API rejected (`observe_login`)
            // is replaced even if it looks unexpired.
            let oauth_token = self.launch_login(agent_id, record).await?;
            let spec = SpawnSpec {
                agent_id: &agent_id_str,
                project_id: &record.project_id,
                cwd,
                sandbox_root,
                source_repos: &source_repos,
                session_id,
                start,
                // Claude's session-level effort, persisted on the record so it
                // re-applies on every spawn (fresh, resume).
                effort: record.effort.as_deref(),
                model: record.model.as_deref(),
                instructions: instructions.as_deref(),
                mcp_servers: &mcp_servers,
                codegraph_available,
                rpc_dir,
                engine,
                // The account stamped at creation, never the live setting.
                account: record.account.as_deref(),
                oauth_token: oauth_token.as_ref(),
                blackboard: blackboard.as_deref(),
                adopted_read_only,
                folder_grants: &folder_grants,
            };
            let spawned = spawn_managed_agent(
                spec,
                ctx.clone(),
                agent_id_str.clone(),
                self.clone(),
                my_gen,
            );
            if spawned.is_ok() {
                self.launch_succeeded(agent_id);
            }
            spawned
        }
    }

    pub async fn resume_agent(self: Arc<Self>, ctx: Arc<EngineCtx>, agent_id: &str) -> Result<()> {
        if !self.agents.lock().contains_key(agent_id) {
            // Kept for the launch below; a failure surfaces there.
            let _ = self.prefetch_login(agent_id).await;
        }
        let _lifecycle_guard = self.agent_lifecycle.lock().await;
        let record = self.workspace.agent(agent_id)?;
        if self.agents.lock().contains_key(agent_id) {
            return Ok(());
        }
        // Refused here, before the status flips, rather than deep in the
        // launch (`AgentRecord::launch_session`).
        record.launch_session()?;
        self.launch(&ctx, agent_id, None).await
    }

    /// Change a session's reasoning effort mid-conversation. Persists the new
    /// value so every future spawn reads it, then re-applies it to the live
    /// process (see `reapply_session_config`). `None` clears the selection
    /// (provider default).
    pub async fn set_agent_effort(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        effort: Option<&str>,
    ) -> Result<()> {
        self.workspace.update_agent_effort(agent_id, effort)?;
        emit_effort(ctx.sink.as_ref(), agent_id, effort);
        self.reapply_session_config(ctx, agent_id).await
    }

    /// Change a session's model mid-conversation. Persists the new value so
    /// every future spawn reads it, then re-applies it to the live process (see
    /// `reapply_session_config`). `None` clears the selection (provider default).
    pub async fn set_agent_model(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        model: Option<&str>,
    ) -> Result<()> {
        self.workspace.update_agent_model(agent_id, model)?;
        emit_model(ctx.sink.as_ref(), agent_id, model);
        self.reapply_session_config(ctx, agent_id).await
    }

    /// Grant `path` — a folder on this machine — to `agent_id`'s agent, or
    /// change the mode of a folder already granted
    /// (`workspace::folder_grants`). Checked here, on the host, before it is
    /// stored; the stored path is canonical. Engines apply grants at launch,
    /// so when the change is one the launch sees a live agent takes a
    /// session-preserving respawn: at once when idle, at the turn's end when
    /// busy (or sooner, through [`Self::apply_changes_now`]). A follow-up that
    /// names the folder waits for that respawn rather than reaching the old
    /// process (`send_user_message`). Returns the folder as stored, which is
    /// how a client must name it from then on.
    pub async fn grant_folder(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        path: &Path,
        writable: bool,
    ) -> Result<FolderGranted> {
        let record = self.workspace.agent(agent_id)?;
        if record.archive.is_some() {
            return Err(Error::Other("an archived workspace runs no agent".into()));
        }
        let requested = FolderGrant {
            path: path.to_path_buf(),
            writable,
        };
        sandbox::ensure_folders_can_be_shared(
            sandbox::cloud_provider(&record)?,
            std::slice::from_ref(&requested),
        )?;
        let checked = crate::workspace::folder_grants::check_all(
            vec![requested],
            sandbox::stamped_engine(&record),
        )
        .await?;
        let Some(FolderGrant { path, .. }) = checked.into_iter().next() else {
            return Err(Error::Other("no folder to grant".into()));
        };
        let unchanged = record
            .folder_grants
            .iter()
            .any(|g| g.path == path && g.writable == writable);
        let update = if unchanged {
            self.folder_grants_update(agent_id, record.folder_grants)
        } else {
            let grants = self
                .workspace
                .upsert_folder_grant(agent_id, &path, writable)?;
            self.apply_folder_grants(ctx, &record, grants).await?
        };
        Ok(FolderGranted {
            granted: path,
            update,
        })
    }

    /// Take back a folder granted to `agent_id`'s agent, relaunching it as
    /// [`Self::grant_folder`] does when the launch saw the folder. `path` names
    /// the grant as stored (canonical), or any spelling that resolves to it.
    pub async fn revoke_folder(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        path: &Path,
    ) -> Result<FolderGrantsUpdate> {
        let record = self.workspace.agent(agent_id)?;
        let Some(stored) = crate::workspace::folder_grants::find(&record.folder_grants, path)
            .map(|g| g.path.clone())
        else {
            return Ok(self.folder_grants_update(agent_id, record.folder_grants));
        };
        let grants = self.workspace.remove_folder_grant(agent_id, &stored)?;
        self.apply_folder_grants(ctx, &record, grants).await
    }

    /// Make `record`'s just-stored `grants` (it holds the ones before) take
    /// effect, and tell every client. Only a change the launch sees relaunches
    /// the agent (`folder_grants::launch_relevant`): a read-only folder that
    /// seatbelt already lets the agent read needs none, either way.
    async fn apply_folder_grants(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        record: &AgentRecord,
        grants: Vec<FolderGrant>,
    ) -> Result<FolderGrantsUpdate> {
        use crate::workspace::folder_grants::launch_relevant;
        let engine = sandbox::stamped_engine(record);
        let home =
            dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
        let home = home.canonicalize().unwrap_or(home);
        let relaunch = launch_relevant(engine, &record.folder_grants, &home)
            != launch_relevant(engine, &grants, &home);
        // Announced before a respawn that may take a while: the chips are
        // the user's to see at once, whatever the relaunch does.
        let mut update = self.folder_grants_update(&record.id, grants);
        emit_folder_grants(ctx.sink.as_ref(), &record.id, &update);
        if relaunch {
            self.respawn_agent_preserving_session(ctx, &record.id)
                .await?;
            let waiting = self.respawn_pending.lock().contains(&record.id);
            if waiting != update.applies_at_turn_end {
                update.applies_at_turn_end = waiting;
                emit_folder_grants(ctx.sink.as_ref(), &record.id, &update);
            }
        }
        Ok(update)
    }

    /// `grants` as the clients see them: with whether a relaunch held for the
    /// running turn's end is still to apply them.
    fn folder_grants_update(&self, agent_id: &str, grants: Vec<FolderGrant>) -> FolderGrantsUpdate {
        FolderGrantsUpdate {
            folder_grants: grants,
            applies_at_turn_end: self.respawn_pending.lock().contains(agent_id),
        }
    }

    /// End `agent_id`'s running turn now so the session-preserving respawn
    /// waiting for it (a folder grant, a model change) happens at once; the
    /// follow-ups queued behind it — a message naming the new folder — then
    /// reach the new process as the next turn. Unlike [`Self::stop_agent`] the
    /// queue is not held: the user asked for the change to land now, not for
    /// the work to stop. Returns whether a turn was ended.
    ///
    /// Under the lifecycle lock, so the respawn cannot swap the process between
    /// the check and the interrupt: with nothing waiting, or the turn already
    /// over (its respawn queued behind this lock), this does nothing — an
    /// interrupt must never land on the new process, which the queued message
    /// may already be running in.
    pub async fn apply_changes_now(self: &Arc<Self>, agent_id: &str) -> Result<bool> {
        let _lifecycle_guard = self.agent_lifecycle.lock().await;
        // Running only: a process still spawning has no turn to end, and a
        // SIGINT there could kill it outright.
        let running = matches!(self.live_status(agent_id), Some(AgentStatus::Running));
        if !running || !self.respawn_pending.lock().contains(agent_id) {
            return Ok(false);
        }
        let Ok(agent) = self.live_agent(agent_id) else {
            return Ok(false);
        };
        tracing::info!(
            agent_id,
            "ending the turn early to apply a pending relaunch"
        );
        agent.interrupt();
        Ok(true)
    }

    /// Make a just-persisted model/effort change take effect.
    ///
    /// Per-turn agents read model/effort from the session record when each turn
    /// is dispatched (see `deliver_user_message`), so persisting the record is
    /// all that's needed — nothing to do here.
    ///
    /// claude bakes both into its persistent process's launch args
    /// (`--model`/`--effort`), so it needs a session-preserving respawn (eager
    /// when idle, deferred to the next turn boundary when busy). A failed restart
    /// propagates: the new value is persisted, but the caller must not report
    /// success when the agent is left without a running process.
    async fn reapply_session_config(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
    ) -> Result<()> {
        if !is_per_turn_provider(&self.workspace.agent(agent_id)?.provider) {
            self.respawn_agent_preserving_session(ctx, agent_id).await?;
        }
        Ok(())
    }

    /// Respawn every live agent using `provider_id` so it picks up a freshly
    /// changed binary path. The binary is resolved only inside `start_process`
    /// (spawn / resume), so a live agent keeps the old binary —
    /// baked into its running process (claude, persistent) or frozen spawn args
    /// (per-turn) — until torn down and restarted. Callers must refresh the
    /// `bin_resolve` override registry *before* calling this so the restarted
    /// processes resolve the new path.
    ///
    /// Only currently-live agents need this; anything not in the `agents` map
    /// will resolve the new binary on its next spawn anyway.
    pub async fn respawn_provider(self: &Arc<Self>, ctx: &Arc<EngineCtx>, provider_id: &str) {
        // Snapshot ids under a short-lived lock; never hold a guard across the
        // `start_process` await in `respawn_agent_preserving_session` (parking_lot
        // guards aren't Send, and `start_process` re-locks these maps → deadlock).
        let ids: Vec<String> = self.agents.lock().keys().cloned().collect();
        for id in ids {
            match self.workspace.agent(&id) {
                Ok(r) if r.provider == provider_id => {}
                _ => continue, // wrong provider, or removed out from under us
            }
            // Fire-and-forget: a failed restart is logged and reflected in the
            // agent's status inside the call; there's no caller to surface it to.
            let _ = self.respawn_agent_preserving_session(ctx, &id).await;
        }
    }

    /// Tear down and restart one live agent, resuming its existing session
    /// (`fresh = false`) so the transcript/conversation is preserved, and
    /// re-reading the record so any value baked into the launch args (binary
    /// path, `--model`, `--effort`) is picked up. Shared by the binary-swap
    /// respawn (`respawn_provider`) and mid-session config changes
    /// (`set_agent_effort`).
    ///
    /// The idle-check and the `agents` removal happen atomically under a single
    /// `agents` lock: a concurrent send can flip an agent Idle→Running on
    /// another thread (`transition_active` touches only `statuses`), so a
    /// separate check-then-remove would risk shutting down an in-flight turn.
    /// If the agent is mid-turn (Spawning/Running) we leave it running and flag
    /// it in `respawn_pending`; the next turn-end Idle transition retries it
    /// (see `transition_active`). This is what keeps the "change config → keep
    /// going" flow working for an agent that's busy at the time.
    ///
    /// Returns `Err` only when the replacement process fails to start (the agent
    /// is left in `Error` status): the caller that initiated the change (e.g.
    /// `set_agent_effort`) then surfaces it instead of reporting false success.
    /// Every "nothing to restart now" outcome — deferred because busy, or the
    /// agent already gone — is `Ok`: the new record still applies on the next
    /// spawn. Fire-and-forget callers (binary swap, the turn-end drain) ignore
    /// the result and rely on the internal status/logging.
    pub(super) async fn respawn_agent_preserving_session(
        self: &Arc<Self>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
    ) -> Result<()> {
        // Keep the complete idle -> teardown -> Spawning -> restarted
        // transition mutually exclusive with project deletion. The deletion
        // marker is installed before that path waits for this lifecycle lock,
        // so a respawn that won the lock but lost the marker race also stops.
        // The login is resolved first, outside the lock (`prefetch_login`); a
        // failure is kept and surfaces from the launch.
        let _ = self.prefetch_login(agent_id).await;
        let lifecycle_guard = self.agent_lifecycle.lock().await;
        let record = match self.workspace.agent(agent_id) {
            Ok(r) => r,
            Err(_) => {
                self.respawn_pending.lock().remove(agent_id);
                return Ok(());
            }
        };
        if self.deleting_projects.lock().contains(&record.project_id) {
            self.respawn_pending.lock().remove(agent_id);
            return Ok(());
        }
        let agent = match self.take_idle(agent_id, &record) {
            super::login_refresh::Taken::Agent(agent) => agent,
            super::login_refresh::Taken::Busy => {
                self.respawn_pending.lock().insert(agent_id.to_string());
                tracing::info!(agent_id, "session-preserving respawn deferred: agent busy");
                return Ok(());
            }
            // Gone: the next spawn re-reads the record anyway.
            super::login_refresh::Taken::Gone => {
                self.respawn_pending.lock().remove(agent_id);
                return Ok(());
            }
        };
        self.respawn_pending.lock().remove(agent_id);
        if let Err(e) = self.restart_taken(ctx, agent_id, Some(agent)).await {
            tracing::warn!(agent_id, error = %e, "session-preserving respawn failed");
            return Err(e);
        }

        // This respawn passed through a turn-end Idle where the normal queue
        // drain deferred to us (see `drain_message_queue`). Now that the agent
        // is back, deliver any follow-ups queued during that turn — unless the
        // user stopped (A2-A), which we own the interrupt check for here. The
        // restart is done, so release the app-wide lifecycle lock first rather
        // than hold it while this agent's delivery lock is awaited.
        drop(lifecycle_guard);
        if !self.interrupted.lock().remove(agent_id) {
            let _delivering = self.lock_delivery(agent_id).await;
            if let Err(e) = flush_queued(self, ctx, agent_id).await {
                tracing::warn!(agent_id, error = %e, "post-respawn queue flush failed");
            }
        }
        // Whatever was held through the respawn has its turn by now, or has
        // none coming (a stop, a flush that put it back).
        self.settle_turn(ctx, agent_id);
        Ok(())
    }

    pub async fn stop_agent(self: Arc<Self>, ctx: Arc<EngineCtx>, agent_id: &str) -> Result<()> {
        // Interrupt the current turn. How it returns to Idle depends on
        // the runner: claude (managed) emits a `result` event and, if it
        // exits, `apply_exit_if_current` moves it to Idle; codex's
        // per-turn `exec` exits on SIGINT and its `on_turn_exit` handler
        // ends the turn (it emits no `turn.completed` when interrupted).
        // Mark the stop so the turn-end Idle transition keeps the queue intact
        // instead of auto-flushing it (A2-A). Cleared when a new turn starts.
        self.interrupted.lock().insert(agent_id.to_string());
        // Clone the handle out and interrupt with the `agents` lock released
        // (interrupt writes deny responses over stdin, which can block). An
        // agent not in the map is a silent no-op, as before.
        if let Ok(agent) = self.live_agent(agent_id) {
            agent.interrupt();
        }
        // Nothing to interrupt when the turn is still owed rather than
        // running (its message held for an agent at rest): stopping is then
        // the user no longer waiting on it.
        self.settle_turn(&ctx, agent_id);
        Ok(())
    }

    /// End the sessions of `agent_ids` ahead of the user's stop of the machine
    /// they run in (a box, `commands::box_stop_impl`), so that stop is no
    /// crash for any of them. Each generation is retired first: neither the
    /// process's own exit nor the machine going down under it reports an
    /// error, and a launch still on its way into the machine is over wherever
    /// it is (`Launch`). An agent at rest stays idle; one mid-turn ends the
    /// turn as the user's stop of it does, interrupted, its follow-ups held;
    /// one still launching is at rest at once, a message held for the launch
    /// kept with it. In every case the agent is idle with no process when
    /// this returns, so the next message revives the session
    /// (`needs_revive`), and its launch starts the machine again.
    pub fn rest_sessions(&self, ctx: &Arc<EngineCtx>, agent_ids: &[String]) {
        for agent_id in agent_ids {
            self.bump_generation(agent_id);
            // Read under the `agents` lock the process is removed under, as
            // `take_idle` reads it.
            let (agent, mid_turn) = {
                let mut agents = self.agents.lock();
                let agent = agents.remove(agent_id);
                let mid_turn = matches!(self.live_status(agent_id), Some(AgentStatus::Running));
                (agent, mid_turn)
            };
            self.activities.lock().remove(agent_id);
            if let Some(agent) = agent {
                let _ = agent.shutdown();
            }
            // Still Running: an exit handled before the generation moved was
            // no stop's, and the error it reported stands.
            if mid_turn && matches!(self.live_status(agent_id), Some(AgentStatus::Running)) {
                // The turn closes as interrupted (`persist_and_emit_status`
                // reads the flag), and the next turn's start clears it. Not
                // through `transition_active`: its turn-end reads (the PR's
                // state) run in the checkout, in the machine about to go
                // down, and one landing after the stop would start it again.
                self.interrupted.lock().insert(agent_id.clone());
                self.set_status(ctx, agent_id, AgentStatus::Idle, None);
            } else if self.rest_launch(ctx, agent_id) {
                // The turn a held message was owed is not coming from this
                // launch; the message waits for the next one.
                self.settle_turn(ctx, agent_id);
            }
        }
    }
}

/// What a launch ended from outside hands back (`Launch`): not a failure of
/// the agent's, and never recorded as one.
fn launch_ended() -> Error {
    Error::Other("launch ended before its process became ready".into())
}

/// What closes a turn for an agent whose output is a JSON event stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TurnClose {
    /// One persistent process for the whole session (claude managed). The
    /// provider's terminal event is the turn boundary, and the status follows
    /// the stream both ways: main-line output after a terminal event means the
    /// provider opened a turn of its own, so the agent is Running again until
    /// that turn's terminal event.
    OnTerminalEvent,
    /// One process per turn (codex, cursor, opencode, pi). The process exit is
    /// the boundary — `on_turn_exit` ends the turn — and the stream only feeds
    /// the detector so the exit can tell a reported turn end from a crash. The
    /// status never follows the stream here: a line flushed after the exit has
    /// no process behind it to end the turn it would open.
    OnProcessExit,
}

/// Parsed-JSON event callback shared by the managed + per-turn spawners:
/// record activity, move the status when the stream says the turn opened or
/// closed, then emit `agent:event`.
pub(super) fn make_event_handler(
    sup: Arc<Supervisor>,
    ctx: Arc<EngineCtx>,
    agent_id: String,
    close: TurnClose,
) -> impl Fn(Value) + Send + Sync + 'static {
    move |event: Value| {
        let (active, ended) = {
            let mut activities = sup.activities.lock();
            match activities.get_mut(&agent_id) {
                Some(activity) => {
                    let active = activity.observe_event(&event);
                    (active, activity.turn_ended())
                }
                None => (false, false),
            }
        };
        let follows_stream = close == TurnClose::OnTerminalEvent;

        // Background sub-agents keep streaming after the main turn's terminal
        // event, and their lifecycle events are the only trace of them while
        // the agent rests; settle their records rather than wait for the next
        // user turn to ingest them.
        sup.settle_after_idle_event(&ctx, &agent_id, &event);

        // Main-line output while Idle: the provider started a turn Fletch never
        // asked for (claude answering a finished background task). Mark it like
        // any other turn — Running, a `turn:started` anchor for the timer — so
        // every surface reads "working" for exactly as long as output flows.
        // Emitted ahead of the event so no client renders content under an
        // idle status.
        if follows_stream && active && matches!(sup.live_status(&agent_id), Some(AgentStatus::Idle))
        {
            sup.provider_runs
                .lock()
                .insert(agent_id.clone(), chrono::Utc::now().timestamp_millis());
            mark_user_turn_started(&sup, &ctx, &agent_id, None);
        }

        // Keep the turn for a client that missed the stream (`read_live_turn`).
        let seq = sup
            .live_turns
            .lock()
            .entry(agent_id.clone())
            .or_default()
            .push(event.clone());

        emit_agent_event(ctx.sink.as_ref(), &agent_id, event, seq);

        // The terminal event closes the turn here and now — after the event
        // itself, so the transcript holds the result before the status says
        // the agent is done. Every turn-end side effect (queue drain, session
        // sync, PR state) hangs off this one transition.
        if follows_stream
            && ended
            && matches!(sup.live_status(&agent_id), Some(AgentStatus::Running))
        {
            transition_active(&sup, &ctx, &agent_id, AgentStatus::Idle);
        }
    }
}

/// Process-exit callback for the managed spawner: hand the outcome to
/// `apply_exit_if_current`, which ignores exits from a stale generation.
fn make_exit_handler(
    sup: Arc<Supervisor>,
    ctx: Arc<EngineCtx>,
    agent_id: String,
    gen: u64,
) -> impl Fn(crate::managed_session::ManagedExit) + Send + Sync + 'static {
    move |exit| {
        apply_exit_if_current(&sup, &ctx, &agent_id, gen, exit.success, exit.message);
    }
}

fn spawn_managed_agent(
    spec: SpawnSpec<'_>,
    ctx: Arc<EngineCtx>,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    let on_event = make_event_handler(
        sup.clone(),
        ctx.clone(),
        agent_id.clone(),
        TurnClose::OnTerminalEvent,
    );
    // The stream's `rate_limit_event` is the one free, exact reading of an
    // account's limits, and only here is it known which account the stream
    // runs under. The event still flows on to the transcript unchanged.
    let limits_ctx = ctx.clone();
    let account = spec.account.map(str::to_string);
    let login_sup = sup.clone();
    let login_agent = agent_id.clone();
    let on_event = move |event: Value| {
        crate::agent::limits::observe_stream_event(&limits_ctx, account.as_deref(), &event);
        // Read before the event closes the turn: a rejection must flag its
        // respawn ahead of the turn-end drain (`observe_login`). A spent
        // quota is read first, since it is not a login rejection
        // (`observe_limit`).
        let login = match login_sup.observe_limit(&limits_ctx, &login_agent, &event) {
            super::login_refresh::LoginVerdict::Fine => {
                login_sup.observe_login(&login_agent, &event)
            }
            limit => limit,
        };
        on_event(event);
        if login == super::login_refresh::LoginVerdict::GaveUp {
            login_sup.report_login_failure(&limits_ctx, &login_agent);
        }
    };
    Agent::spawn_managed(spec, on_event, make_exit_handler(sup, ctx, agent_id, gen))
}

/// Build a per-turn agent (codex, cursor). Their process exits at the end
/// of *every* turn — that's normal, not the agent dying — so unlike the
/// managed spawner we don't wire `apply_exit_if_current` (which would
/// remove the agent from the map). Instead the per-turn exit is reported
/// via `on_turn_exit`, which ends the turn (Idle) without tearing the
/// agent down. This covers turns that exit without an in-band turn-end
/// event (interrupt, crash) so the agent doesn't sit Running forever. The
/// session-id callback persists the id the agent assigns on its first turn so
/// later turns (and re-attach after restart)
/// resume it.
fn spawn_per_turn_agent(
    provider: &str,
    spec: PerTurnSpec,
    ctx: Arc<EngineCtx>,
    agent_id: String,
    sup: Arc<Supervisor>,
    gen: u64,
) -> Result<Agent> {
    let id_for_sid = agent_id.clone();
    let sup_for_sid = sup.clone();
    let ctx_for_exit = ctx.clone();
    let id_for_exit = agent_id.clone();
    let sup_for_exit = sup.clone();

    let limit_sup = sup.clone();
    let limit_ctx = ctx.clone();
    let limit_agent = agent_id.clone();
    let on_event = make_event_handler(sup, ctx, agent_id, TurnClose::OnProcessExit);
    // A `turn.failed` on a spent quota flags its retry under the active
    // account before the process exit closes the turn and drains the
    // respawn (`observe_limit`).
    let on_event = move |event: Value| {
        limit_sup.observe_limit(&limit_ctx, &limit_agent, &event);
        on_event(event);
    };
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
        // End the turn.
        // User Stop is an expected non-zero exit, and so is a turn the CLI
        // already reported as failed in-band (codex emits `turn.failed` for a
        // usage limit / auth / API error, then exits 1): the error is in the
        // transcript, and the agent is resumable like claude after a failed
        // `result`. Only a non-interrupted failure with no turn-end event —
        // the CLI died before saying anything — is a real crash. What it did
        // say is read here, before the crash path drops the activity: opencode
        // reports its failure as a non-terminal `error` event and then exits 1
        // with an empty stderr, so the exit status alone would lose the reason.
        let (turn_ended_in_band, reported) = sup_for_exit
            .activities
            .lock()
            .get(&id_for_exit)
            .map(|a| (a.turn_ended(), a.turn_failure().map(str::to_string)))
            .unwrap_or((false, None));
        if exit.success || exit.interrupted || turn_ended_in_band {
            transition_active(
                &sup_for_exit,
                &ctx_for_exit,
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
            sup_for_exit.trigger_session_sync(ctx_for_exit.clone(), id_for_exit.clone());
            sup_for_exit.set_status(
                &ctx_for_exit,
                &id_for_exit,
                AgentStatus::Error,
                Some(format!(
                    "Agent process exited: {}",
                    super::turn_error::exit_message_with_report(&exit.message, reported.as_deref())
                )),
            );
        }
    };

    let desc = per_turn_descriptor(provider)
        .ok_or_else(|| Error::Other(format!("unknown per-turn agent provider: {provider}")))?;
    Agent::spawn_per_turn(desc, spec, on_event, on_session_id, on_turn_exit)
}

/// Fail `launch` if its process is still not up after [`SPAWN_TIMEOUT`].
/// Armed where the process is started, never over the provisioning before it.
/// It times that one attempt: ended or settled meanwhile, the timeout finds
/// nothing to fail, whatever the agent is doing by then.
pub(super) fn arm_spawn_timeout(sup: Arc<Supervisor>, ctx: Arc<EngineCtx>, launch: Launch) {
    crate::host::spawn(async move {
        let agent_id = launch.agent_id();
        // An agent may first have to wait for the engine's setup (a box's
        // image download and tools disk build), far longer than any process
        // start; the box reports that wait as its own phase meanwhile.
        let setup = sup
            .workspace
            .agent(agent_id)
            .ok()
            .map_or(Duration::ZERO, |record| sandbox::setup_allowance(&record));
        let timeout = SPAWN_TIMEOUT + setup;
        tokio::time::sleep(timeout).await;
        spawn_timed_out(&sup, &ctx, &launch, timeout);
    });
}

/// `launch`'s spawn timeout is up after `timeout`: fail it if it is still
/// unsettled, and take down whatever process it had started.
fn spawn_timed_out(sup: &Supervisor, ctx: &Arc<EngineCtx>, launch: &Launch, timeout: Duration) {
    // Settling retires the launch's generation with the status, so the
    // process this attempt may be inserting concurrently is either not kept
    // by `start_process` or taken down below, and nothing it still prints or
    // exits with is read as the agent's.
    let err = format!(
        "Spawn timed out after {}s — process did not become ready.",
        timeout.as_secs()
    );
    if !sup.settle_launch(ctx, launch, AgentStatus::Error, Some(err)) {
        return;
    }
    let agent_id = launch.agent_id();
    let taken = sup.agents.lock().remove(agent_id);
    if let Some(agent) = taken {
        let _ = agent.shutdown();
    }
    sup.activities.lock().remove(agent_id);
}

/// `launch` failed with `err`: its agent is left in `Error` with the reason,
/// unless the launch was ended from outside first (`Launch`), whose failure
/// is nobody's to see.
pub(super) fn fail_spawn(sup: &Supervisor, ctx: &Arc<EngineCtx>, launch: &Launch, err: String) {
    if !sup.settle_launch(ctx, launch, AgentStatus::Error, Some(err.clone())) {
        tracing::info!(
            agent_id = launch.agent_id(),
            error = %err,
            "a launch that was already over ended; nothing reported"
        );
    }
}

fn apply_exit_if_current(
    sup: &Supervisor,
    ctx: &Arc<EngineCtx>,
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
    // resumable). Invalidate this generation so the gen-guarded RPC watcher
    // stops polling instead of spinning for the app's lifetime.
    sup.bump_generation(agent_id);

    // Bind the removed handle so the `agents` guard drops before the
    // (last-`Arc`) session teardown runs its child kill/reap off the lock.
    let taken = sup.agents.lock().remove(agent_id);
    drop(taken);
    sup.activities.lock().remove(agent_id);

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
        sup.set_status(ctx, agent_id, status.clone(), err);
        if matches!(status, AgentStatus::Idle) {
            sup.fetch_and_emit_pr_state(ctx.clone(), agent_id.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{NativeTranscript, TurnOutcome};

    /// An Idle supervisor holding one agent whose last turn ended, plus the
    /// event handler that would receive its stream.
    struct RestingAgent {
        sup: Arc<Supervisor>,
        agent_id: String,
        sink: Arc<crate::host::sink::RecordingSink>,
        on_event: Box<dyn Fn(Value)>,
        /// `host::spawn` falls back to `tokio::spawn` before boot, so the handler
        /// needs a runtime context for as long as it is called.
        _runtime: tokio::runtime::Runtime,
        /// Dropping it would delete the context's database.
        _dir: tempfile::TempDir,
    }

    fn resting_agent(close: TurnClose) -> RestingAgent {
        let db = crate::workspace::tests::test_db();
        let (agent_id, wm) = crate::workspace::tests::make_workspace_with_session(&db);
        let sup = Arc::new(Supervisor::new(Arc::new(wm)));
        let (ctx, sink, dir) = crate::host::ctx::test_ctx();
        sup.statuses
            .lock()
            .insert(agent_id.clone(), AgentStatus::Idle);
        let mut activity = ManagedActivity::claude();
        activity.observe_event(&serde_json::json!({"type": "result", "subtype": "success"}));
        sup.activities
            .lock()
            .insert(agent_id.clone(), Box::new(activity));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let on_event = Box::new(make_event_handler(
            sup.clone(),
            ctx,
            agent_id.clone(),
            close,
        ));
        RestingAgent {
            sup,
            agent_id,
            sink,
            on_event,
            _runtime: runtime,
            _dir: dir,
        }
    }

    /// Claude ends the user's turn while a background sub-agent is still
    /// running, then starts a turn of its own to answer the task's notification.
    /// Nothing Fletch delivered opened that turn, so the status has to follow
    /// the stream: Running from its first main-line event, Idle at its `result`.
    #[test]
    fn a_turn_the_provider_starts_on_its_own_moves_the_status_with_the_stream() {
        use serde_json::json;
        let RestingAgent {
            sup,
            agent_id,
            sink,
            on_event,
            _runtime,
            _dir,
        } = resting_agent(TurnClose::OnTerminalEvent);
        let _enter = _runtime.enter();

        // Background-task chatter between turns is not a turn.
        on_event(json!({"type": "system", "subtype": "task_notification", "task_id": "abc"}));
        assert_eq!(sup.live_status(&agent_id), Some(AgentStatus::Idle));

        // The reply turn's first main-line event: Running, with a timer anchor,
        // announced ahead of the event so no client renders it under `idle`.
        on_event(json!({
            "type": "user",
            "message": {"role": "user", "content": "<task-notification>done</task-notification>"}
        }));
        assert_eq!(sup.live_status(&agent_id), Some(AgentStatus::Running));
        let events = sink.events();
        assert!(events.iter().any(|(n, _)| n == "turn:started"));
        let status_at = events
            .iter()
            .position(|(n, _)| n == "agent:status")
            .unwrap();
        let user_event_at = events
            .iter()
            .position(|(n, p)| n == "agent:event" && p["event"]["type"] == "user")
            .unwrap();
        assert!(status_at < user_event_at, "{events:?}");

        on_event(json!({"type": "assistant", "message": {"role": "assistant", "content": []}}));
        assert_eq!(sup.live_status(&agent_id), Some(AgentStatus::Running));

        // Its `result` closes the turn — after the event itself, so the
        // transcript holds the result before the status says the agent is done.
        on_event(json!({"type": "result", "subtype": "success"}));
        assert_eq!(sup.live_status(&agent_id), Some(AgentStatus::Idle));
        let events = sink.events();
        let last_status = events
            .iter()
            .rposition(|(n, _)| n == "agent:status")
            .unwrap();
        let last_event = events
            .iter()
            .rposition(|(n, _)| n == "agent:event")
            .unwrap();
        assert!(last_event < last_status);
        assert_eq!(events[last_status].1["status"], "idle");
    }

    /// That turn is more of the user's turn's work, not a turn of its own: its
    /// time lands on the turn it followed, which keeps its own `ended_at`.
    #[test]
    fn a_turn_the_provider_starts_on_its_own_counts_toward_the_turn_it_followed() {
        use serde_json::json;
        let RestingAgent {
            sup,
            agent_id,
            sink: _sink,
            on_event,
            _runtime,
            _dir,
        } = resting_agent(TurnClose::OnTerminalEvent);
        let _enter = _runtime.enter();
        sup.workspace
            .insert_user_turn(&agent_id, "t1", "go", &[])
            .unwrap();
        sup.workspace.mark_user_turn_started("t1", 1000).unwrap();
        sup.workspace
            .mark_user_turn_ended(&agent_id, TurnOutcome::Completed)
            .unwrap();
        let closed = sup.workspace.read_history_turns(&agent_id).unwrap()[0].ended_at;

        on_event(json!({
            "type": "user",
            "message": {"role": "user", "content": "<task-notification>done</task-notification>"}
        }));
        // Back-date the run so its time is measurable, whatever the clock's grain.
        *sup.provider_runs.lock().get_mut(&agent_id).unwrap() -= 5000;
        on_event(json!({"type": "result", "subtype": "success"}));

        let turn = &sup.workspace.read_history_turns(&agent_id).unwrap()[0];
        assert!(turn.provider_run_ms >= 5000, "{}", turn.provider_run_ms);
        assert_eq!(turn.ended_at, closed);
        assert!(sup.provider_runs.lock().is_empty());
    }

    /// An agent stopped mid-run takes the run's clock down with it: time spent
    /// down must not count when it next comes to rest.
    #[test]
    fn stopping_mid_run_drops_the_providers_clock() {
        use serde_json::json;
        let RestingAgent {
            sup,
            agent_id,
            sink: _sink,
            on_event,
            _runtime,
            _dir,
        } = resting_agent(TurnClose::OnTerminalEvent);
        let _enter = _runtime.enter();
        on_event(json!({
            "type": "user",
            "message": {"role": "user", "content": "<task-notification>done</task-notification>"}
        }));
        assert!(sup.provider_runs.lock().contains_key(&agent_id));

        let (ctx, _ctx_sink, _ctx_dir) = crate::host::ctx::test_ctx();
        sup.persist_and_emit_status(&ctx, &agent_id, AgentStatus::Stopped, None);
        assert!(sup.provider_runs.lock().is_empty());
    }

    /// A per-turn provider's process exit is its turn boundary (`on_turn_exit`).
    /// Its stream only feeds the detector: a line the reader flushes after the
    /// exit must not reopen a turn that no process is left to end, and its
    /// terminal event must not pre-empt the exit.
    #[test]
    fn a_per_turn_stream_never_moves_the_status() {
        use serde_json::json;
        let RestingAgent {
            sup,
            agent_id,
            sink,
            on_event,
            _runtime,
            _dir,
        } = resting_agent(TurnClose::OnProcessExit);
        let _enter = _runtime.enter();

        on_event(json!({"type": "item.completed", "item": {"type": "agent_message"}}));
        assert_eq!(sup.live_status(&agent_id), Some(AgentStatus::Idle));

        sup.statuses
            .lock()
            .insert(agent_id.clone(), AgentStatus::Running);
        on_event(json!({"type": "turn.completed", "usage": {}}));
        assert_eq!(sup.live_status(&agent_id), Some(AgentStatus::Running));
        assert!(sink.events().iter().all(|(n, _)| n == "agent:event"));
    }

    /// A timed-out spawn's task outlives the timeout, and the user may have
    /// resumed the agent before that task gets round to cleaning up. Resume
    /// reuses the very checkout the task is about to remove, so the discard
    /// has to look at who owns the paths now: a live agent keeps them, a dead
    /// attempt still clears up after itself.
    #[tokio::test]
    async fn an_abandoned_spawn_only_discards_paths_nobody_took_back() {
        let sup = crate::supervisor::tests::test_supervisor();
        let td = tempfile::tempdir().unwrap();
        // Agent dirs live under a checkouts root; tombstones go to its sibling
        // discard root, so both stay inside the tempdir.
        let root = td.path().join("workspaces");

        let resumed = root.join("resumed");
        std::fs::create_dir_all(&resumed).unwrap();
        sup.statuses
            .lock()
            .insert("resumed".into(), AgentStatus::Spawning);
        discard_if_still_dead(&sup, "resumed", &resumed).await;
        assert!(
            resumed.is_dir(),
            "a newer attempt owns these paths; the stale task must not delete them"
        );

        let failed = root.join("failed");
        std::fs::create_dir_all(failed.join("checkout")).unwrap();
        sup.statuses
            .lock()
            .insert("failed".into(), AgentStatus::Error);
        discard_if_still_dead(&sup, "failed", &failed).await;
        assert!(
            !failed.exists(),
            "nobody took the agent back, so the failed spawn's dir goes"
        );
        // The tree is set aside under the lock and deleted after it; neither
        // the dir nor its tombstone may survive.
        let leftovers: Vec<_> = std::fs::read_dir(crate::workspace::discard_root(&root))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(leftovers.is_empty(), "tombstone left behind: {leftovers:?}");

        // A dir the attempt never created is nothing to clean up, and not an
        // error either.
        discard_if_still_dead(&sup, "failed", &root.join("never-made")).await;
    }

    /// A `claude` and a `codex` agent on one repo, both with the checkout
    /// adopted at `<td>/ws/repo`.
    fn fixture(td: &Path) -> Supervisor {
        let sup = crate::supervisor::tests::test_supervisor();
        let repo = td.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        sup.workspace.add_workspace_repo(repo.clone()).unwrap();
        let tracked = TrackedRepo {
            repo_path: repo,
            subdir: "repo".into(),
            branch: None,
            parent_branch: None,
            pr_base: None,
            base_sha: None,
            pr_number: None,
            pr_url: None,
            pr_title: None,
            pr_state: None,
            label: None,
            adopted_checkout: Some(td.join("ws").join("repo")),
        };
        for provider in ["claude", "codex"] {
            let mut record = new_agent_record(
                provider.into(),
                provider.into(),
                provider.into(),
                tracked.clone(),
                "task".into(),
                AgentView::Custom,
            );
            sup.workspace.add_agent(&mut record).unwrap();
        }
        sup
    }

    /// Where the claude agent's current session writes its transcript: the
    /// container-era per-agent dir beside the checkout, which the locator checks
    /// first (the session's random id keeps the real `~/.claude` scan from
    /// ever matching).
    fn transcript_of(sup: &Supervisor, td: &Path) -> PathBuf {
        let own = sup.workspace.agent("claude").unwrap().session_id.unwrap();
        let slug = td
            .join("ws")
            .join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME)
            .join("slug");
        std::fs::create_dir_all(&slug).unwrap();
        slug.join(format!("{own}.jsonl"))
    }

    /// Start a new session in `agent_id` continuing the end of its
    /// conversation, as the provider session `native` was written as when
    /// given.
    fn restart(sup: &Supervisor, agent_id: &str, native: Option<&NativeTranscript>) {
        let lineage = sup
            .workspace
            .resolve_anchor(agent_id, crate::workspace::Anchor::End)
            .unwrap();
        sup.start_session(agent_id, &lineage, native).unwrap();
    }

    fn start(sup: &Supervisor, td: &Path) -> SessionStart {
        let cwd = td.join("ws").join("repo");
        session_start(&sup.workspace.agent("claude").unwrap(), &cwd)
    }

    const MESSAGE: &str = "{\"type\":\"user\",\"uuid\":\"u1\"}\n";

    /// Whether a launch resumes is asked of the session, not the workspace or
    /// the caller: one started in place has nothing to resume until its own
    /// first message lands, though the workspace has long had a task.
    #[test]
    fn a_session_launches_fresh_until_its_transcript_holds_a_message() {
        let td = tempfile::tempdir().unwrap();
        let sup = fixture(td.path());
        std::fs::write(transcript_of(&sup, td.path()), MESSAGE).unwrap();
        assert_eq!(start(&sup, td.path()), SessionStart::Resume);

        restart(&sup, "claude", None);
        assert_eq!(
            start(&sup, td.path()),
            SessionStart::Fresh,
            "the new session has no transcript yet"
        );
        let transcript = transcript_of(&sup, td.path());
        std::fs::write(&transcript, "{\"type\":\"mode\",\"mode\":\"normal\"}\n").unwrap();
        assert_eq!(
            start(&sup, td.path()),
            SessionStart::Fresh,
            "metadata alone can't be resumed"
        );
        std::fs::write(&transcript, MESSAGE).unwrap();
        assert_eq!(start(&sup, td.path()), SessionStart::Resume);
    }

    /// A session whose transcript Fletch wrote for it resumes from its very
    /// first launch: that is how the CLI picks the conversation up.
    #[test]
    fn a_session_with_a_written_transcript_resumes_from_its_first_launch() {
        let td = tempfile::tempdir().unwrap();
        let sup = fixture(td.path());
        let native = NativeTranscript {
            provider_session_id: uuid::Uuid::new_v4().to_string(),
            prefix: 1,
        };
        restart(&sup, "claude", Some(&native));
        std::fs::write(transcript_of(&sup, td.path()), MESSAGE).unwrap();

        assert_eq!(
            sup.workspace.agent("claude").unwrap().session_id,
            Some(native.provider_session_id)
        );
        assert_eq!(start(&sup, td.path()), SessionStart::Resume);
        // A per-turn agent resumes by its session id on every turn instead.
        let codex = sup.workspace.agent("codex").unwrap();
        assert_eq!(
            session_start(&codex, &td.path().join("ws").join("repo")),
            SessionStart::Resume,
            "it has had a first prompt"
        );
    }

    /// A resume whose launch is refused ends in `Error` with the refusal,
    /// not left `Spawning` for the watchdog to report as a timeout.
    #[tokio::test]
    async fn a_failed_resume_leaves_the_agent_in_error_with_the_reason() {
        let td = tempfile::tempdir().unwrap();
        let sup = Arc::new(fixture(td.path()));
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        // A move under way refuses the launch (`settle_before_acting`).
        sup.moving.lock().insert("codex".into());

        let err = sup
            .clone()
            .resume_agent(ctx, "codex")
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("moving"), "{err}");
        assert_eq!(sup.status_of("codex"), Some(AgentStatus::Error));
        assert_eq!(sup.workspace.agent("codex").unwrap().last_error, Some(err));
    }

    /// A launch makes only the checkouts that are the agent's own to make: a
    /// tree it adopted belongs to whoever made it, there or not.
    #[tokio::test]
    async fn a_launch_leaves_an_adopted_checkout_to_its_owner() {
        let td = tempfile::tempdir().unwrap();
        let sup = Arc::new(fixture(td.path()));
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        // A second repo of the same project (projects go by the repo dir's
        // name), which a launch that made the primary would attach: nothing
        // was made here, so nothing is attached either.
        let other = td.path().join("second").join("repo");
        std::fs::create_dir_all(other.join(".git")).unwrap();
        sup.workspace.add_workspace_repo(other.clone()).unwrap();
        let record = sup.workspace.agent("codex").unwrap();
        assert!(sup
            .workspace
            .project_repo_paths(&record.project_id)
            .contains(&other));

        sup.ensure_checkouts(&ctx, "codex").await.unwrap();

        assert!(!td.path().join("ws").exists());
        assert_eq!(sup.workspace.agent("codex").unwrap().repos.len(), 1);
    }

    /// What a launch finishing an unfinished spawn still has to attach: the
    /// project's repos the record has no checkout of, in the project's order
    /// — the primary and anything attached before the failure left out.
    #[test]
    fn a_finished_spawn_has_every_repo_of_its_project() {
        let td = tempfile::tempdir().unwrap();
        let sup = fixture(td.path());
        let record = sup.workspace.agent("codex").unwrap();
        let primary = record.repos[0].repo_path.clone();
        let (docs, api) = (td.path().join("docs"), td.path().join("api"));

        assert_eq!(
            unattached_repos(&record, vec![primary.clone(), docs.clone(), api.clone()]),
            vec![docs.clone(), api.clone()]
        );

        let mut attached = record.clone();
        attached.repos.push(TrackedRepo {
            repo_path: docs.clone(),
            subdir: "docs".into(),
            ..record.repos[0].clone()
        });
        assert_eq!(
            unattached_repos(&attached, vec![primary.clone(), docs, api.clone()]),
            vec![api]
        );
        assert!(unattached_repos(&record, vec![primary]).is_empty());
    }

    // ── the user's stop of a box ──────────────────────────────────────────

    /// A supervisor that knows `ids`, each at rest, and that the context
    /// knows, as it does once the engine has booted.
    fn agents_at_rest(ctx: &Arc<EngineCtx>, ids: &[&str]) -> Arc<Supervisor> {
        let sup = Arc::new(crate::supervisor::tests::test_supervisor());
        ctx.set_supervisor(sup.clone());
        for id in ids {
            let mut record = crate::supervisor::tests::record_with_status(id, AgentStatus::Idle);
            sup.workspace.add_agent(&mut record).unwrap();
            sup.statuses
                .lock()
                .insert(id.to_string(), AgentStatus::Idle);
        }
        sup
    }

    /// `agent_id`'s session in a box as the host holds it: a live process, and
    /// the exit handler of the launch that started it.
    struct BoxSession(Box<dyn Fn(crate::managed_session::ManagedExit)>);

    impl BoxSession {
        /// The machine goes down under the session: its ssh exits 255, and
        /// the exit reaches the supervisor's handler as a launch's does.
        fn machine_down(&self) {
            (self.0)(crate::managed_session::ManagedExit {
                success: false,
                message: "exit status: 255".into(),
            });
        }
    }

    fn session_in(
        sup: &Arc<Supervisor>,
        ctx: &Arc<EngineCtx>,
        agent_id: &str,
        machine: &Path,
    ) -> BoxSession {
        let gen = {
            let mut generations = sup.generations.lock();
            let gen = generations.entry(agent_id.to_string()).or_insert(0);
            *gen += 1;
            *gen
        };
        sup.agents.lock().insert(
            agent_id.to_string(),
            Arc::new(Agent::idle_process(machine, None)),
        );
        BoxSession(Box::new(make_exit_handler(
            sup.clone(),
            ctx.clone(),
            agent_id.to_string(),
            gen,
        )))
    }

    /// `agent_id` rests as if its session had gone to rest on its own: idle,
    /// no error recorded or announced, and no process left, so the next
    /// message revives it (`needs_revive`).
    fn assert_at_rest(sup: &Supervisor, sink: &crate::host::sink::RecordingSink, agent_id: &str) {
        assert_eq!(sup.status_of(agent_id), Some(AgentStatus::Idle));
        assert_eq!(sup.workspace.agent(agent_id).unwrap().last_error, None);
        let errors: Vec<_> = sink
            .events()
            .into_iter()
            .filter(|(name, p)| {
                name == "agent:status" && p["agent_id"] == agent_id && p["status"] == "error"
            })
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
        assert!(!sup.agents.lock().contains_key(agent_id));
    }

    /// What the user's stop of a box does to the sessions in it, once the
    /// registry has the box (`commands::box_stop_impl`).
    fn stop_box_of(sup: &Supervisor, ctx: &Arc<EngineCtx>, sessions: &[&str]) {
        let sessions: Vec<String> = sessions.iter().map(|id| id.to_string()).collect();
        sup.rest_sessions(ctx, &sessions);
    }

    /// The user stops a box whose session is idle: the machine going down
    /// under it afterwards is no crash.
    #[tokio::test]
    async fn an_idle_session_rests_when_the_user_stops_its_box() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let id = "box-stop-idle";
        let sup = agents_at_rest(&ctx, &[id]);
        let machine = tempfile::tempdir().unwrap();
        let session = session_in(&sup, &ctx, id, machine.path());

        stop_box_of(&sup, &ctx, &[id]);
        session.machine_down();

        assert_at_rest(&sup, &sink, id);
    }

    /// A turn running when the user stops its box ends as the user's stop of
    /// the turn ends one: interrupted, not failed.
    #[tokio::test]
    async fn a_turn_in_progress_ends_interrupted_when_the_user_stops_its_box() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let id = "box-stop-turn";
        let sup = agents_at_rest(&ctx, &[id]);
        let machine = tempfile::tempdir().unwrap();
        let session = session_in(&sup, &ctx, id, machine.path());
        sup.workspace
            .insert_user_turn(id, "t1", "fix the build", &[])
            .unwrap();
        mark_user_turn_started(&sup, &ctx, id, Some("t1"));

        stop_box_of(&sup, &ctx, &[id]);
        session.machine_down();

        assert_at_rest(&sup, &sink, id);
        let turns = sup.workspace.read_history_turns(id).unwrap();
        assert_eq!(turns[0].outcome.as_deref(), Some("interrupted"));
        assert_eq!(sup.turn_since(id), None);
    }

    /// A tenant's session runs in its host's box, and rests with it as the
    /// owner's does.
    #[tokio::test]
    async fn a_tenants_session_rests_with_its_hosts_box() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let (host, tenant) = ("box-stop-host", "box-stop-tenant");
        let sup = agents_at_rest(&ctx, &[host, tenant]);
        let machine = tempfile::tempdir().unwrap();
        let host_session = session_in(&sup, &ctx, host, machine.path());
        let tenant_session = session_in(&sup, &ctx, tenant, machine.path());

        stop_box_of(&sup, &ctx, &[host, tenant]);
        host_session.machine_down();
        tenant_session.machine_down();

        assert_at_rest(&sup, &sink, host);
        assert_at_rest(&sup, &sink, tenant);
    }

    /// `box_stop` for an agent that has no running box stops nothing: its
    /// session, wherever it runs, goes on, and an exit of it afterwards is
    /// the crash it would have been.
    #[tokio::test]
    async fn a_box_stop_for_an_agent_with_no_box_leaves_its_session_running() {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let id = "box-stop-no-box";
        let sup = agents_at_rest(&ctx, &[id]);
        let machine = tempfile::tempdir().unwrap();
        let session = session_in(&sup, &ctx, id, machine.path());
        let generation = sup.generation(id);

        let outcome = crate::commands::box_stop_impl(&ctx, id).await.unwrap();

        assert_eq!(outcome, "alreadyexited");
        assert!(sup.agents.lock().contains_key(id));
        assert_eq!(sup.generation(id), generation);
        assert_eq!(sup.status_of(id), Some(AgentStatus::Idle));

        session.machine_down();
        assert_eq!(sup.status_of(id), Some(AgentStatus::Error));
        assert_eq!(
            sup.workspace.agent(id).unwrap().last_error.as_deref(),
            Some("Agent process exited: exit status: 255")
        );
    }

    /// The user stops a box while a launch is still on its way into it, no
    /// process registered yet: the agent is at rest at once, and the launch
    /// failing against the machine going down afterwards is no failure of
    /// the agent's.
    #[tokio::test]
    async fn a_launch_met_by_its_boxs_stop_rests_instead_of_failing() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let id = "box-stop-launching";
        let sup = agents_at_rest(&ctx, &[id]);
        let launch = sup.begin_launch(&ctx, id);

        stop_box_of(&sup, &ctx, &[id]);
        assert_at_rest(&sup, &sink, id);

        fail_spawn(&sup, &ctx, &launch, "ssh: connection closed".into());
        assert_at_rest(&sup, &sink, id);
    }

    /// A message sent after that stop, before the stopped launch has come
    /// back, finds the agent at rest and revives it: nothing waits on the
    /// launch that was ended. A message held for that launch is kept, and
    /// its turn is no longer owed until the next launch delivers it.
    #[tokio::test]
    async fn a_message_after_a_stop_during_launch_revives_the_session() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let id = "box-stop-then-message";
        let sup = agents_at_rest(&ctx, &[id]);
        let launch = sup.begin_launch(&ctx, id);
        sup.open_turn(&ctx, id);
        sup.message_queue.lock().enqueue(
            id,
            crate::message_queue::PendingMsg {
                turn_id: "t-held".into(),
                text: "held for the launch".into(),
                attachments: Vec::new(),
            },
        );
        assert!(!sup.needs_revive(id));

        stop_box_of(&sup, &ctx, &[id]);

        assert!(sup.needs_revive(id));
        assert_eq!(sup.turn_since(id), None);
        assert!(!sup.message_queue.lock().is_empty(id));

        // The revive's launch is the agent's attempt now; the stopped one
        // coming back, either way, changes nothing about it.
        let revived = sup.begin_launch(&ctx, id);
        fail_spawn(&sup, &ctx, &launch, "ssh: connection closed".into());
        assert!(!sup.settle_launch(&ctx, &launch, AgentStatus::Idle, None));
        assert!(sup.launch_is_live(&revived));
        assert_eq!(sup.status_of(id), Some(AgentStatus::Spawning));
        assert!(sup.settle_launch(&ctx, &revived, AgentStatus::Idle, None));
        assert_at_rest(&sup, &sink, id);
    }

    /// The spawn timeout of a launch a stop ended finds nothing to fail,
    /// whether it is up before the launch comes back or after.
    #[tokio::test]
    async fn the_spawn_timeout_of_a_stopped_launch_reports_nothing() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let id = "box-stop-timeout";
        let sup = agents_at_rest(&ctx, &[id]);
        let launch = sup.begin_launch(&ctx, id);

        stop_box_of(&sup, &ctx, &[id]);
        spawn_timed_out(&sup, &ctx, &launch, SPAWN_TIMEOUT);

        assert_at_rest(&sup, &sink, id);
        fail_spawn(&sup, &ctx, &launch, "ssh: connection closed".into());
        assert_at_rest(&sup, &sink, id);
    }

    /// A spawn timeout times the launch it was armed for: it never fails a
    /// later launch of the same agent, and still fails its own.
    #[tokio::test]
    async fn a_spawn_timeout_fails_only_its_own_launch() {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let id = "spawn-timeout-own";
        let sup = agents_at_rest(&ctx, &[id]);
        let first = sup.begin_launch(&ctx, id);
        fail_spawn(&sup, &ctx, &first, "no such binary".into());

        // Resumed before the first launch's timeout is up.
        let second = sup.begin_launch(&ctx, id);
        spawn_timed_out(&sup, &ctx, &first, SPAWN_TIMEOUT);
        assert!(sup.launch_is_live(&second));

        spawn_timed_out(&sup, &ctx, &second, SPAWN_TIMEOUT);
        assert_eq!(sup.status_of(id), Some(AgentStatus::Error));
        let error = sup.workspace.agent(id).unwrap().last_error.unwrap();
        assert!(error.starts_with("Spawn timed out"), "{error}");
    }

    /// The same stop, when the launch's process came up and the machine went
    /// down under it before the launch had registered it.
    #[tokio::test]
    async fn a_process_still_launching_rests_when_its_box_stops_under_it() {
        let (ctx, sink, _dir) = crate::host::ctx::test_ctx();
        let id = "box-stop-launched";
        let sup = agents_at_rest(&ctx, &[id]);
        sup.begin_launch(&ctx, id);
        let machine = tempfile::tempdir().unwrap();
        let session = session_in(&sup, &ctx, id, machine.path());

        stop_box_of(&sup, &ctx, &[id]);
        session.machine_down();

        assert_at_rest(&sup, &sink, id);
    }

    /// A stop ends the one launch it met: the next launch's failure is
    /// reported.
    #[tokio::test]
    async fn a_stop_does_not_excuse_the_next_launchs_failure() {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        let id = "box-stop-relaunch";
        let sup = agents_at_rest(&ctx, &[id]);
        sup.begin_launch(&ctx, id);
        stop_box_of(&sup, &ctx, &[id]);

        let next = sup.begin_launch(&ctx, id);
        fail_spawn(&sup, &ctx, &next, "no such binary".into());

        assert_eq!(sup.status_of(id), Some(AgentStatus::Error));
        assert_eq!(
            sup.workspace.agent(id).unwrap().last_error.as_deref(),
            Some("no such binary")
        );
    }
}
