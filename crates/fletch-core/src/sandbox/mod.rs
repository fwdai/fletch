pub mod boxes;
pub mod cargo_home;
mod engine;
pub mod guarantees;
pub mod placement;
pub mod policy;
pub mod provider;
pub mod provision;
mod seatbelt;
mod seed;
pub mod toolchain_cache;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::RwLock;

use crate::error::{Error, Result};
use crate::workspace::AgentRecord;

// The name in an agent's writable root where a confined box tenant keeps its
// private copies of what it shares with its host: Fletch's own, which no
// checkout may take (`workspace::allocate_repo_subdir`).
pub use boxes::run::GuestRun;
pub(crate) use boxes::COW_DIR;
pub use engine::{
    AgentLaunchCtx, EngineCaps, EngineKind, EnvDelivery, KillGuard, KillHandle, LaunchPlan,
    LauncherArgs, SandboxEngine, StageArgs, TurnPrep,
};
pub use guarantees::{describe as describe_isolation, IsolationReport};
pub(crate) use seatbelt::pid_alive;
pub use seatbelt::{
    build_run_profile, cleanup_nested_checkouts_roots, cleanup_nested_rpc_roots,
    nested_checkouts_root, nested_rpc_root, profile_args, SANDBOX_EXEC,
};
pub use toolchain_cache::{
    reclaim_toolchain_caches, remove_project_cache, toolchain_cache_dir, toolchain_cache_root,
    ToolchainCache, TOOLCHAIN_CACHE_ROOT_ENV,
};

/// The engine an agent that *adopts* `tree` — works in an existing checkout
/// instead of provisioning one (the local reviewer in a coder's checkout) —
/// is stamped with: **where the tree lives**. A tree inside a box is
/// reachable only from that box, so the agent runs there as its tenant; a
/// host tree is invisible to a guest, so it lands on seatbelt. The caller
/// still gates the provider against the result.
pub fn engine_for_adopted_tree(tree: &Path) -> EngineKind {
    if boxes::box_holding(tree).is_some() {
        return EngineKind::Box;
    }
    EngineKind::SandboxExec
}

/// This engine's own data directory ([`crate::host::BootConfig::data_dir`]),
/// mirrored process-wide so the macOS profile builder — reached on the spawn
/// path, with no handle on the boot config — can carve it out of what a confined
/// agent may read or write.
///
/// It cannot be derived. [`crate::data_dir`] is the *desktop's* directory, keyed
/// by [`crate::BUNDLE_ID`]; a `fletch-host` keeps its database (the GitHub token
/// in plaintext, since a service has no unlocked keychain), its Noise
/// `remote/host_key` and its `remote/devices.json` under
/// `~/Library/Application Support/fletch-host` or wherever `--data-dir` points.
///
/// `None` until a host publishes one — which is what every unit test and every
/// caller that is not [`crate::host::boot`] sees, and the profile then carries
/// the bundle-id deny alone, exactly as it always did.
static DATA_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Publish the data dir this engine was configured with. Called once from
/// [`crate::host::boot`], before anything can spawn an agent.
pub fn set_data_dir(dir: PathBuf) {
    *DATA_DIR.write() = Some(dir);
}

/// The data dir [`set_data_dir`] published, if a host published one.
pub(crate) fn configured_data_dir() -> Option<PathBuf> {
    DATA_DIR.read().clone()
}

/// Resolve the engine for an agent stamped with `kind`, availability-checked
/// at spawn time. An agent stamped with an engine this host does not have —
/// a box on a machine that cannot boot one, seatbelt off macOS — is a hard
/// error, never a quiet launch under the other.
pub fn engine_for(kind: EngineKind) -> Result<Arc<dyn SandboxEngine>> {
    match kind {
        // Behind the platform: `seatbelt::refusal` is the one fact that the
        // macOS sandbox exists only on macOS, read here, by the spawn
        // override and by the composer's list alike.
        EngineKind::SandboxExec => match seatbelt::refusal() {
            None => Ok(seatbelt_engine()),
            Some(reason) => Err(Error::SandboxUnavailable(reason)),
        },
        // Behind the runtime probe: `boxes::availability` refuses with the
        // first thing missing, so a box-stamped agent never launches
        // elsewhere.
        EngineKind::Box => boxes::availability().map(|_| boxes::BoxEngine::shared() as Arc<_>),
    }
}

/// The `projects` dir `kind` gives claude in place of the shared
/// `~/.claude/projects`, for the agent whose writable root is `writable_root`;
/// `None` when the engine lets claude write the shared one (seatbelt). The
/// box engine mirrors the guest's projects dir into
/// `<box dir>/claude-projects` over its transcript relay. The transcript
/// writer (`transcripts::claude_projects_dir`) and reader (`transcripts::
/// find_session_jsonl`) consult this, so neither names an engine.
pub(crate) fn transcript_root(kind: EngineKind, writable_root: &Path) -> Option<PathBuf> {
    match kind {
        // The box dir is a sibling of the writable root, named by the same
        // agent id (its last component).
        EngineKind::Box => boxes::transcript_dir_for_root(writable_root),
        EngineKind::SandboxExec => None,
    }
}

/// Every per-agent transcript root some engine keeps under `writable_root`
/// ([`transcript_root`]), for a reader that does not know the agent's
/// engine.
pub(crate) fn all_transcript_roots(writable_root: &Path) -> Vec<PathBuf> {
    EngineKind::ALL
        .iter()
        .filter_map(|&kind| transcript_root(kind, writable_root))
        .collect()
}

/// Where the host reads `provider`'s sessions for `agent_id` when they live
/// somewhere other than the host's own dirs: the mirror under a box agent's
/// box dir, refreshed first from a running box for a provider whose relay
/// pulls rather than tails. For every other engine's agent the directory is
/// absent (or `None`), so a reader falls through to the provider's own dirs.
pub(crate) fn session_mirror(provider: &str, agent_id: &str) -> Option<PathBuf> {
    boxes::home::sessions_mirror(provider, agent_id)
}

/// Create the `provider` session file `rel` (relative to its sessions dir)
/// holding `lines`, for the agent `agent_id` running in `cwd` under `engine`,
/// when the engine keeps the agent's sessions itself: a box writes it in the
/// guest, where the CLI that resumes it reads, and in the host's mirror, and
/// answers the mirror's path. `None` when the provider writes it on the host
/// as usual.
pub(crate) fn create_session_file(
    provider: &str,
    engine: EngineKind,
    agent_id: &str,
    cwd: &Path,
    rel: &Path,
    lines: &[serde_json::Value],
) -> Result<Option<PathBuf>> {
    match engine {
        EngineKind::Box => {
            let home = self::provider::require(provider)?;
            boxes::home::create_session_file(home, agent_id, cwd, rel, lines).map(Some)
        }
        EngineKind::SandboxExec => Ok(None),
    }
}

/// The engine stamped on an agent's record at creation. Records from before
/// engine selection existed (NULL) always ran under sandbox-exec, so that
/// stays their permanent kind — never re-derived from the live setting. A
/// record of a retired engine reads as seatbelt here; every launch and fork
/// refuses it first ([`ensure_engine_not_retired`]).
pub fn stamped_engine(record: &AgentRecord) -> EngineKind {
    record
        .sandbox_engine
        .as_deref()
        .and_then(EngineKind::from_setting)
        .unwrap_or(EngineKind::SandboxExec)
}

/// The engines Fletch no longer runs, by their stored spelling, with where a
/// workspace of theirs ran.
const RETIRED_ENGINES: &[(&str, &str)] = &[
    ("docker", "a Docker container"),
    ("podman", "a Podman container"),
];

/// Refuse `record` when it is stamped with a retired engine: launching it
/// under another would be a silent change of its isolation. Checked before
/// every launch ([`before_launch`]) and fork ([`WorkspaceType::stamped`]).
pub fn ensure_engine_not_retired(record: &AgentRecord) -> Result<()> {
    let stored = record.sandbox_engine.as_deref().unwrap_or_default();
    match RETIRED_ENGINES.iter().find(|(name, _)| *name == stored) {
        Some((_, ran_in)) => Err(Error::SandboxUnavailable(format!(
            "This workspace ran in {ran_in}, which Fletch no longer supports; \
             start a new workspace."
        ))),
        None => Ok(()),
    }
}

/// A spawn's per-session engine override (`SpawnRequest::workspace_type`,
/// docs/sandbox-foundation.md rule 8), resolved by [`engine_for_spawn`].
///
/// `Host` and `Box` are the user-facing choice the composer offers
/// ([`workspace_types`]), read off the wire by [`WorkspaceType::parse`].
/// `Stamped` is the exact engine an existing record was stamped with, built
/// only here ([`WorkspaceType::stamped`]): a fork passes its parent's, so the
/// child runs under the parent's engine, whichever it is, even where the
/// automatic choice would now pick another. It is not the project's
/// placement setting (`placement::Placement`), which only feeds the choice
/// made when there is no override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceType {
    /// The macOS sandbox.
    Host,
    /// A box, or a refusal with the reason when there can be none.
    Box,
    /// The engine an existing record was stamped with.
    Stamped(EngineKind),
}

impl WorkspaceType {
    /// The composer's spelling: `host` or `box`. Anything else, or none, is
    /// no override.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "host" => Some(Self::Host),
            "box" => Some(Self::Box),
            _ => None,
        }
    }

    /// The engine `record` was stamped with, as an override: what a fork
    /// passes for its parent. Refused for a retired engine.
    pub fn stamped(record: &AgentRecord) -> Result<Self> {
        ensure_engine_not_retired(record)?;
        Ok(Self::Stamped(stamped_engine(record)))
    }
}

/// Whether `provider` can launch under `engine`: the one support gate,
/// [`provider::supports`], as the spawn path's error. Checked before every
/// spawn — fresh spawns and resume/view-switch — so a record stamped with an
/// engine can never launch a provider that engine cannot run.
pub fn ensure_supports_provider(engine: EngineKind, provider: &str) -> Result<()> {
    self::provider::supports(engine, provider).map_err(Error::Other)
}

/// The engine a new agent is stamped with (docs/sandbox-foundation.md rule
/// 8). An agent adopting `adopted_tree` runs where the tree lives
/// ([`engine_for_adopted_tree`]) and ignores `override_`: a tree in a box is
/// reachable only there, a host tree never from a guest. Anyone else gets
/// the session's own choice when there is one, else the automatic choice
/// over the placement of the project at `repo`, whose stored setting is
/// `placement_setting`.
///
/// The override ([`WorkspaceType`]) stands over the project's placement for
/// this spawn only: `Host` is the macOS sandbox, refused with the reason the
/// user sees where there is none (`seatbelt::refusal`: off macOS); `Box` is
/// a box, waiting at launch for an image still on its way as a project
/// pinned to boxes does, and refused with the reason the user sees
/// (`box_refusal`: a runtime that cannot boot, a provider boxes do not run)
/// when there can be no box at all — an explicit choice never quietly lands
/// somewhere else. `Stamped(kind)` is exactly `kind`, checked as the
/// explicit choice is. Without an override this never errs.
///
/// Blocking: the automatic choice stats the repository and may run the
/// runtime probe once, so a spawn calls this off the async runtime.
pub fn engine_for_spawn(
    adopted_tree: Option<&Path>,
    placement_setting: Option<&str>,
    repo: &Path,
    provider: &str,
    override_: Option<WorkspaceType>,
) -> Result<EngineKind> {
    if let Some(tree) = adopted_tree {
        return Ok(engine_for_adopted_tree(tree));
    }
    match override_ {
        Some(WorkspaceType::Host | WorkspaceType::Stamped(EngineKind::SandboxExec)) => {
            match seatbelt::refusal() {
                Some(reason) => Err(Error::Other(reason)),
                None => Ok(EngineKind::SandboxExec),
            }
        }
        Some(WorkspaceType::Box | WorkspaceType::Stamped(EngineKind::Box)) => {
            match box_refusal(provider) {
                Some(reason) => Err(Error::Other(reason)),
                None => Ok(EngineKind::Box),
            }
        }
        None => {
            let placement = placement::resolve(placement_setting, repo);
            Ok(boxes::engine_for_new_agent(provider, placement))
        }
    }
}

/// Whether a fresh agent of any of `providers` would land in a box for the
/// project at `repo`, whose stored placement is `placement_setting`: the
/// automatic choice [`engine_for_spawn`] makes when neither an adopted tree
/// nor a session's own workspace type decides. For a feature whose agents
/// adopt a host-side tree and so cannot honour that placement. Blocking, like
/// [`engine_for_spawn`].
pub fn fresh_agents_land_in_a_box<'a>(
    placement_setting: Option<&str>,
    repo: &Path,
    providers: impl IntoIterator<Item = &'a str>,
) -> bool {
    lands_in_a_box(placement_setting, repo, providers, boxes::ready_for)
}

/// [`fresh_agents_land_in_a_box`] with the runtime probe supplied as `ready`,
/// so a test can fix the answer.
fn lands_in_a_box<'a>(
    placement_setting: Option<&str>,
    repo: &Path,
    providers: impl IntoIterator<Item = &'a str>,
    ready: impl Fn(&str) -> bool,
) -> bool {
    let placement = placement::resolve(placement_setting, repo);
    providers.into_iter().any(|provider| {
        boxes::engine_for_new_agent_with(provider, placement, || ready(provider)) == EngineKind::Box
    })
}

/// The engine a workspace of `provider` moves to for the user's choice
/// `kind` (`host` or `box`, as [`WorkspaceType::parse`] reads it), or why it
/// can't move there now, in the sentence the user sees. The spawn override's
/// refusals ([`engine_for_spawn`]), plus one: the workspace is down while it
/// moves, so a box still being set up (its image, the provider's tools) is
/// refused rather than waited for. Blocking: the runtime probe.
pub fn engine_to_move_to(kind: &str, provider: &str) -> Result<EngineKind> {
    let choice = WorkspaceType::parse(kind)
        .ok_or_else(|| Error::Other(format!("Unknown workspace type `{kind}`.")))?;
    let engine = engine_for_spawn(None, None, Path::new(""), provider, Some(choice))?;
    if engine == EngineKind::Box && !boxes::ready_for(provider) {
        return Err(Error::Other(
            "The Linux VM is still being set up. Try again once it's ready.".into(),
        ));
    }
    Ok(engine)
}

/// Why a new agent of `provider` cannot run in a box, in the sentence the
/// user sees, or `None` when it can (an image still downloading is waited
/// for, not refused): the machine and runtime, then the provider gate.
fn box_refusal(provider: &str) -> Option<String> {
    if let Some(refusal) = boxes::runtime_refusal() {
        return Some(refusal);
    }
    self::provider::supports(EngineKind::Box, provider).err()
}

/// One workspace type a new agent may be given, as the composer offers it
/// ([`workspace_types`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkspaceTypeOption {
    /// `host` (the macOS sandbox) or `box` (an isolated VM): the spelling
    /// [`WorkspaceType::parse`] reads a spawn's override from.
    pub kind: &'static str,
    /// Whether a spawn may ask for it.
    pub available: bool,
    /// Why not, when it is not: the sentence the spawn would fail with. On
    /// the default option, why it is the default when that is worth saying
    /// (the macOS sandbox standing in for a box: [`host_default_reason`]).
    pub reason: Option<String>,
    /// What [`engine_for_spawn`] picks with no override.
    pub default: bool,
}

/// The workspace types a new agent of `provider` in the project at `repo`
/// (stored placement `placement_setting`) may be given: the one list the UI
/// reads. Each type is available exactly when [`engine_for_spawn`] would
/// accept it as an override, with the same sentence when not: the macOS
/// sandbox on macOS (`seatbelt::refusal`), a box where one can boot
/// (`box_refusal`). Blocking, like [`engine_for_spawn`].
///
/// The box comes first, always: it is the default engine and the macOS
/// sandbox its fallback. The order stays fixed even when the default moves
/// to this Mac, so a menu never reshuffles under the user; the `default`
/// flag says which one a spawn gets.
pub fn workspace_types(
    placement_setting: Option<&str>,
    repo: &Path,
    provider: &str,
) -> Vec<WorkspaceTypeOption> {
    let host_reason = seatbelt::refusal();
    let box_reason = box_refusal(provider);
    let placement = placement::resolve(placement_setting, repo);
    // What `engine_for_spawn` picks with a `None` override: the automatic
    // choice over the project's placement. A box refused above is never
    // ready, and a passing runtime is remembered, so the runtime is probed at
    // most once for the whole answer.
    let default = boxes::engine_for_new_agent_with(provider, placement, || {
        box_reason.is_none() && boxes::ready_for(provider)
    });
    let host_default = default == EngineKind::SandboxExec;
    let host = WorkspaceTypeOption {
        kind: "host",
        available: host_reason.is_none(),
        reason: host_reason.or_else(|| {
            host_default
                .then(|| host_default_reason(placement_setting, placement, box_reason.as_deref()))
        }),
        default: host_default,
    };
    let vm = WorkspaceTypeOption {
        kind: "box",
        available: box_reason.is_none(),
        reason: box_reason,
        default: default == EngineKind::Box,
    };
    vec![vm, host]
}

/// The default macOS sandbox's reason while the VM image downloads. The
/// composer adds the live percent to exactly this (`Composer/workspaceType.ts`).
pub const IMAGE_DOWNLOADING: &str = "VM image downloading";

/// Why the macOS sandbox is the default, in a few words: the project's
/// placement first (as [`boxes::engine_for_new_agent`] decides), then why
/// no box can be had (`box_refusal`'s `refusal`), then what the box is still
/// waiting on ([`boxes::ready_for`]).
fn host_default_reason(
    placement_setting: Option<&str>,
    placement: placement::Placement,
    refusal: Option<&str>,
) -> String {
    use placement::Placement;
    if placement == Placement::Host {
        return if Placement::parse(placement_setting) == Placement::Host {
            "set in the project's settings"
        } else {
            "this project builds a macOS app"
        }
        .into();
    }
    if let Some(refusal) = refusal {
        return match boxes::runtime_refusal() {
            Some(_) => "Linux VMs aren't available on this Mac".into(),
            None => refusal.to_string(),
        };
    }
    match boxes::setup::image_status().phase {
        "downloading" | "verifying" => IMAGE_DOWNLOADING,
        "unavailable" => "VM image download failed",
        "installed" => "Linux VM still setting up",
        _ => "VM image not downloaded yet",
    }
    .into()
}

/// Provision `spec.dest` for `record`, an agent stamped `engine`, at the base
/// branch `parent_branch` as `origin` has it, falling back to `spec.base_ref`.
/// A box's checkout goes into its guest (`git init` there, the base pushed
/// over the channel, the same fixups), the box booting with the shares the
/// agent's launch will ask for; seatbelt's is a `--shared` host clone.
pub async fn provision_checkout(
    engine: EngineKind,
    record: &AgentRecord,
    spec: &provision::CheckoutSpec<'_>,
    parent_branch: &str,
) -> Result<provision::BaseFreshness> {
    match engine {
        EngineKind::Box => {
            let identity = boxes::BoxIdentity::for_record(record)?;
            boxes::provision::provision(
                &identity,
                spec,
                Some(parent_branch),
                boxes::provision::Landing::Detached,
            )
            .await
        }
        EngineKind::SandboxExec => provision::provision_at_remote_base(spec, parent_branch).await,
    }
}

/// Restore `record`'s checkout `spec.dest`, an agent stamped `engine`, at its
/// archived tip `spec.base_ref`. With `branch` = `(name, fetch_from)` it
/// lands on branch `name`, the tip refetched from `origin` under
/// `fetch_from` when it must be; without one it lands detached. When the tip
/// is gone for good it opens detached at `parent_branch_sha`. Returns whether
/// it landed on the branch. A box rebuilds the checkout inside its guest from
/// what the source repository holds; seatbelt walks the host clone's restore
/// ladder.
pub async fn restore_checkout(
    engine: EngineKind,
    record: &AgentRecord,
    spec: &provision::CheckoutSpec<'_>,
    branch: Option<(&str, &str)>,
    parent_branch_sha: Option<&str>,
) -> Result<bool> {
    match engine {
        EngineKind::Box => {
            let identity = boxes::BoxIdentity::for_record(record)?;
            let name = branch.map(|(name, _)| name);
            boxes::provision::restore(&identity, spec, name, parent_branch_sha).await
        }
        EngineKind::SandboxExec => match branch {
            Some((name, fetch_from)) => {
                provision::provision_on_branch(spec, name, fetch_from, parent_branch_sha).await
            }
            None => provision::provision_recovering(spec, parent_branch_sha)
                .await
                .map(|()| false),
        },
    }
}

/// The engine's step before every launch of `record`, run before the launch
/// writes anything under the agent's writable root. A box-stamped agent that
/// adopted its tree runs in the box that holds it (tenants), so it is routed
/// there first and the profile lands in the guest it will run in. A record
/// of a retired engine is refused here ([`ensure_engine_not_retired`]).
pub fn before_launch(record: &AgentRecord) -> Result<()> {
    ensure_engine_not_retired(record)?;
    match stamped_engine(record) {
        EngineKind::Box => boxes::route_tenant(record).map(|_| ()),
        EngineKind::SandboxExec => Ok(()),
    }
}

/// The extra time `record`'s spawn gets for the setup it may wait on: a box
/// agent may first have to wait for the host's setup (the image download,
/// its tools disk build), far longer than any process start. Zero for every
/// other engine and once the setup is done.
pub fn setup_allowance(record: &AgentRecord) -> Duration {
    match stamped_engine(record) {
        EngineKind::Box => boxes::setup::setup_allowance(&record.provider),
        EngineKind::SandboxExec => Duration::ZERO,
    }
}

/// Discard what the engine keeps for `agent_id` outside its host dirs: a box
/// agent's VM and box directory, data disk and checkouts included. A
/// directory probe for every other agent, so callers run it unconditionally.
pub async fn discard_agent_state(agent_id: &str) -> Result<()> {
    boxes::discard_if_any(agent_id).await
}

/// A turn of `agent_id` ended, however it ended: whatever the engine opened
/// for that turn closes (a box's network opened from the composer). A no-op
/// for every other agent, so callers run it unconditionally.
pub fn turn_ended(agent_id: &str) {
    boxes::turn_ended(agent_id);
}

/// `agent_id`'s pull request merged into its repository's default branch:
/// what the engine does once a workspace's work is in the base (a box's disk
/// becomes what its project's next boxes start with). A no-op for every
/// other agent, so callers run it unconditionally.
pub async fn merged_into_default_branch(agent_id: &str) {
    boxes::merged_into_default_branch(agent_id).await;
}

/// Stop every engine-held VM cleanly as the app quits, once the agents'
/// sessions are gone, so no box disk is left dirty.
pub fn shutdown_all() {
    boxes::shutdown_all();
}

/// The engine's step after a turn checkpoint of `agent_id` was captured: a
/// box agent's checkpoint lives in the guest, and the host-side mirror
/// follows it so publish has the objects whenever the box next stops.
pub async fn after_checkpoint(agent_id: &str) -> Result<()> {
    boxes::mirror::refresh_agent(agent_id).await
}

/// The repository host git runs transport (fetch, push) over for `checkout`
/// (docs/sandbox-foundation.md rule 4): `None` for the checkout itself, on
/// the host; for a checkout in a box, its host-side mirror, refreshed from the
/// guest now (booting a stopped box). `source_repo` is the user's repository
/// the mirror's alternates point at.
pub async fn transport_repo(checkout: &Path, source_repo: &Path) -> Result<Option<PathBuf>> {
    boxes::mirror::for_transport(checkout, source_repo).await
}

/// After transport moved the tracking refs in `repo`, the stand-in
/// [`transport_repo`] gave for `checkout`, make them `checkout`'s too, where
/// the agent and the Git panel read them. A no-op for a host checkout.
pub async fn sync_tracking(repo: &Path, checkout: &Path) -> Result<()> {
    boxes::mirror::push_tracking(repo, checkout).await
}

/// Make `sha`, a commit the host wrote into the transport repository (a
/// signed copy on publish), an object in `checkout` before any ref there
/// names it. A no-op for a host checkout, whose store already has it.
pub async fn export_commit(checkout: &Path, sha: &str) -> Result<()> {
    boxes::mirror::export_commit(checkout, sha).await
}

/// Make `refname` (a turn checkpoint) in the checkout `source` exist in the
/// checkout `dest` under the same name, force-updated: a re-captured
/// checkpoint is never a fast-forward of the old one. Wherever each lives:
///
/// - on one filesystem (the host, or one box's guest): `dest` fetches from
///   `source` by path, where `dest` lives;
/// - out of a box: the one ref fetched from `source` in the guest into the
///   box's host-side mirror under a name of this carry's own
///   (docs/sandbox-foundation.md rule 4), which a host `dest` fetches from;
///   the name is dropped from the mirror afterwards;
/// - into a box: pushed over the box's channel, from the host `source` or
///   from the source box's mirror.
///
/// A box on either side is booted if it is stopped: carrying code is a user
/// action (a fork).
pub async fn carry_checkpoint(source: &Path, dest: &Path, refname: &str) -> Result<()> {
    let from_box = boxes::box_holding(source);
    let into_box = boxes::box_holding(dest);
    let one_place = match (&from_box, &into_box) {
        (None, None) => true,
        (Some(s), Some(d)) => s.agent_id == d.agent_id,
        (Some(_), None) | (None, Some(_)) => false,
    };
    if one_place {
        return fetch_ref(dest, source, &format!("+{refname}:{refname}")).await;
    }
    // Out of a box, the ref is read under the name `carry_out` gave it in the
    // mirror: the original name there is shared by every checkout of the box.
    let (from, carried) = match &from_box {
        Some(identity) => {
            let (mirror, carried) = boxes::mirror::carry_out(identity, source, refname).await?;
            (mirror, Some(carried))
        }
        None => (source.to_path_buf(), None),
    };
    let refspec = format!("+{}:{refname}", carried.as_deref().unwrap_or(refname));
    let result = match &into_box {
        Some(identity) => boxes::mirror::push_into(identity, dest, &from, &[refspec]).await,
        None => fetch_ref(dest, &from, &refspec).await,
    };
    if let Some(carried) = &carried {
        boxes::mirror::drop_carried(&from, carried).await;
    }
    result
}

/// `git fetch --no-tags <from> <refspec>` in `dest`, `from` named by path.
async fn fetch_ref(dest: &Path, from: &Path, refspec: &str) -> Result<()> {
    let from = from
        .to_str()
        .ok_or_else(|| Error::InvalidPath(from.display().to_string()))?;
    crate::git::run_git(
        dest,
        &["fetch", "--no-tags", from, refspec],
        "fetch checkpoint",
    )
    .await?;
    Ok(())
}

/// The interactive shell the user's workspace terminal runs for the checkout
/// `tree`, when the engine keeps it somewhere the host's shell can't reach:
/// `(program, args, kill)` for a host PTY to spawn, `None` for a host tree,
/// which gets the user's own shell. A box's is a shell in its guest over the
/// box's channel (booting a stopped box), interactive but not a login shell:
/// Debian's /etc/profile resets PATH, dropping the tools disk (claude,
/// codegraph) the guest env puts on it. The box is held like an agent's
/// session from before it is reached, so its idle stop never ends the
/// user's shell under them; `kill` releases it, and a return without a shell
/// drops the hold.
pub fn shell_program(tree: &Path) -> Result<Option<(PathBuf, Vec<String>, KillHandle)>> {
    let hold = boxes::hold_session(tree);
    Ok(
        boxes::guest_program(tree, "bash", &["-i".to_string()])?.map(|(program, args)| {
            let kill = hold.map_or(KillHandle::ProcessGroup, boxes::Hold::into_kill);
            (program, args, kill)
        }),
    )
}

/// The Run panel's process for the checkout `tree` when the engine keeps the
/// tree where the host's shell cannot run a command in it: a box's, run in
/// its guest over the box's channel, the dev server's port forwarded to this
/// host ([`GuestRun`]). `None` for a host tree, which the Run profile runs on
/// the host. The box is held against its idle stop from here (booting a
/// stopped one) until the session the launch starts ends; a `GuestRun`
/// dropped before its launch releases it.
pub fn guest_run(tree: &Path) -> Result<Option<GuestRun>> {
    GuestRun::open(tree)
}

/// Whether `path` is a checkout whose engine is not running, so work there
/// would boot it: a stopped box. `false` for a host path. A timer-driven read
/// skips such a checkout (docs/sandbox-foundation.md rule 2).
pub fn checkout_asleep(path: &Path) -> bool {
    boxes::asleep_for_path(path)
}

/// The disposal state of `path`, a checkout whose engine is not running
/// ([`checkout_asleep`]), as read when it stopped: a box records its
/// checkouts' state on every stop and drops the record as it starts. `None`
/// unless the box is stopped, and when nothing usable was recorded, which a
/// caller about to delete must treat as unknown.
pub fn disposal_at_stop(path: &Path) -> Option<crate::git_state::DisposalCheck> {
    boxes::mirror::disposal_at_stop(path)
}

/// Whether `path` is a checkout the engine keeps off the host (in a box,
/// running or not).
pub fn checkout_is_remote(path: &Path) -> bool {
    boxes::box_holding(path).is_some()
}

/// Whether `path` is in a box that is running, so work there would not boot
/// it.
pub fn checkout_awake(path: &Path) -> bool {
    boxes::running_for_path(path)
}

/// The agent binary handed to `launch_agent` under `engine`: in a box the CLI
/// on its tools disk, under seatbelt what `host_resolve` finds on this host.
/// `label` names the provider in a refusal.
pub fn agent_bin(
    engine: EngineKind,
    provider: &str,
    label: &str,
    host_resolve: impl FnOnce() -> Result<String>,
) -> Result<String> {
    match engine {
        // The CLI on the box's tools disk; a host path would mean nothing in
        // the guest.
        EngineKind::Box => fletch_box::tools::bin_path(provider)
            .ok_or_else(|| Error::Other(format!("{label} isn't available in Linux VMs yet"))),
        EngineKind::SandboxExec => host_resolve(),
    }
}

/// The codegraph launcher as `agent_id`'s sandbox sees it under `engine`,
/// when it has one: what `host_bin` finds for a seatbelt agent, which runs on
/// this host; the Linux build on the tools disk the agent's box runs with.
pub fn codegraph_bin(
    engine: EngineKind,
    agent_id: &str,
    provider: &str,
    host_bin: impl FnOnce() -> Option<String>,
) -> Option<String> {
    match engine {
        EngineKind::SandboxExec => host_bin(),
        EngineKind::Box => boxes::codegraph_bin(agent_id, provider),
    }
}

/// The seatbelt engine, shared process-wide: it is stateless, and per-launch
/// state (profile tempfile) lives on the `LaunchPlan`.
fn seatbelt_engine() -> Arc<dyn SandboxEngine> {
    static ENGINE: OnceLock<Arc<dyn SandboxEngine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| Arc::new(seatbelt::SandboxExecEngine))
        .clone()
}

#[cfg(test)]
mod adopted_tree_tests {
    use super::*;

    /// An agent adopting a tree runs where the tree lives: in the box that
    /// holds it, and never in a box for a tree no box holds.
    #[tokio::test]
    async fn an_adopting_agent_runs_where_its_tree_lives() {
        let registry = boxes::BoxRegistry::global();
        let root = PathBuf::from("/adopted-tree-test/coder-root");
        registry.register(boxes::BoxIdentity {
            agent_id: "adopted-tree-test-coder".into(),
            project_id: "p".into(),
            provider: "claude".into(),
            writable_root: root.clone(),
            rpc_dir: PathBuf::from("/adopted-tree-test/rpc"),
            home: PathBuf::from("/adopted-tree-test/home"),
            shares: Vec::new(),
        });
        assert_eq!(engine_for_adopted_tree(&root.join("repo")), EngineKind::Box);
        // A host tree never resolves to a box.
        assert_ne!(
            engine_for_adopted_tree(Path::new("/adopted-tree-test/runs/run-1/repo")),
            EngineKind::Box
        );
        registry.discard("adopted-tree-test-coder").await.unwrap();
        assert_ne!(engine_for_adopted_tree(&root.join("repo")), EngineKind::Box);
    }
}

#[cfg(test)]
mod workspace_type_tests {
    use super::*;

    /// The composer's spelling is `host` or `box`; nothing else is an
    /// override, and no spelling reaches a stamped engine.
    #[test]
    fn only_host_and_box_parse() {
        assert_eq!(WorkspaceType::parse("host"), Some(WorkspaceType::Host));
        assert_eq!(WorkspaceType::parse(" box "), Some(WorkspaceType::Box));
        for raw in ["", "auto", "sandbox-exec", "vm"] {
            assert_eq!(WorkspaceType::parse(raw), None, "{raw}");
        }
    }

    /// A session's workspace type stands over the host's choice and the
    /// project's placement, an explicit choice is never quietly something
    /// else, and an adopted tree keeps running where it lives. Written to
    /// hold whether or not this machine can boot a box.
    #[test]
    fn a_session_choice_is_honoured_or_refused() {
        let repo = tempfile::tempdir().unwrap();
        // A project pinned to boxes still gets this Mac when the session
        // asks; off macOS there is no such thing, and the choice is refused
        // with the sentence the composer's list carries, never a box.
        let host = engine_for_spawn(
            None,
            Some("box"),
            repo.path(),
            "claude",
            Some(WorkspaceType::Host),
        );
        match seatbelt::refusal() {
            None => assert_eq!(host.unwrap(), EngineKind::SandboxExec),
            Some(reason) => assert_eq!(host.unwrap_err().to_string(), reason),
        }
        // A provider boxes do not run: refused whatever the runtime says,
        // never stamped seatbelt instead.
        assert!(engine_for_spawn(
            None,
            None,
            repo.path(),
            "antigravity",
            Some(WorkspaceType::Box)
        )
        .is_err());
        // Without an override nothing errs.
        assert!(engine_for_spawn(None, None, repo.path(), "antigravity", None).is_ok());
        // An adopted host tree ignores the override.
        let tree = Path::new("/workspace-type-test/runs/run-1/repo");
        let adopted = engine_for_spawn(
            Some(tree),
            None,
            repo.path(),
            "claude",
            Some(WorkspaceType::Box),
        )
        .unwrap();
        assert_ne!(adopted, EngineKind::Box);
    }

    /// A stamped engine is kept exactly, whatever the project's placement
    /// says: a fork of a seatbelt parent stays on this Mac — and off macOS,
    /// where there is no seatbelt, it is refused rather than moved.
    #[test]
    fn a_stamped_engine_is_kept() {
        let repo = tempfile::tempdir().unwrap();
        let kept = engine_for_spawn(
            None,
            Some("box"),
            repo.path(),
            "claude",
            Some(WorkspaceType::Stamped(EngineKind::SandboxExec)),
        );
        match seatbelt::refusal() {
            None => assert_eq!(kept.unwrap(), EngineKind::SandboxExec),
            Some(reason) => assert_eq!(kept.unwrap_err().to_string(), reason),
        }
    }

    /// Off macOS the macOS sandbox is not on offer: the list says so with
    /// the reason, the box is the default, and asking for the sandbox anyway
    /// is refused at the spawn and at the launch with the same sentence.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn off_macos_the_macos_sandbox_is_not_offered() {
        let repo = tempfile::tempdir().unwrap();
        let reason = seatbelt::refusal().expect("no seatbelt off macOS");
        let options = workspace_types(None, repo.path(), "claude");
        let [vm, host] = [option(&options, "box"), option(&options, "host")];
        assert!(!host.available && !host.default);
        assert_eq!(host.reason.as_deref(), Some(reason.as_str()));
        assert!(vm.default, "the box is the only engine here");
        assert!(engine_for(EngineKind::SandboxExec).is_err());
    }

    /// A record of a retired engine is refused, naming the engine, never
    /// launched or forked under another.
    #[test]
    fn a_retired_engine_is_refused() {
        let mut record = crate::workspace::new_agent_record(
            "retired".into(),
            "retired".into(),
            "claude".into(),
            crate::workspace::TrackedRepo {
                repo_path: PathBuf::from("/r"),
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
                adopted_checkout: None,
            },
            String::new(),
            crate::workspace::AgentView::Custom,
        );
        record.sandbox_engine = Some("docker".into());
        let err = ensure_engine_not_retired(&record).unwrap_err().to_string();
        assert!(err.contains("Docker container"), "{err}");
        assert!(WorkspaceType::stamped(&record).is_err());
        assert!(before_launch(&record).is_err());
        record.sandbox_engine = Some("podman".into());
        assert!(ensure_engine_not_retired(&record).is_err());
        for kept in [None, Some("sandbox-exec"), Some("box")] {
            record.sandbox_engine = kept.map(Into::into);
            assert!(ensure_engine_not_retired(&record).is_ok(), "{kept:?}");
        }
    }

    /// A stamped box that cannot exist refuses with the reason, as an
    /// explicit box does, never falling back to this Mac.
    #[test]
    fn a_stamped_box_that_cannot_exist_is_refused() {
        let repo = tempfile::tempdir().unwrap();
        let err = engine_for_spawn(
            None,
            Some("host"),
            repo.path(),
            "antigravity",
            Some(WorkspaceType::Stamped(EngineKind::Box)),
        )
        .expect_err("a stamped box for a provider boxes do not run must refuse");
        assert_eq!(Some(err.to_string()), box_refusal("antigravity"));
    }

    /// The option of `kind` in a list, wherever it sits.
    fn option<'a>(options: &'a [WorkspaceTypeOption], kind: &str) -> &'a WorkspaceTypeOption {
        options
            .iter()
            .find(|o| o.kind == kind)
            .unwrap_or_else(|| panic!("no {kind} option"))
    }

    /// The box is listed first and the macOS sandbox second, whichever is
    /// the default: the order never moves under the user.
    #[test]
    fn the_box_is_listed_first_whatever_the_default() {
        let repo = tempfile::tempdir().unwrap();
        for provider in ["claude", "antigravity"] {
            let kinds: Vec<&str> = workspace_types(None, repo.path(), provider)
                .iter()
                .map(|o| o.kind)
                .collect();
            assert_eq!(kinds, ["box", "host"], "{provider}");
        }
    }

    /// The list offers both types, this Mac always, and a box only when the
    /// override would be accepted, with the refusal as its reason.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_list_says_why_a_box_is_not_offered() {
        let repo = tempfile::tempdir().unwrap();
        let options = workspace_types(None, repo.path(), "antigravity");
        assert!(options
            .iter()
            .all(|o| WorkspaceType::parse(o.kind).is_some()));
        let [vm, host] = [option(&options, "box"), option(&options, "host")];
        assert!(host.available && host.default);
        assert!(
            host.reason.as_deref().is_some_and(|r| !r.is_empty()),
            "the default says why it is not a box"
        );
        assert!(!vm.available);
        assert!(vm.reason.as_deref().is_some_and(|r| !r.is_empty()));
        assert!(!vm.default, "nothing defaults to a refused box here");
    }

    /// A project that stays on this Mac says so, in the words of where that
    /// was decided: its detection, or its settings.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_default_says_why_it_stays_on_this_mac() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join("App.xcodeproj")).unwrap();
        let reason = |setting| {
            option(&workspace_types(setting, repo.path(), "claude"), "host")
                .reason
                .clone()
        };
        assert_eq!(
            reason(None).as_deref(),
            Some("this project builds a macOS app")
        );
        assert_eq!(
            reason(Some("host")).as_deref(),
            Some("set in the project's settings")
        );
    }
}

#[cfg(test)]
mod tests {
    /// Files outside `sandbox/` that name engine internals today. This list
    /// may only shrink: a feature that needs an engine fact asks
    /// `sandbox` for it instead of matching on the engine itself. These are
    /// the set named in docs/sandbox-foundation.md rule 1: the engine's own
    /// settings surface (`commands/boxes.rs`, `commands/settings.rs`, their
    /// re-export in `commands/mod.rs`, the per-project egress setting in
    /// `commands/project_settings.rs` and `project_file/mod.rs`), boot
    /// (`host/boot.rs`) and the event list (`remote/events.rs`). Paths are
    /// relative to this crate's `src/`.
    const ALLOWED: &[&str] = &[
        "commands/boxes.rs",
        "commands/mod.rs",
        "commands/project_settings.rs",
        "commands/settings.rs",
        "host/boot.rs",
        "project_file/mod.rs",
        "remote/events.rs",
    ];

    /// What marks a line as engine knowledge: matching on an engine variant,
    /// or reaching into the box engine's module.
    const TOKENS: &[&str] = &["EngineKind::", "sandbox::boxes", "boxes::", "box_holding("];

    /// Engine knowledge stays in `sandbox/` (docs/sandbox-foundation.md, rule
    /// 1). Every production line of every file outside it counts, comments
    /// and doc comments included: simple and conservative. A file that gains
    /// one of [`TOKENS`] fails this until the knowledge moves behind
    /// `sandbox`; a file that loses its last one fails it until it is struck
    /// from [`ALLOWED`], so the list cannot go stale.
    #[test]
    fn engine_knowledge_stays_in_sandbox() {
        let found: Vec<String> = crate::source_scan::production_sources()
            .into_iter()
            .filter(|(rel, text)| {
                !rel.starts_with("sandbox/") && TOKENS.iter().any(|t| text.contains(t))
            })
            .map(|(rel, _)| rel)
            .collect();
        let added: Vec<&String> = found
            .iter()
            .filter(|rel| !ALLOWED.contains(&rel.as_str()))
            .collect();
        let gone: Vec<&&str> = ALLOWED
            .iter()
            .filter(|rel| !found.iter().any(|f| f == *rel))
            .collect();
        assert!(
            added.is_empty() && gone.is_empty(),
            "engine knowledge outside sandbox/ must go through sandbox (docs/sandbox-foundation.md \
             rule 1).\nnew files naming engine internals: {added:?}\nno longer naming them, \
             strike from sandbox::tests::ALLOWED: {gone:?}"
        );
    }

    /// The answer mirrors the spawn path's automatic choice: a project pinned
    /// to boxes is boxed wherever one can boot, a project pinned to this Mac
    /// never is, and an undecided project follows whether a box is ready.
    #[test]
    fn fresh_agents_are_boxed_exactly_when_a_spawn_would_box_them() {
        use super::lands_in_a_box;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let claude = ["claude"];

        assert!(lands_in_a_box(Some("box"), repo, claude, |_| true));
        if cfg!(target_os = "macos") {
            assert!(!lands_in_a_box(Some("host"), repo, claude, |_| true));
            assert!(!lands_in_a_box(None, repo, claude, |_| false));
            assert!(lands_in_a_box(None, repo, claude, |_| true));
            // One boxed provider among several is enough.
            assert!(lands_in_a_box(
                None,
                repo,
                ["claude", "codex"],
                |provider| provider == "codex"
            ));
            // A provider nobody asked about decides nothing.
            assert!(!lands_in_a_box(None, repo, claude, |provider| provider == "codex"));
            // A native-looking project runs on this Mac whatever is ready.
            std::fs::create_dir(repo.join("App.xcodeproj")).unwrap();
            assert!(!lands_in_a_box(None, repo, claude, |_| true));
        } else {
            // Off macOS there is no seatbelt: every fresh agent is a box.
            assert!(lands_in_a_box(Some("host"), repo, claude, |_| false));
        }
    }
}
