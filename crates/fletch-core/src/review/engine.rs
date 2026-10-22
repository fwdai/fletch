//! The local review loop: a hidden reviewer agent reads a coder's checkout and
//! reports findings as local review threads; the coder answers them through
//! the ordinary `resolve-comments` playbook; a re-review follows once the code
//! moves. Runs on the host, like delegations and autopilot, so it keeps going
//! with every window shut and every client sees the same state.
//!
//! The state a client renders (`ReviewState.phase`) is *derived* on every look
//! from four facts — a reviewer turn in flight, a recorded failure, the
//! signature of the code last reviewed against the code now, and the open
//! local threads — rather than kept as a state machine of its own. That keeps
//! the UI honest by construction: a push makes a clean review stale the
//! moment the host next looks, and the last thread resolved makes findings
//! clean, with no transition anyone could forget to write.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use super::model::{Finding, ReplyAuthor};
use super::{now_ms, persist, store, Key};
use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::supervisor::{SpawnRequest, StatusEvent, Supervisor};
use crate::workspace::{AgentRecord, AgentStatus, AgentView, TrackedRepo, PURPOSE_REVIEWER};

/// One `ReviewState`, whenever a checkout's review changes.
pub const EVENT_CHANGED: &str = "review:changed";

/// `settings` key: review every PR as it opens. Absent reads as on.
pub const SETTING_ON_PR_OPEN: &str = "review.on_pr_open";
/// `settings` key: the custom agent that reviews. Absent or blank falls back
/// to the custom agent named [`DEFAULT_REVIEWER_NAME`].
pub const SETTING_AGENT: &str = "review.agent";
/// The starter pack's reviewer preset, matched by name (case-insensitive).
pub const DEFAULT_REVIEWER_NAME: &str = "Reviewer";

/// Automatic re-reviews one PR gets; a manual start is never budgeted.
pub const MAX_ROUNDS_PER_PR: u32 = 3;
/// A reviewer turn longer than this is abandoned, loudly.
pub const WALL_TIMEOUT_MS: i64 = 20 * 60 * 1000;
/// A PR opened longer ago than this that the watcher first sees (a host
/// restart) is not news, and not reviewed on sight.
const PR_OPENED_RECENTLY_MS: i64 = 15 * 60 * 1000;
/// The driver's look at every tracked checkout.
const TICK: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    #[default]
    Idle,
    Reviewing,
    Findings,
    Clean,
    Stale,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Trigger {
    Manual,
    PrOpen,
    ReReview,
}

/// The reviewer a checkout has: the custom agent it was built from and the
/// hidden workspace that runs it (`agent_id`, once spawned).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewerView {
    pub custom_agent_id: String,
    pub name: String,
    pub provider: String,
    pub model: Option<String>,
    pub agent_id: Option<String>,
}

/// What a client renders for one checkout; the `review:changed` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewState {
    pub agent_id: String,
    pub subdir: Option<String>,
    pub phase: Phase,
    pub trigger: Option<Trigger>,
    pub reviewer: Option<ReviewerView>,
    pub round: u32,
    pub open: u32,
    pub resolved: u32,
    pub since_ms: i64,
    pub reviewed_signature: Option<String>,
    pub error: Option<String>,
}

/// A reviewer turn in progress. In memory only: a turn does not survive a
/// restart, and a restart turns the persisted `reviewing` into `failed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flight {
    pub reviewer_agent_id: String,
    pub started_ms: i64,
    pub submitted: bool,
    /// The code under review, as [`signature`] spells it.
    pub signature: String,
    pub saw_running: bool,
}

/// One checkout's durable review memory (`review_state`), plus its flight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Tracked {
    pub reviewer: Option<ReviewerView>,
    /// Reviews run on this checkout, ever.
    pub round: u32,
    /// The PR the automatic rounds are counted against, and that count.
    pub pr_number: Option<u32>,
    pub pr_rounds: u32,
    pub reviewed_signature: Option<String>,
    pub trigger: Option<Trigger>,
    /// Why the last review failed; cleared by the next start.
    pub failed: Option<String>,
    /// A note that changes no phase (a refused automatic start); cleared by
    /// the next start.
    pub error: Option<String>,
    pub since_ms: i64,
    /// The phase last derived, so a restart can tell `reviewing` was cut off.
    pub phase: Phase,
    #[serde(skip)]
    pub flight: Option<Flight>,
    /// A start has claimed this checkout and is spawning its reviewer: as
    /// exclusive as a flight, before the flight exists.
    #[serde(skip)]
    pub starting: bool,
    /// What the last scan saw, so a look that scans nothing (`get_reviews`,
    /// a tick on a quiet checkout) still answers from facts.
    #[serde(skip)]
    pub last_obs: Option<Observed>,
    /// Until when the checkout counts as active (the coder's last turn) and
    /// is scanned every tick even without an open PR.
    #[serde(skip)]
    pub active_until_ms: i64,
}

impl Tracked {
    /// The phase the facts add up to.
    pub fn derive(&self, current_signature: Option<&str>, open: u32) -> Phase {
        if self.flight.is_some() {
            return Phase::Reviewing;
        }
        if self.failed.is_some() {
            return Phase::Failed;
        }
        let Some(reviewed) = self.reviewed_signature.as_deref() else {
            return Phase::Idle;
        };
        // An unreadable checkout is not a changed one.
        if current_signature.is_some_and(|now| now != reviewed) {
            return Phase::Stale;
        }
        if open > 0 {
            Phase::Findings
        } else {
            Phase::Clean
        }
    }

    /// Claim the checkout for a start about to run. Refused while a round is
    /// in flight or another start holds the claim; [`Self::begin`] or
    /// [`Self::release`] ends it.
    pub fn claim(&mut self) -> std::result::Result<(), &'static str> {
        if self.flight.is_some() || self.starting {
            return Err("a review is already running on this checkout");
        }
        self.starting = true;
        Ok(())
    }

    /// A claimed start that never began.
    pub fn release(&mut self) {
        self.starting = false;
    }

    /// Open a round. Refused while one is in flight.
    pub fn begin(
        &mut self,
        reviewer: ReviewerView,
        trigger: Trigger,
        signature: String,
        pr_number: Option<u32>,
        now: i64,
    ) -> std::result::Result<(), &'static str> {
        if self.flight.is_some() {
            return Err("a review is already running on this checkout");
        }
        self.starting = false;
        if self.pr_number != pr_number {
            self.pr_number = pr_number;
            self.pr_rounds = 0;
        }
        if trigger == Trigger::ReReview {
            self.pr_rounds += 1;
        }
        self.round += 1;
        self.failed = None;
        self.error = None;
        self.trigger = Some(trigger);
        self.flight = Some(Flight {
            reviewer_agent_id: reviewer.agent_id.clone().unwrap_or_default(),
            started_ms: now,
            submitted: false,
            signature,
            saw_running: false,
        });
        self.reviewer = Some(reviewer);
        Ok(())
    }

    /// The reviewer reported. Once per turn: the second report is refused.
    pub fn submitted(&mut self) -> std::result::Result<(), &'static str> {
        let Some(flight) = self.flight.as_mut() else {
            return Err("no review is in flight for this reviewer");
        };
        if flight.submitted {
            return Err("already submitted this turn");
        }
        flight.submitted = true;
        self.reviewed_signature = Some(flight.signature.clone());
        Ok(())
    }

    /// The reviewer's turn ended. Without a report that is a failure.
    pub fn turn_ended(&mut self) {
        let Some(flight) = self.flight.take() else {
            return;
        };
        if !flight.submitted {
            self.failed = Some("the reviewer ended its turn without submitting a review".into());
        }
    }

    pub fn fail(&mut self, why: impl Into<String>) {
        self.flight = None;
        self.starting = false;
        self.failed = Some(why.into());
    }

    /// Whether `pr_number` has had a round already — a PR seen open again
    /// (a watcher re-seed, a restart) is not news.
    pub fn reviewed_pr(&self, pr_number: u32) -> bool {
        self.round > 0 && self.pr_number == Some(pr_number)
    }

    /// The user stopped it: not a failure, just no review from this round.
    pub fn stop(&mut self) {
        self.flight = None;
        self.error = Some("review stopped".into());
    }

    /// Whether an automatic round is still within this PR's budget.
    pub fn auto_budget_left(&self, pr_number: u32) -> bool {
        self.pr_number != Some(pr_number) || self.pr_rounds < MAX_ROUNDS_PER_PR
    }
}

/// The host's tracked checkouts and what each was last announced as.
#[derive(Default)]
pub struct Table {
    inner: Mutex<BTreeMap<Key, Tracked>>,
    emitted: Mutex<BTreeMap<Key, ReviewState>>,
}

impl Table {
    pub fn keys(&self) -> Vec<Key> {
        self.inner.lock().keys().cloned().collect()
    }

    pub fn get(&self, key: &Key) -> Option<Tracked> {
        self.inner.lock().get(key).cloned()
    }

    /// Apply `f` to a checkout's memory, creating it on first touch.
    pub fn update<T>(&self, key: &Key, f: impl FnOnce(&mut Tracked) -> T) -> T {
        f(self.inner.lock().entry(key.clone()).or_default())
    }

    /// Apply `f` only to a tracked checkout.
    pub fn update_existing<T>(&self, key: &Key, f: impl FnOnce(&mut Tracked) -> T) -> Option<T> {
        self.inner.lock().get_mut(key).map(f)
    }

    pub fn remove(&self, key: &Key) -> Option<Tracked> {
        self.emitted.lock().remove(key);
        self.inner.lock().remove(key)
    }

    /// The checkout whose reviewer turn `reviewer_agent_id` is running.
    pub fn key_of_reviewer(&self, reviewer_agent_id: &str) -> Option<Key> {
        self.inner
            .lock()
            .iter()
            .find(|(_, t)| {
                t.flight
                    .as_ref()
                    .is_some_and(|f| f.reviewer_agent_id == reviewer_agent_id)
            })
            .map(|(k, _)| k.clone())
    }

    /// Every checkout of `agent_id` that has a reviewer workspace.
    pub fn reviewers_of(&self, agent_id: &str) -> Vec<(Key, String)> {
        self.inner
            .lock()
            .iter()
            .filter(|(k, _)| k.0 == agent_id)
            .filter_map(|(k, t)| {
                t.reviewer
                    .as_ref()
                    .and_then(|r| r.agent_id.clone())
                    .map(|id| (k.clone(), id))
            })
            .collect()
    }

    fn keys_of(&self, agent_id: &str) -> Vec<Key> {
        self.inner
            .lock()
            .keys()
            .filter(|k| k.0 == agent_id)
            .cloned()
            .collect()
    }
}

/// The host's one table. Global like the delegation table: the reviewer's
/// mailbox op reaches it with no supervisor in hand.
pub fn global() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(Table::default)
}

/// The code under review, as staleness measures it: HEAD plus every dirty
/// path with its kind and line counts, so an edit to a file that was already
/// dirty moves it too. Not autopilot's `state_signature`, which folds the
/// threads in — a finding resolved must not read as the code having moved.
pub fn signature(git: &crate::git_state::GitState) -> String {
    let mut files: Vec<String> = git
        .files
        .iter()
        .map(|f| format!("{}:{:?}:+{}-{}", f.path, f.kind, f.additions, f.deletions))
        .collect();
    files.sort_unstable();
    format!(
        "{}|{}",
        git.head_sha.as_deref().unwrap_or(""),
        files.join(",")
    )
}

/// How long after the coder's last turn a checkout with no open PR keeps
/// being scanned for staleness; past it, only a look at it (`get_review`)
/// or a PR opening scans it again.
const ACTIVE_WINDOW_MS: i64 = 2 * 60 * 1000;

// ── reviewer resolution ──────────────────────────────────────────────────────

/// The custom agent that reviews, as its row has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerSpec {
    pub custom_agent_id: String,
    pub name: String,
    pub base: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

fn spec_of(row: &serde_json::Map<String, serde_json::Value>) -> Option<ReviewerSpec> {
    let text = |k: &str| {
        row.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(ReviewerSpec {
        custom_agent_id: text("id")?,
        name: text("name").unwrap_or_else(|| DEFAULT_REVIEWER_NAME.to_string()),
        base: text("base")?,
        model: text("model"),
        effort: text("effort"),
    })
}

fn custom_agent_by_id(conn: &rusqlite::Connection, id: &str) -> Option<ReviewerSpec> {
    crate::database::db_select(conn, "custom_agents", json!({ "where": { "id": id } }))
        .ok()?
        .first()
        .and_then(spec_of)
}

/// The reviewer to use: `requested`, else the `review.agent` setting, else
/// the newest custom agent named "Reviewer" — the first that resolves.
pub fn resolve_reviewer(
    conn: &rusqlite::Connection,
    requested: Option<&str>,
) -> Option<ReviewerSpec> {
    fn clean(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    if let Some(spec) = clean(requested).and_then(|id| custom_agent_by_id(conn, id)) {
        return Some(spec);
    }
    let setting = crate::database::get_setting(conn, SETTING_AGENT);
    if let Some(spec) = clean(setting.as_deref()).and_then(|id| custom_agent_by_id(conn, id)) {
        return Some(spec);
    }
    let rows = crate::database::db_select(
        conn,
        "custom_agents",
        json!({ "orderBy": "updated_at", "orderDirection": "desc" }),
    )
    .ok()?;
    rows.iter()
        .filter(|r| {
            r.get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|n| n.trim().eq_ignore_ascii_case(DEFAULT_REVIEWER_NAME))
        })
        .find_map(spec_of)
}

/// Whether PRs are reviewed as they open. On unless switched off.
pub fn on_pr_open(conn: &rusqlite::Connection) -> bool {
    !matches!(
        crate::database::get_setting(conn, SETTING_ON_PR_OPEN).as_deref(),
        Some("false") | Some("0")
    )
}

// ── observation ──────────────────────────────────────────────────────────────

/// The facts one checkout's phase is derived from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    /// `None` when the checkout was not scanned or can't be read.
    signature: Option<String>,
    open: u32,
    resolved: u32,
    pr_number: Option<u32>,
    pr_open: bool,
}

fn repo_for<'a>(record: &'a AgentRecord, key: &Key) -> Option<&'a TrackedRepo> {
    match &key.1 {
        None => record.repos.first(),
        Some(subdir) => record.repos.iter().find(|r| &r.subdir == subdir),
    }
}

/// The checkout's facts. `with_git` runs the git scan (several processes);
/// without it the signature is the last scan's, and the rest is read from
/// the database alone.
async fn observe(sup: &Supervisor, key: &Key, with_git: bool) -> Observed {
    let signature = if with_git {
        crate::commands::get_git_state_impl(sup, &key.0, key.1.as_deref())
            .await
            .ok()
            .flatten()
            .filter(|g| g.blocked_config.is_empty())
            .map(|g| signature(&g))
    } else {
        global()
            .get(key)
            .and_then(|t| t.last_obs)
            .and_then(|o| o.signature)
    };
    let (open, resolved) = {
        let conn = sup.workspace.db().lock();
        store::counts(&conn, key).unwrap_or((0, 0))
    };
    let (pr_number, pr_open) = sup
        .workspace
        .agent(&key.0)
        .ok()
        .and_then(|r| repo_for(&r, key).cloned())
        .map(|repo| {
            (
                repo.pr_number.map(|n| n as u32),
                repo.pr_state.as_deref() == Some("open"),
            )
        })
        .unwrap_or((None, false));
    Observed {
        signature,
        open,
        resolved,
        pr_number,
        pr_open,
    }
}

fn view(key: &Key, tracked: &Tracked, phase: Phase, open: u32, resolved: u32) -> ReviewState {
    ReviewState {
        agent_id: key.0.clone(),
        subdir: key.1.clone(),
        phase,
        trigger: tracked.trigger,
        reviewer: tracked.reviewer.clone(),
        round: tracked.round,
        open,
        resolved,
        since_ms: tracked.since_ms,
        reviewed_signature: tracked.reviewed_signature.clone(),
        error: tracked.failed.clone().or_else(|| tracked.error.clone()),
    }
}

/// Settle a tracked checkout's phase from `obs`, and announce and persist it
/// when anything a client renders changed. The view, or `None` when the
/// checkout is not tracked.
fn settle(ctx: &Arc<EngineCtx>, key: &Key, obs: Observed) -> Option<ReviewState> {
    let table = global();
    let now = now_ms();
    let state = table.update_existing(key, |t| {
        let phase = t.derive(obs.signature.as_deref(), obs.open);
        if phase != t.phase {
            t.phase = phase;
            t.since_ms = now;
        }
        let state = view(key, t, phase, obs.open, obs.resolved);
        t.last_obs = Some(obs);
        state
    })?;
    let changed = {
        let mut emitted = table.emitted.lock();
        if emitted.get(key) == Some(&state) {
            false
        } else {
            emitted.insert(key.clone(), state.clone());
            true
        }
    };
    if changed {
        if let Some(t) = table.get(key) {
            let conn = ctx.db.lock();
            if let Err(e) = persist::save(&conn, key, &t) {
                tracing::warn!(error = %e, agent_id = %key.0, "review state not persisted");
            }
        }
        crate::host::emit(ctx.sink.as_ref(), EVENT_CHANGED, &state);
    }
    Some(state)
}

/// [`settle`] from a fresh git scan of the checkout.
async fn scan(ctx: &Arc<EngineCtx>, sup: &Supervisor, key: &Key) -> Option<ReviewState> {
    global().get(key)?;
    let obs = observe(sup, key, true).await;
    settle(ctx, key, obs)
}

/// [`settle`] without a git scan: the last scan's signature, the database's
/// counts and PR. For looks that must be cheap.
async fn refresh(ctx: &Arc<EngineCtx>, sup: &Supervisor, key: &Key) -> Option<ReviewState> {
    global().get(key)?;
    let obs = observe(sup, key, false).await;
    settle(ctx, key, obs)
}

// ── the reviewer workspace ───────────────────────────────────────────────────

/// The reviewer workspace for `coder`'s checkout `repo`, reused when the one
/// on record is live and built from the same custom agent, else spawned
/// adopting the coder's checkout.
async fn ensure_reviewer(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    coder: &AgentRecord,
    repo: &TrackedRepo,
    spec: &ReviewerSpec,
    existing: Option<&ReviewerView>,
) -> Result<ReviewerView> {
    if let Some(id) = existing.and_then(|r| r.agent_id.as_deref()) {
        match sup.workspace.agent(id) {
            Ok(record)
                if record.archive.is_none()
                    && record.custom_agent_id.as_deref() == Some(spec.custom_agent_id.as_str()) =>
            {
                if !sup.agents.lock().contains_key(id) {
                    sup.clone().resume_agent(ctx.clone(), id).await?;
                }
                return Ok(ReviewerView {
                    custom_agent_id: spec.custom_agent_id.clone(),
                    name: spec.name.clone(),
                    provider: record.provider.clone(),
                    model: record.model.clone(),
                    agent_id: Some(id.to_string()),
                });
            }
            Ok(record) if record.archive.is_none() => {
                // Built from another reviewer: replaced, not reused.
                if let Err(e) = sup.clone().discard_agent(id).await {
                    tracing::warn!(error = %e, reviewer = id, "old reviewer not discarded");
                }
            }
            _ => {}
        }
    }
    let profile = {
        let conn = ctx.db.lock();
        crate::agent_profile::library::resolve_custom_agent(
            &conn,
            &spec.custom_agent_id,
            &spec.base,
        )
    };
    let checkout = repo.checkout_path(&coder.id)?;
    let coder_name = coder.title.clone().unwrap_or_else(|| coder.name.clone());
    let req = SpawnRequest {
        view: AgentView::Custom,
        repo_path: repo.repo_path.clone(),
        provider: spec.base.clone(),
        name: None,
        effort: spec.effort.clone(),
        model: spec.model.clone(),
        instructions: profile.as_ref().and_then(|p| p.instructions.clone()),
        handoff: None,
        lineage: None,
        custom_agent_id: Some(spec.custom_agent_id.clone()),
        skills: profile
            .as_ref()
            .map(|p| p.skills.clone())
            .unwrap_or_default(),
        mcp_servers: profile
            .as_ref()
            .map(|p| p.mcp_servers.clone())
            .unwrap_or_default(),
        fork_base: Some(repo.base_branch().await),
        run_repo: None,
        owner_run_id: None,
        existing_workspace: Some(checkout),
        code_from: None,
        issue_ref: None,
        purpose: Some(PURPOSE_REVIEWER.to_string()),
        task: Some(format!("Review {coder_name}'s change")),
        account: None,
    };
    let record = sup.clone().spawn_agent(ctx.clone(), req).await?;
    Ok(ReviewerView {
        custom_agent_id: spec.custom_agent_id.clone(),
        name: spec.name.clone(),
        provider: record.provider.clone(),
        model: record.model.clone(),
        agent_id: Some(record.id),
    })
}

/// What one round asks: the change to read and how to report, plus the
/// threads still open from earlier rounds so they are judged, not repeated.
async fn round_prompt(
    sup: &Supervisor,
    coder: &AgentRecord,
    repo: &TrackedRepo,
    key: &Key,
    round: u32,
    trigger: Trigger,
) -> String {
    let checkout = repo.checkout_path(&coder.id).ok();
    let base = match &checkout {
        Some(c) => Some(repo.resolve_base(c).await),
        None => None,
    };
    let git = crate::commands::get_git_state_impl(sup, &coder.id, key.1.as_deref())
        .await
        .ok()
        .flatten();
    let base_name = base
        .as_ref()
        .map(|b| b.name.clone())
        .unwrap_or_else(|| repo.parent_branch.clone().unwrap_or_default());
    let base_sha = base
        .as_ref()
        .and_then(|b| b.fork_point.clone().or_else(|| b.tip.clone()))
        .unwrap_or_else(|| base_name.clone());
    let head = git
        .as_ref()
        .and_then(|g| g.head_sha.clone())
        .unwrap_or_else(|| "HEAD".into());
    let dirty: Vec<String> = git
        .as_ref()
        .map(|g| g.files.iter().map(|f| f.path.clone()).collect())
        .unwrap_or_default();
    let open = {
        let conn = sup.workspace.db().lock();
        store::unresolved_rows(&conn, key).unwrap_or_default()
    };

    let mut text = String::new();
    text.push_str(&format!(
        "[review] round {round} — {}\n\n",
        match trigger {
            Trigger::Manual => "requested by the user",
            Trigger::PrOpen => "the pull request was opened",
            Trigger::ReReview => "the code changed since your last review",
        }
    ));
    text.push_str(&format!(
        "Review the change in this checkout. Base: `{base_name}` at `{base_sha}`. Head: `{head}`.\n\
         See the committed change with `git diff {base_sha}...HEAD` and `git log {base_sha}..HEAD`"
    ));
    if dirty.is_empty() {
        text.push_str("; the working tree is clean.\n");
    } else {
        text.push_str(&format!(
            "; the working tree also has uncommitted changes in {} (`git status`, `git diff`), which are part of the change.\n",
            dirty.join(", ")
        ));
    }
    if open.is_empty() {
        text.push_str("\nNo earlier findings are open.\n");
    } else {
        text.push_str(
            "\nStill open from earlier rounds (do not report these again; judge the new code):\n",
        );
        for row in &open {
            let anchor = match (&row.path, row.line) {
                (Some(p), Some(l)) => format!("{p}:{l}"),
                (Some(p), None) => p.clone(),
                _ => "unanchored".into(),
            };
            let replied = if row.we_replied_last() {
                " — the author replied without resolving"
            } else {
                ""
            };
            text.push_str(&format!(
                "- [{}] {} ({anchor}){replied}\n",
                row.id, row.title
            ));
        }
    }
    text.push_str(
        "\nReport exactly once with the `review_submit` op as your instructions describe, then reply in one line.",
    );
    text
}

// ── the ops ──────────────────────────────────────────────────────────────────

/// Start a review of `agent_id`'s checkout `subdir`.
pub async fn start(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
    custom_agent_id: Option<&str>,
    trigger: Trigger,
) -> Result<ReviewState> {
    let coder = sup.workspace.agent(agent_id)?;
    if coder.archive.is_some() {
        return Err(Error::Other("agent is archived".into()));
    }
    let key = super::checkout_key(&coder, subdir)?;
    let repo = repo_for(&coder, &key)
        .cloned()
        .ok_or_else(|| Error::Other("agent has no tracked repos".into()))?;
    let table = global();
    // Claimed first, before anything awaits: a second start meanwhile — the
    // watcher and the agent's own op both announcing one PR, a tick and a
    // click — is refused here rather than racing to the spawn.
    table
        .update(&key, Tracked::claim)
        .map_err(|_| Error::Other("A review is already running on this checkout".into()))?;
    let released = |why: Error| {
        table.update(&key, Tracked::release);
        why
    };
    let spec = {
        let conn = ctx.db.lock();
        resolve_reviewer(&conn, custom_agent_id)
    }
    .ok_or_else(|| {
        released(Error::Other(
            "No reviewer is set up — pick one in Settings › Review".into(),
        ))
    })?;
    let obs = observe(sup, &key, true).await;
    let Some(signature) = obs.signature.clone() else {
        return Err(released(Error::Other(
            "the checkout can't be read yet; try again shortly".into(),
        )));
    };

    let existing = table.get(&key).and_then(|t| t.reviewer);
    let placeholder = ReviewerView {
        custom_agent_id: spec.custom_agent_id.clone(),
        name: spec.name.clone(),
        provider: spec.base.clone(),
        model: spec.model.clone(),
        agent_id: existing.as_ref().and_then(|r| r.agent_id.clone()),
    };
    let now = now_ms();
    table
        .update(&key, |t| {
            t.active_until_ms = now + ACTIVE_WINDOW_MS;
            t.begin(placeholder, trigger, signature, obs.pr_number, now)
        })
        .map_err(|e| released(Error::Other(e.into())))?;
    settle(ctx, &key, obs);

    let reviewer = match ensure_reviewer(ctx, sup, &coder, &repo, &spec, existing.as_ref()).await {
        Ok(reviewer) => reviewer,
        Err(e) => {
            table.update(&key, |t| {
                t.fail(format!("the reviewer could not start: {e}"))
            });
            refresh(ctx, sup, &key).await;
            return Err(e);
        }
    };
    let reviewer_id = reviewer.agent_id.clone().unwrap_or_default();
    table.update(&key, |t| {
        if let Some(f) = t.flight.as_mut() {
            f.reviewer_agent_id = reviewer_id.clone();
        }
        t.reviewer = Some(reviewer.clone());
    });
    let prompt = round_prompt(
        sup,
        &coder,
        &repo,
        &key,
        table.get(&key).map_or(1, |t| t.round),
        trigger,
    )
    .await;
    let turn_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = sup
        .clone()
        .send_user_message(ctx, &reviewer_id, &turn_id, &prompt, &[])
        .await
    {
        table.update(&key, |t| {
            t.fail(format!("the review could not be sent: {e}"))
        });
        refresh(ctx, sup, &key).await;
        return Err(e);
    }
    refresh(ctx, sup, &key)
        .await
        .ok_or_else(|| Error::Other("review state vanished".into()))
}

/// Stop the reviewer turn running on `agent_id`'s checkout, if any.
pub async fn stop(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<ReviewState> {
    let coder = sup.workspace.agent(agent_id)?;
    let key = super::checkout_key(&coder, subdir)?;
    let table = global();
    let reviewer = table
        .get(&key)
        .and_then(|t| t.flight.map(|f| f.reviewer_agent_id));
    if let Some(id) = reviewer {
        if !id.is_empty() {
            if let Err(e) = sup.clone().stop_agent(ctx.clone(), &id).await {
                tracing::warn!(error = %e, reviewer = %id, "reviewer not interrupted");
            }
        }
        table.update(&key, Tracked::stop);
    }
    refresh(ctx, sup, &key)
        .await
        .ok_or_else(|| Error::Other("no review on this checkout".into()))
}

pub async fn get(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
) -> Result<Option<ReviewState>> {
    let coder = sup.workspace.agent(agent_id)?;
    let key = super::checkout_key(&coder, subdir)?;
    Ok(scan(ctx, sup, &key).await)
}

/// Every tracked checkout, from the last scan: the next tick corrects a
/// few seconds' drift, and a client connecting must not cost a git scan per
/// checkout.
pub async fn all(ctx: &Arc<EngineCtx>, sup: &Arc<Supervisor>) -> Vec<ReviewState> {
    let mut out = Vec::new();
    for key in global().keys() {
        if let Some(state) = refresh(ctx, sup, &key).await {
            out.push(state);
        }
    }
    out
}

/// The user dismissed a local thread: resolved as theirs, the reason kept.
pub async fn dismiss_thread(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
    thread_id: &str,
    reason: Option<&str>,
) -> Result<()> {
    let coder = sup.workspace.agent(agent_id)?;
    let key = super::checkout_key(&coder, subdir)?;
    {
        let conn = ctx.db.lock();
        let Some(row) = store::get(&conn, thread_id)? else {
            return Err(Error::Other(format!(
                "no local review thread {thread_id:?}"
            )));
        };
        if row.agent_id != key.0 || row.subdir != key.1 {
            return Err(Error::Other(format!(
                "thread {thread_id:?} is not on this checkout"
            )));
        }
        let note = reason
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("dismissed");
        store::reply(&conn, thread_id, ReplyAuthor::User, note)?;
        store::resolve(&conn, thread_id, ReplyAuthor::User)?;
    }
    super::emit_threads_changed(ctx, &key.0, key.1.as_deref(), Vec::new()).await;
    refresh(ctx, sup, &key).await;
    Ok(())
}

/// The reviewer's report (`review_submit`), by the reviewer's own agent id.
pub async fn submit(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    reviewer_agent_id: &str,
    verdict: &str,
    findings: Vec<Finding>,
) -> Result<()> {
    let record = sup.workspace.agent(reviewer_agent_id)?;
    if record.purpose.as_deref() != Some(PURPOSE_REVIEWER) {
        return Err(Error::Other(
            "review_submit is for the reviewer agent only".into(),
        ));
    }
    let table = global();
    let key = table
        .key_of_reviewer(reviewer_agent_id)
        .ok_or_else(|| Error::Other("no review is in flight for this reviewer".into()))?;
    // The findings decide: an approval with findings is a revise, as a revise
    // with none is an approval — nothing reported is ever dropped.
    if !matches!(verdict, "approve" | "revise") {
        return Err(Error::Other(format!(
            "verdict must be \"approve\" or \"revise\", got {verdict:?}"
        )));
    }
    let approve = findings.is_empty();
    let (round, author) = table
        .update_existing(&key, |t| {
            t.submitted().map(|()| {
                (
                    t.round,
                    t.reviewer
                        .as_ref()
                        .map(|r| r.name.clone())
                        .unwrap_or_else(|| DEFAULT_REVIEWER_NAME.into()),
                )
            })
        })
        .ok_or_else(|| Error::Other("no review is in flight for this reviewer".into()))?
        .map_err(|e| Error::Other(e.into()))?;
    if !approve {
        let ids = {
            let conn = ctx.db.lock();
            store::insert_findings(&conn, &key, round, &author, &findings)?
        };
        super::emit_threads_changed(ctx, &key.0, key.1.as_deref(), ids).await;
    }
    refresh(ctx, sup, &key).await;
    Ok(())
}

// ── triggers ─────────────────────────────────────────────────────────────────

/// A PR of `agent_id`'s checkout opened (through the agent's `open_pr`, or
/// seen by the watcher): review it, when the setting says so, nothing is in
/// flight, this PR has not had a round, and the code is not the code last
/// reviewed. `opened_at` lets a watcher's first look at a long-open PR (a
/// restart) pass quietly.
pub async fn on_pr_opened(
    ctx: &Arc<EngineCtx>,
    sup: &Arc<Supervisor>,
    agent_id: &str,
    subdir: Option<&str>,
    number: Option<u32>,
    opened_at: Option<i64>,
) {
    if opened_at.is_some_and(|at| now_ms() - at > PR_OPENED_RECENTLY_MS) {
        return;
    }
    let on = {
        let conn = ctx.db.lock();
        on_pr_open(&conn)
    };
    if !on {
        return;
    }
    let Ok(coder) = sup.workspace.agent(agent_id) else {
        return;
    };
    if coder.purpose.is_some() || coder.owner_run_id.is_some() {
        return;
    }
    let Ok(key) = super::checkout_key(&coder, subdir) else {
        return;
    };
    let tracked = global().get(&key);
    if tracked
        .as_ref()
        .is_some_and(|t| t.flight.is_some() || t.starting)
    {
        return;
    }
    if let (Some(t), Some(n)) = (&tracked, number) {
        if t.reviewed_pr(n) {
            return;
        }
    }
    // Reviewed clean or with findings at exactly this code (a review asked
    // for before the PR): nothing new to read.
    if let Some(reviewed) = tracked.as_ref().and_then(|t| t.reviewed_signature.clone()) {
        let obs = observe(sup, &key, true).await;
        if obs.signature.as_deref() == Some(reviewed.as_str()) {
            return;
        }
    }
    if let Err(e) = start(ctx, sup, agent_id, subdir, None, Trigger::PrOpen).await {
        tracing::info!(error = %e, agent_id, "review on PR open not started");
    }
}

/// The coder is being archived or discarded: its reviewers go with it, and
/// so does the checkout's review memory.
///
/// `ctx` is what an archive needs; a discard (which has none in hand) discards
/// the reviewers too.
pub async fn on_coder_gone(sup: &Arc<Supervisor>, ctx: Option<&Arc<EngineCtx>>, agent_id: &str) {
    let table = global();
    let reviewers = table.reviewers_of(agent_id);
    for key in table.keys_of(agent_id) {
        table.remove(&key);
    }
    {
        let conn = sup.workspace.db().lock();
        if let Err(e) = persist::delete_for_agent(&conn, agent_id) {
            tracing::warn!(error = %e, agent_id, "review state rows not deleted");
        }
    }
    for (_, reviewer) in reviewers {
        let Ok(record) = sup.workspace.agent(&reviewer) else {
            continue;
        };
        if ctx.is_some() && record.archive.is_some() {
            continue;
        }
        let result = clean_up_reviewer(sup.clone(), ctx.cloned(), reviewer.clone()).await;
        if let Err(e) = result {
            tracing::warn!(error = %e, reviewer = %reviewer, "reviewer not cleaned up with its coder");
        }
    }
}

/// Archive (with a ctx) or discard (without) one reviewer workspace. A boxed,
/// concrete future on purpose: the archive and the discard each come back
/// through [`on_coder_gone`] for the reviewer's own (empty) set of reviewers,
/// and an `async fn` recursing through an opaque type cannot be sent.
fn clean_up_reviewer(
    sup: Arc<Supervisor>,
    ctx: Option<Arc<EngineCtx>>,
    reviewer: String,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        match ctx {
            None => sup.discard_agent(&reviewer).await,
            Some(ctx) => {
                sup.archive_agent_as(ctx, &reviewer, crate::supervisor::ArchiveTrigger::User)
                    .await
            }
        }
    })
}

// ── the driver ───────────────────────────────────────────────────────────────

/// Load the persisted memory. A review in flight at the last shutdown is a
/// failed one: its turn went with the process.
pub fn load(ctx: &EngineCtx) {
    let rows = {
        let conn = ctx.db.lock();
        persist::load_all(&conn)
    };
    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "review state not loaded");
            return;
        }
    };
    let table = global();
    for (key, mut tracked) in rows {
        tracked.flight = None;
        if tracked.phase == Phase::Reviewing {
            tracked.failed = Some("interrupted by restart".into());
            tracked.phase = Phase::Failed;
        }
        table.update(&key, |t| *t = tracked);
    }
}

async fn on_status(ctx: &Arc<EngineCtx>, sup: &Arc<Supervisor>, event: StatusEvent) {
    let table = global();
    if let Some(key) = table.key_of_reviewer(&event.agent_id) {
        let ended = table
            .update_existing(&key, |t| {
                let Some(flight) = t.flight.as_mut() else {
                    return false;
                };
                match event.status {
                    AgentStatus::Running => {
                        flight.saw_running = true;
                        false
                    }
                    AgentStatus::Spawning => false,
                    AgentStatus::Idle if !flight.saw_running => false,
                    AgentStatus::Idle => {
                        t.turn_ended();
                        true
                    }
                    AgentStatus::Stopped => {
                        t.fail("the reviewer was stopped before it reported");
                        true
                    }
                    AgentStatus::Error => {
                        t.fail("the reviewer's turn failed");
                        true
                    }
                }
            })
            .unwrap_or(false);
        if ended {
            refresh(ctx, sup, &key).await;
        }
        return;
    }
    // A coder settling: its checkouts may have moved, or resolved a thread —
    // and stay worth scanning for a while.
    if event.status == AgentStatus::Idle {
        let until = now_ms() + ACTIVE_WINDOW_MS;
        for key in table.keys_of(&event.agent_id) {
            table.update_existing(&key, |t| t.active_until_ms = until);
            scan(ctx, sup, &key).await;
        }
    }
}

/// The status stream lagged: whatever the in-flight reviewers' turns did
/// meanwhile is read back from their current status.
async fn resync(ctx: &Arc<EngineCtx>, sup: &Arc<Supervisor>) {
    let table = global();
    for key in table.keys() {
        let Some(flight) = table.get(&key).and_then(|t| t.flight) else {
            continue;
        };
        if flight.reviewer_agent_id.is_empty() {
            continue;
        }
        let status = sup.status_of(&flight.reviewer_agent_id);
        let ended = table
            .update_existing(&key, |t| match status {
                // Reported and settled, or settled after it was seen running:
                // the turn is over. Idle without either is ambiguous (it may
                // not have started) and is left to the wall clock.
                Some(AgentStatus::Idle) if flight.submitted || flight.saw_running => {
                    t.turn_ended();
                    true
                }
                Some(AgentStatus::Error) => {
                    t.fail("the reviewer's turn failed");
                    true
                }
                Some(AgentStatus::Stopped) | None => {
                    t.fail("the reviewer went away before it reported");
                    true
                }
                _ => false,
            })
            .unwrap_or(false);
        if ended {
            refresh(ctx, sup, &key).await;
        }
    }
}

/// One look at every tracked checkout: timeouts, staleness, and the
/// automatic re-review an open PR gets while its budget lasts. A checkout is
/// git-scanned only while it has an open PR or the coder was active lately;
/// the rest settle from their last scan. A start runs in the background —
/// spawning a reviewer takes a while, and the other checkouts can't wait.
async fn tick(ctx: &Arc<EngineCtx>, sup: &Arc<Supervisor>) {
    let table = global();
    let now = now_ms();
    let on = {
        let conn = ctx.db.lock();
        on_pr_open(&conn)
    };
    for key in table.keys() {
        // The wall clock: a reported turn that never settled is over; one
        // that never reported has failed, and is interrupted.
        let timed_out = table
            .update_existing(&key, |t| match &t.flight {
                Some(f) if now - f.started_ms > WALL_TIMEOUT_MS => {
                    let id = f.reviewer_agent_id.clone();
                    if f.submitted {
                        t.turn_ended();
                        None
                    } else {
                        t.fail("the reviewer did not report within 20 minutes");
                        Some(id)
                    }
                }
                _ => None,
            })
            .flatten();
        if let Some(id) = timed_out {
            if !id.is_empty() {
                let _ = sup.clone().stop_agent(ctx.clone(), &id).await;
            }
        }
        let Some(tracked) = table.get(&key) else {
            continue;
        };
        let cheap = observe(sup, &key, false).await;
        let scan_now = tracked.flight.is_none()
            && !tracked.starting
            && (cheap.pr_open || now < tracked.active_until_ms);
        let obs = if scan_now {
            observe(sup, &key, true).await
        } else {
            cheap
        };
        let Some(state) = settle(ctx, &key, obs.clone()) else {
            continue;
        };
        if state.phase != Phase::Stale || !on {
            continue;
        }
        let Some(pr_number) = obs.pr_number.filter(|_| obs.pr_open) else {
            continue;
        };
        let coder_busy = matches!(
            sup.status_of(&key.0),
            Some(AgentStatus::Running | AgentStatus::Spawning)
        );
        if coder_busy || crate::supervisor::delegation::is_live(&key) {
            continue;
        }
        if !tracked.auto_budget_left(pr_number) {
            let why =
                format!("re-review budget spent for PR #{pr_number} ({MAX_ROUNDS_PER_PR} rounds)");
            table.update_existing(&key, |t| t.error = Some(why));
            refresh(ctx, sup, &key).await;
            continue;
        }
        let (ctx, sup, key) = (ctx.clone(), sup.clone(), key.clone());
        crate::host::spawn(async move {
            if let Err(e) = start(
                &ctx,
                &sup,
                &key.0,
                key.1.as_deref(),
                None,
                Trigger::ReReview,
            )
            .await
            {
                let why = format!("re-review not started: {e}");
                global().update_existing(&key, |t| t.error = Some(why));
                refresh(&ctx, &sup, &key).await;
            }
        });
    }
}

/// Start the driver: the persisted memory, then a look on every status
/// transition and every [`TICK`].
pub fn spawn(ctx: Arc<EngineCtx>, sup: Arc<Supervisor>) {
    load(&ctx);
    crate::host::spawn(async move {
        let mut statuses = sup.subscribe_status();
        // One clock outside the loop: a sleep built per iteration would be
        // dropped and restarted by every status event, and a busy host would
        // never tick.
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                event = statuses.recv() => match event {
                    Ok(event) => on_status(&ctx, &sup, event).await,
                    Err(RecvError::Lagged(_)) => resync(&ctx, &sup).await,
                    Err(RecvError::Closed) => return,
                },
                _ = ticker.tick() => tick(&ctx, &sup).await,
            }
        }
    });
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
