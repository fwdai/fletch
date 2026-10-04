//! Delegations: one git playbook handed to the coding agent, as a host fact.
//!
//! A delegation is "the agent takes it from here" — the judgment part of an
//! action (a commit message, a PR description, conflict edits, a test fix)
//! belongs to the agent, and the host watches for the transition that proves it
//! landed. It lives here, not in a client, because it has to keep running with
//! every window closed, happen once however many clients are connected, and
//! spend an agent turn.
//!
//! Three rules carry over from the desktop's watcher (`src/delegation.ts`, which
//! this replaces as the acting copy):
//!
//! - **Hold the trigger while the agent is mid-turn.** A message written to a
//!   running turn coalesces into it instead of running as its own, and then the
//!   turn's git ops cannot be told apart from ours. So a delegation sent to a
//!   `Running` agent is recorded `queued` and delivered when the agent settles —
//!   at most one per agent per pass, or two queued on one agent's two checkouts
//!   would coalesce with each other.
//! - **Resolve on causality, not on a snapshot.** Done only when the agent ran an
//!   op from this delegation's own playbook during the delegated turn
//!   (`agent:git-action`, see [`action_proves_kind`]) *and* the checkout reached
//!   the target ([`delegation_resolved`]). A target already met by hand would
//!   otherwise read as work the agent never did.
//! - **A settled agent is the normal ending for what cannot be observed.**
//!   `fix-checks` and `resolve-comments` never resolve from state
//!   ([`settles_on_idle`]), so their give-up is a success.
//!
//! The table is in memory and keyed per checkout (`agent_id`, `subdir`), the
//! scope a delegation targets: a multi-repo agent may hold one per repo, each
//! judged against its own repo's state. The decision functions are pure and
//! tested here; [`spawn`] is the driver that feeds them status transitions,
//! `agent:git-action` acks and a short tick while anything is live.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Notify;

use crate::error::{Error, Result};
use crate::git_state::{GitState, StatusKind};
use crate::github::{MergeState, MergeableState, PrChecks, PrState, PrStatus};
use crate::host::{EngineCtx, EventSink};
use crate::workspace::{AgentRecord, AgentStatus};

use super::Supervisor;

/// The event every client mirrors its delegation view from.
pub const EVENT_CHANGED: &str = "delegation:changed";

/// Marker prefix for app-sent action triggers. The full per-action playbooks
/// live in the agent's injected instructions (`instructions/git_actions.md`),
/// so the chat carries only this one-liner.
pub const APP_ACTION_PREFIX: &str = "[app-action] ";

/// How long a settled agent may sit without its delegated turn having been seen
/// running before the delegation reads as abandoned. Covers send→turn-start
/// latency, and the gap between a dequeued trigger and its turn starting.
pub const GIVE_UP_GRACE_MS: i64 = 15_000;

/// How often the driver re-reads the checkouts of live delegations. Status
/// transitions and git-action acks wake it at once; this is the pulse for the
/// give-up clock and for a target landing after the turn that produced it.
const TICK: Duration = Duration::from_secs(2);

/// What a settled delegation whose target never appeared tells the user.
pub const ABANDONED_NOTICE: &str = "Agent finished — review the chat for details";

/// What a delegation whose trigger could not be delivered tells the user.
const UNDELIVERED_NOTICE: &str = "Couldn't hand the action to the agent";

// ---------------------------------------------------------------------------
// Pure decisions
// ---------------------------------------------------------------------------

/// Every unit of work that can be handed to the coding agent. The wire
/// spelling is the playbook name, except `Resolve`, whose playbook is
/// `resolve-conflicts` (see [`DelegationKind::from_action`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DelegationKind {
    Commit,
    CommitPush,
    CommitPr,
    OpenPr,
    Push,
    Resolve,
    UpdateBranch,
    FixChecks,
    ResolveComments,
}

impl DelegationKind {
    pub const ALL: [DelegationKind; 9] = [
        DelegationKind::Commit,
        DelegationKind::CommitPush,
        DelegationKind::CommitPr,
        DelegationKind::OpenPr,
        DelegationKind::Push,
        DelegationKind::Resolve,
        DelegationKind::UpdateBranch,
        DelegationKind::FixChecks,
        DelegationKind::ResolveComments,
    ];

    /// The playbook name the trigger carries (`[app-action] <action>`).
    pub fn action(self) -> &'static str {
        match self {
            DelegationKind::Commit => "commit",
            DelegationKind::CommitPush => "commit-push",
            DelegationKind::CommitPr => "commit-pr",
            DelegationKind::OpenPr => "open-pr",
            DelegationKind::Push => "push",
            DelegationKind::Resolve => "resolve-conflicts",
            DelegationKind::UpdateBranch => "update-branch",
            DelegationKind::FixChecks => "fix-checks",
            DelegationKind::ResolveComments => "resolve-comments",
        }
    }

    /// The kind a playbook name delegates, or `None` for a name that is not a
    /// playbook — which `delegate_git` refuses rather than send.
    pub fn from_action(action: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.action() == action)
    }
}

/// Kinds whose completion cannot be observed in git/PR state: CI takes minutes,
/// and GitHub reports thread resolution on its own schedule.
/// [`delegation_resolved`] never returns true for them, so a settled agent is
/// their normal ending rather than an abandonment.
pub fn settles_on_idle(kind: DelegationKind) -> bool {
    matches!(
        kind,
        DelegationKind::FixChecks | DelegationKind::ResolveComments
    )
}

/// Does a successful `agent:git-action` op stand as proof that THIS
/// delegation's requested work ran? The op must belong to the delegation's own
/// playbook — a turn we queued behind can emit an unrelated mutation. Resolution
/// still ANDs this with the target snapshot, so listing every op a kind touches
/// (not just the final one) is safe.
pub fn action_proves_kind(kind: DelegationKind, op: &str) -> bool {
    match kind {
        DelegationKind::Commit => op == "git_commit",
        // A conflicted merge completes as a merge commit (post-commit reports
        // `git_update_branch`); a rebase/cherry-pick resolution is a plain
        // commit. The snapshot (no conflicts left) gates completion.
        DelegationKind::Resolve => op == "git_commit" || op == "git_update_branch",
        DelegationKind::CommitPush | DelegationKind::FixChecks => {
            op == "git_commit" || op == "git_push"
        }
        DelegationKind::CommitPr => op == "git_commit" || op == "open_pr",
        DelegationKind::OpenPr => op == "open_pr",
        DelegationKind::Push => op == "git_push",
        // Any thread action proves the turn engaged with the review, and a turn
        // that only pushed back is a legitimate outcome.
        DelegationKind::ResolveComments => op == "reply_thread" || op == "resolve_thread",
        // Only an actual base merge: a plain commit reports `git_commit`, so an
        // unrelated commit cannot stand in for the merge.
        DelegationKind::UpdateBranch => op == "git_update_branch",
    }
}

/// Whether the transition this delegation waits for has landed, judged against
/// the delegation's OWN checkout.
pub fn delegation_resolved(
    kind: DelegationKind,
    git: Option<&GitState>,
    pr: Option<&PrState>,
    checks: Option<&PrChecks>,
) -> bool {
    let clean = git.is_some_and(|g| g.files.is_empty());
    let pr_open = pr.is_some_and(|p| p.state == PrStatus::Open);
    match kind {
        DelegationKind::Commit => clean,
        DelegationKind::CommitPush => clean && git.is_some_and(|g| g.unpushed == 0),
        // A PR may already be open (new changes pushed onto it), so "PR open"
        // alone is not evidence — the tree must be clean too.
        DelegationKind::CommitPr => clean && pr_open,
        DelegationKind::OpenPr => pr_open,
        DelegationKind::Push => git.is_some_and(|g| g.unpushed == 0),
        DelegationKind::Resolve => git.is_some_and(|g| {
            !g.files
                .iter()
                .any(|f| matches!(f.kind, StatusKind::Conflicted))
        }),
        // `Unknown` = GitHub still recomputing after a push — keep waiting.
        DelegationKind::UpdateBranch => match checks {
            Some(c) => !matches!(
                c.merge_state,
                MergeState::Behind | MergeState::Dirty | MergeState::Unknown
            ),
            None => pr.is_some_and(|p| p.mergeable == MergeableState::Mergeable),
        },
        DelegationKind::FixChecks | DelegationKind::ResolveComments => false,
    }
}

/// Success notice once the watched transition lands (or, for a kind that
/// [`settles_on_idle`], once the agent settles).
pub fn delegation_done(kind: DelegationKind) -> &'static str {
    match kind {
        DelegationKind::Commit => "Agent committed your changes",
        DelegationKind::CommitPush => "Committed & pushed",
        DelegationKind::CommitPr => "Committed — PR is open",
        DelegationKind::OpenPr => "PR is open",
        DelegationKind::Push => "Pushed to origin",
        DelegationKind::Resolve => "Conflicts resolved",
        DelegationKind::UpdateBranch => "Branch updated",
        DelegationKind::FixChecks => "Agent finished — checks are re-running",
        DelegationKind::ResolveComments => "Review comments addressed",
    }
}

/// Build the one-line trigger: `[app-action] <name> key="value" …`, in the
/// order given. Params carry only the dynamic context the static playbook
/// can't know; empty values are dropped and `"` is backslash-escaped. Same
/// format as the TypeScript `appActionMessage` (pinned by the shared fixture in
/// `delegation_tests.rs` and `tests/delegation.test.ts`).
pub fn app_action_message(action: &str, params: &[(&str, &str)]) -> String {
    let mut out = format!("{APP_ACTION_PREFIX}{action}");
    for (key, value) in params {
        if value.is_empty() {
            continue;
        }
        out.push_str(&format!(" {key}=\"{}\"", value.replace('"', "\\\"")));
    }
    out
}

/// One in-flight delegation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegation {
    pub kind: DelegationKind,
    /// The trigger to deliver. Held here while `queued`.
    pub prompt: String,
    /// Epoch ms when the delegation entered its current phase: set at record,
    /// reset on dequeue. The give-up grace window counts from here.
    pub started_at: i64,
    /// The delegated turn has been seen running since `started_at`. Arms the
    /// give-up clock; never confirms success.
    pub saw_running: bool,
    /// The agent ran an op from this delegation's playbook during the
    /// delegated turn. Never set while `queued`: those ops belong to the turn
    /// we are waiting behind.
    pub saw_git_op: bool,
    /// The agent was running when this was recorded, so the trigger is held
    /// until it settles.
    pub queued: bool,
}

/// What the watcher should do for one observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The watched transition landed — clear, report done.
    Resolve,
    Wait,
    /// The turn we were queued behind settled — deliver the held trigger.
    Dequeue,
    /// Our turn started — arm the give-up clock.
    MarkRunning,
    /// The agent settled without the transition — clear, report honestly.
    GiveUp,
}

pub fn delegation_step(
    delegation: &Delegation,
    status: &AgentStatus,
    resolved: bool,
    now: i64,
) -> Step {
    if resolved && delegation.saw_git_op && !delegation.queued {
        return Step::Resolve;
    }
    let active = matches!(status, AgentStatus::Running | AgentStatus::Spawning);
    // Queued behind a foreign turn: its activity is not ours to interpret.
    if delegation.queued {
        return if active { Step::Wait } else { Step::Dequeue };
    }
    if *status == AgentStatus::Running && !delegation.saw_running {
        return Step::MarkRunning;
    }
    let armed = delegation.saw_running || now - delegation.started_at > GIVE_UP_GRACE_MS;
    if !active && armed {
        return Step::GiveUp;
    }
    Step::Wait
}

/// Which checkout: the agent and the subdir of a secondary repo (`None` for the
/// primary).
pub type Key = (String, Option<String>);

/// Where a delegation is in its life, as clients see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Queued,
    Started,
    Running,
    Done,
    Abandoned,
}

/// The fresh state a pass judges one checkout against. `None` = not read (or
/// unreadable), which never resolves anything.
#[derive(Debug, Clone, Default)]
pub struct Observed {
    pub git: Option<GitState>,
    pub pr: Option<PrState>,
    pub checks: Option<PrChecks>,
}

/// What one pass decided about a single delegation (`Wait` is not
/// represented). Carries the copy, so the applier holds no policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Finish {
        key: Key,
        phase: Phase,
        notice: &'static str,
    },
    Dequeue {
        key: Key,
    },
    MarkRunning {
        key: Key,
    },
    DropOrphan {
        key: Key,
    },
}

/// Decide what to do about every in-flight delegation this pass. `statuses`
/// misses an agent that is gone (archived / discarded), which orphans its
/// delegations.
pub fn plan_pass(
    delegations: &BTreeMap<Key, Delegation>,
    statuses: &HashMap<String, AgentStatus>,
    observed: &HashMap<Key, Observed>,
    now: i64,
) -> Vec<Effect> {
    let mut effects = Vec::new();
    // At most one dequeue per agent per pass: delivering two would coalesce
    // the second into the first's turn. The loser stays queued and waits out
    // the turn just triggered.
    let mut dequeued: Vec<&str> = Vec::new();
    for (key, delegation) in delegations {
        let agent_id = key.0.as_str();
        let Some(status) = statuses.get(agent_id) else {
            effects.push(Effect::DropOrphan { key: key.clone() });
            continue;
        };
        let seen = observed.get(key);
        let resolved = delegation_resolved(
            delegation.kind,
            seen.and_then(|o| o.git.as_ref()),
            seen.and_then(|o| o.pr.as_ref()),
            seen.and_then(|o| o.checks.as_ref()),
        );
        match delegation_step(delegation, status, resolved, now) {
            Step::Resolve => effects.push(Effect::Finish {
                key: key.clone(),
                phase: Phase::Done,
                notice: delegation_done(delegation.kind),
            }),
            Step::Dequeue => {
                if !dequeued.contains(&agent_id) {
                    dequeued.push(agent_id);
                    effects.push(Effect::Dequeue { key: key.clone() });
                }
            }
            Step::MarkRunning => effects.push(Effect::MarkRunning { key: key.clone() }),
            Step::GiveUp => effects.push(if settles_on_idle(delegation.kind) {
                Effect::Finish {
                    key: key.clone(),
                    phase: Phase::Done,
                    notice: delegation_done(delegation.kind),
                }
            } else {
                Effect::Finish {
                    key: key.clone(),
                    phase: Phase::Abandoned,
                    notice: ABANDONED_NOTICE,
                }
            }),
            Step::Wait => {}
        }
    }
    effects
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One delegation as a client sees it: the `delegation:changed` payload, and a
/// row of `get_delegations` (with no `notice`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DelegationView {
    pub agent_id: String,
    pub subdir: Option<String>,
    pub kind: DelegationKind,
    pub phase: Phase,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

#[derive(Debug, Clone)]
struct Entry {
    /// Distinguishes a delegation from a later one recorded on the same key,
    /// so a pass that decided about the old one cannot finish the new one.
    id: u64,
    delegation: Delegation,
}

#[derive(Default)]
struct Inner {
    table: BTreeMap<Key, Entry>,
    next_id: u64,
}

/// The host's in-flight delegations.
#[derive(Default)]
pub struct Delegations {
    inner: Mutex<Inner>,
}

fn view(key: &Key, d: &Delegation, phase: Phase, notice: Option<&str>) -> DelegationView {
    DelegationView {
        agent_id: key.0.clone(),
        subdir: key.1.clone(),
        kind: d.kind,
        phase,
        started_at: d.started_at,
        notice: notice.map(str::to_string),
    }
}

fn live_phase(d: &Delegation) -> Phase {
    if d.queued {
        Phase::Queued
    } else if d.saw_running {
        Phase::Running
    } else {
        Phase::Started
    }
}

impl Delegations {
    fn record(
        &self,
        key: Key,
        kind: DelegationKind,
        prompt: String,
        queued: bool,
        now: i64,
    ) -> (u64, DelegationView) {
        let mut inner = self.inner.lock();
        inner.next_id += 1;
        let id = inner.next_id;
        let delegation = Delegation {
            kind,
            prompt,
            started_at: now,
            saw_running: false,
            saw_git_op: false,
            queued,
        };
        let shown = view(&key, &delegation, live_phase(&delegation), None);
        inner.table.insert(key, Entry { id, delegation });
        (id, shown)
    }

    fn snapshot(&self) -> BTreeMap<Key, (u64, Delegation)> {
        self.inner
            .lock()
            .table
            .iter()
            .map(|(k, e)| (k.clone(), (e.id, e.delegation.clone())))
            .collect()
    }

    fn is_empty(&self) -> bool {
        self.inner.lock().table.is_empty()
    }

    /// Every live delegation, in key order.
    pub fn views(&self) -> Vec<DelegationView> {
        self.inner
            .lock()
            .table
            .iter()
            .map(|(k, e)| view(k, &e.delegation, live_phase(&e.delegation), None))
            .collect()
    }

    /// Remove `key` if it still holds delegation `id`; the removed view, in
    /// `phase` with `notice`, is what to announce.
    fn finish(
        &self,
        key: &Key,
        id: u64,
        phase: Phase,
        notice: Option<&str>,
    ) -> Option<DelegationView> {
        let mut inner = self.inner.lock();
        if inner.table.get(key).map(|e| e.id) != Some(id) {
            return None;
        }
        let entry = inner.table.remove(key)?;
        Some(view(key, &entry.delegation, phase, notice))
    }

    /// Our turn started: arm the give-up clock. `None` when there was nothing
    /// to change.
    fn mark_running(&self, key: &Key, id: Option<u64>) -> Option<DelegationView> {
        let mut inner = self.inner.lock();
        let entry = inner.table.get_mut(key)?;
        if id.is_some_and(|id| id != entry.id) {
            return None;
        }
        let d = &mut entry.delegation;
        if d.queued || d.saw_running {
            return None;
        }
        d.saw_running = true;
        Some(view(key, d, Phase::Running, None))
    }

    /// Flip a held delegation to delivered and hand back its trigger. Atomic,
    /// so a repeated pass cannot deliver it twice.
    fn take_dequeue(&self, key: &Key, id: u64, now: i64) -> Option<(String, DelegationView)> {
        let mut inner = self.inner.lock();
        let entry = inner.table.get_mut(key)?;
        if entry.id != id || !entry.delegation.queued {
            return None;
        }
        let d = &mut entry.delegation;
        d.queued = false;
        d.started_at = now;
        Some((d.prompt.clone(), view(key, d, Phase::Started, None)))
    }

    /// The agent ran `op`. Sets the causal proof on every delivered delegation
    /// of that agent whose playbook the op belongs to. Agent-scoped because the
    /// event is: it says which op ran, not in which checkout. Safe, because
    /// resolution ANDs it with each checkout's own target snapshot.
    fn note_git_action(&self, agent_id: &str, op: &str) -> bool {
        let mut inner = self.inner.lock();
        let mut changed = false;
        for (key, entry) in inner.table.iter_mut() {
            let d = &mut entry.delegation;
            if key.0 != agent_id || d.queued || d.saw_git_op || !action_proves_kind(d.kind, op) {
                continue;
            }
            d.saw_git_op = true;
            changed = true;
        }
        changed
    }

    /// Every delivered delegation of `agent_id` whose turn is now running.
    fn mark_agent_running(&self, agent_id: &str) -> Vec<DelegationView> {
        let keys: Vec<Key> = self
            .inner
            .lock()
            .table
            .keys()
            .filter(|k| k.0 == agent_id)
            .cloned()
            .collect();
        keys.iter()
            .filter_map(|k| self.mark_running(k, None))
            .collect()
    }

    /// Whether a delivered delegation on this checkout already authorizes the
    /// publish `op` — the user started it moments ago, and its playbook
    /// performs this op. Scoped to the same checkout and matched through
    /// [`action_proves_kind`], so a delegation cannot launder an unrelated
    /// publish; a queued one has not been delivered, so the agent is not
    /// running it yet.
    fn pre_authorizes(&self, agent_id: &str, repo: Option<&str>, op: &str) -> bool {
        let key: Key = (agent_id.to_string(), repo.map(str::to_string));
        self.inner
            .lock()
            .table
            .get(&key)
            .is_some_and(|e| !e.delegation.queued && action_proves_kind(e.delegation.kind, op))
    }
}

/// The host's one table. Global, like the publish-approval registry it is
/// consulted by (`rpc::approval`), which has no supervisor to reach it through.
pub fn global() -> &'static Delegations {
    static TABLE: OnceLock<Delegations> = OnceLock::new();
    TABLE.get_or_init(Delegations::default)
}

fn signal() -> &'static Notify {
    static SIGNAL: OnceLock<Notify> = OnceLock::new();
    SIGNAL.get_or_init(Notify::new)
}

/// Wake the driver now.
fn nudge() {
    signal().notify_one();
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn emit(sink: &dyn EventSink, shown: &DelegationView) {
    crate::host::emit(sink, EVENT_CHANGED, shown);
}

/// Whether a live delegation on this checkout already authorizes the gated
/// publish `op` (see [`Delegations::pre_authorizes`]). `repo` is the approval
/// gate's spelling: `None` for the primary checkout.
pub fn pre_authorizes(agent_id: &str, repo: Option<&str>, op: &str) -> bool {
    global().pre_authorizes(agent_id, repo, op)
}

/// An `agent:git-action` the agent's RPC reported. Called where the host emits
/// that event, so the ack is the same fact every client hears.
pub(crate) fn note_git_action(agent_id: &str, op: &str) {
    if global().note_git_action(agent_id, op) {
        nudge();
    }
}

// ---------------------------------------------------------------------------
// The ops
// ---------------------------------------------------------------------------

/// The checkout a request names, as a table key's subdir: `None` for the
/// primary (whether named or not), the subdir for a tracked secondary, and an
/// error for a repo the agent does not track.
fn checkout_subdir(record: &AgentRecord, subdir: Option<&str>) -> Result<Option<String>> {
    let Some(requested) = subdir.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if record.repos.first().is_some_and(|r| r.subdir == requested) {
        return Ok(None);
    }
    if record.repos.iter().any(|r| r.subdir == requested) {
        return Ok(Some(requested.to_string()));
    }
    Err(Error::Other(format!("unknown repo {requested:?}")))
}

async fn deliver(
    sup: &Arc<Supervisor>,
    ctx: &Arc<EngineCtx>,
    agent_id: &str,
    prompt: &str,
) -> Result<()> {
    let turn_id = uuid::Uuid::new_v4().to_string();
    sup.clone()
        .send_user_message(ctx, agent_id, &turn_id, prompt, &[])
        .await
        .map(|_| ())
}

/// Hand `action` to the agent: compose the trigger, record the delegation, and
/// send it now — or, while the agent is running, hold it for the driver to
/// deliver when the turn settles.
async fn delegate(
    table: &Delegations,
    sup: &Arc<Supervisor>,
    ctx: &Arc<EngineCtx>,
    agent_id: &str,
    subdir: Option<&str>,
    action: &str,
    params: &BTreeMap<String, String>,
) -> Result<DelegationView> {
    let kind = DelegationKind::from_action(action)
        .ok_or_else(|| Error::Other(format!("unknown git action {action:?}")))?;
    let record = sup.workspace.agent(agent_id)?;
    if record.archive.is_some() {
        return Err(Error::Other("agent is archived".into()));
    }
    let subdir = checkout_subdir(&record, subdir)?;
    // `repo` is the host's to set: it names the checkout the delegation is
    // keyed and judged by, so a caller's own value cannot point elsewhere.
    let mut pairs: Vec<(&str, &str)> = params
        .iter()
        .filter(|(k, _)| k.as_str() != "repo")
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    if let Some(s) = &subdir {
        pairs.push(("repo", s));
    }
    let prompt = app_action_message(action, &pairs);
    let queued = sup.status_of(agent_id) == Some(AgentStatus::Running);
    let key: Key = (agent_id.to_string(), subdir);
    // Recorded before the send, so the `Running` that delivery raises finds it.
    let (id, shown) = table.record(key.clone(), kind, prompt.clone(), queued, now_ms());
    emit(ctx.sink.as_ref(), &shown);
    if !queued {
        if let Err(e) = deliver(sup, ctx, agent_id, &prompt).await {
            if let Some(gone) = table.finish(&key, id, Phase::Abandoned, Some(UNDELIVERED_NOTICE)) {
                emit(ctx.sink.as_ref(), &gone);
            }
            return Err(e);
        }
    }
    nudge();
    Ok(shown)
}

/// `delegate_git`: the `_impl` the Tauri command and the remote dispatcher
/// both call.
pub async fn delegate_git_impl(
    sup: &Arc<Supervisor>,
    ctx: &Arc<EngineCtx>,
    agent_id: &str,
    subdir: Option<&str>,
    action: &str,
    params: &BTreeMap<String, String>,
) -> Result<DelegationView> {
    delegate(global(), sup, ctx, agent_id, subdir, action, params).await
}

/// `get_delegations`: every live delegation, for a client connecting
/// mid-flight.
pub fn get_delegations_impl() -> Vec<DelegationView> {
    global().views()
}

// ---------------------------------------------------------------------------
// The driver
// ---------------------------------------------------------------------------

/// The agent's status, or `None` once it is gone — archived records included,
/// since an archived agent has no checkout to finish anything in.
fn agent_status(sup: &Supervisor, agent_id: &str) -> Option<AgentStatus> {
    let record = sup.workspace.agent(agent_id).ok()?;
    if record.archive.is_some() {
        return None;
    }
    sup.status_of(agent_id)
}

fn needs_pr(kind: DelegationKind) -> bool {
    matches!(
        kind,
        DelegationKind::CommitPr | DelegationKind::OpenPr | DelegationKind::UpdateBranch
    )
}

/// Read the checkout state a delegation is judged against. Only for one whose
/// outcome can depend on it — delivered, acked, observable — since otherwise
/// [`delegation_step`] decides without looking.
async fn observe(sup: &Supervisor, key: &Key, kind: DelegationKind) -> Observed {
    let (agent_id, subdir) = (key.0.as_str(), key.1.as_deref());
    // A checkout Fletch refuses to run git in reads as a zero state — "clean,
    // nothing unpushed" — which must not resolve anything.
    let git = crate::commands::get_git_state_impl(sup, agent_id, subdir)
        .await
        .ok()
        .flatten()
        .filter(|g| g.blocked_config.is_empty());
    let (pr, checks) = if needs_pr(kind) {
        match crate::commands::get_pr_live_impl(sup, agent_id, subdir).await {
            Ok(Some(live)) => (Some(live.state), live.checks),
            _ => (None, None),
        }
    } else {
        (None, None)
    };
    Observed { git, pr, checks }
}

/// One sweep over the table: plan, then apply each effect against the entry it
/// was decided about.
async fn run_pass(table: &Delegations, ctx: &Arc<EngineCtx>, sup: &Arc<Supervisor>) {
    let snapshot = table.snapshot();
    if snapshot.is_empty() {
        return;
    }
    let mut statuses = HashMap::new();
    let mut observed = HashMap::new();
    for (key, (_, d)) in &snapshot {
        if !statuses.contains_key(&key.0) {
            if let Some(status) = agent_status(sup, &key.0) {
                statuses.insert(key.0.clone(), status);
            }
        }
        if d.saw_git_op && !d.queued && !settles_on_idle(d.kind) {
            observed.insert(key.clone(), observe(sup, key, d.kind).await);
        }
    }
    let delegations: BTreeMap<Key, Delegation> = snapshot
        .iter()
        .map(|(k, (_, d))| (k.clone(), d.clone()))
        .collect();
    let now = now_ms();
    let sink = ctx.sink.as_ref();
    for effect in plan_pass(&delegations, &statuses, &observed, now) {
        match effect {
            Effect::Finish { key, phase, notice } => {
                if let Some(shown) = table.finish(&key, snapshot[&key].0, phase, Some(notice)) {
                    emit(sink, &shown);
                    // A fresh PR or branch update moves the merge gate; let the
                    // PR watcher read it now rather than at its next minute.
                    if phase == Phase::Done {
                        super::pr_watch::nudge();
                    }
                }
            }
            Effect::DropOrphan { key } => {
                if let Some(shown) = table.finish(&key, snapshot[&key].0, Phase::Abandoned, None) {
                    emit(sink, &shown);
                }
            }
            Effect::MarkRunning { key } => {
                if let Some(shown) = table.mark_running(&key, Some(snapshot[&key].0)) {
                    emit(sink, &shown);
                }
            }
            Effect::Dequeue { key } => {
                let id = snapshot[&key].0;
                let Some((prompt, shown)) = table.take_dequeue(&key, id, now) else {
                    continue;
                };
                emit(sink, &shown);
                if let Err(e) = deliver(sup, ctx, &key.0, &prompt).await {
                    tracing::warn!(agent_id = %key.0, error = %e, "delegation: held trigger not delivered");
                    if let Some(gone) =
                        table.finish(&key, id, Phase::Abandoned, Some(UNDELIVERED_NOTICE))
                    {
                        emit(sink, &gone);
                    }
                }
            }
        }
    }
}

/// The driver: a pass on every status transition, every git-action ack and
/// every [`TICK`] while anything is live; asleep otherwise. Each pass on its
/// own task, so a panic cannot end the loop.
pub fn spawn(ctx: Arc<EngineCtx>, sup: Arc<Supervisor>) {
    crate::host::spawn(async move {
        let mut statuses = sup.subscribe_status();
        loop {
            let live = !global().is_empty();
            tokio::select! {
                event = statuses.recv() => match event {
                    // Marked straight off the event rather than from a later
                    // status read: a short turn can be over before the pass
                    // runs, and a turn seen running is what arms the clock.
                    Ok(event) if event.status == AgentStatus::Running => {
                        for shown in global().mark_agent_running(&event.agent_id) {
                            emit(ctx.sink.as_ref(), &shown);
                        }
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                },
                _ = signal().notified() => {}
                _ = tokio::time::sleep(TICK), if live => {}
            }
            if global().is_empty() {
                continue;
            }
            let pass = {
                let (ctx, sup) = (ctx.clone(), sup.clone());
                crate::host::spawn(async move { run_pass(global(), &ctx, &sup).await }).await
            };
            if let Err(e) = pass {
                tracing::error!(error = %e, "delegation pass panicked — watching continues");
            }
        }
    });
}

#[cfg(test)]
#[path = "delegation_tests.rs"]
mod tests;
