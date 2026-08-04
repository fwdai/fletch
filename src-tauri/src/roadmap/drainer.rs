//! The roadmap queue drainer: the background task that turns `queued` roadmap
//! items into running workflows, and reflects those runs back onto the board.
//!
//! # Why status is the queue
//!
//! There is no queue table. `roadmap_items.status` *is* the queue — `queued`
//! means "the user asked for this to be built", and `rank` orders it (migration
//! 0032; before that it was `created_at`, i.e. FIFO, which made "build this one
//! first" inexpressible). A second table would be a second source of truth for
//! the one fact the board already draws, and would need its own reconciliation
//! after every crash. The queue is read through [`store::list`], so the order
//! the board shows and the order this dispatches in are the same query — the
//! user's drag and the PM's accepted reordering move both at once. The horizon
//! (`now`/`next`/`later`) deliberately does **not** gate dispatch: queueing is
//! an explicit act, and the drainer never moves an item between horizons on its
//! own.
//!
//! # The tick
//!
//! Every [`TICK`] (and immediately on a [`nudge`], poked by the roadmap
//! commands so a queue action doesn't wait out the interval) the drainer, per
//! project that has roadmap work in flight:
//!
//! 1. **Settles** every `active` item against its run row ([`settle`]), and
//!    hands each settlement to the project-manager chat as a review turn
//!    ([`super::review`]) — the loop back up to the agent that wrote the brief.
//! 2. **Dispatches** at most one queued item ([`pick_next`]), if the project is
//!    under [`MAX_CONCURRENT_ROADMAP_RUNS`] live roadmap-dispatched runs.
//!
//! An item settled into `in_review` leaves the drainer's world entirely —
//! `projects_with_work` only looks at `queued`/`active`, so a board waiting on
//! reviews is inert here. [`super::merge_sweep`] owns it from there and hands it
//! back (as `done`, unblocking dependants, or as `open` if the PR was closed).
//!
//! # Locking
//!
//! Everything that reads or writes the database happens inside a
//! `parking_lot::Mutex` guard with no `.await` in scope — the guard is dropped
//! before [`WorkflowService::launch`] is awaited. That also makes the *claim*
//! atomic: the drainer re-reads the item and flips `queued → active` inside one
//! guard, so an unqueue that races the tick either lands before the claim (and
//! the item is skipped) or after it — and one that lands after is *refused*,
//! because the queue actions go through `roadmap_update_item`'s conditional path
//! (`expect_status`, i.e. [`store::update_where_status`]). A blind
//! `active → open` there would orphan the run this tick is about to launch: the
//! `run_id` write-back would land on an `open` row and [`settle_project`], which
//! only looks at `active` items, would never settle it.
//!
//! # Surfacing why an item isn't moving
//!
//! A queued item with no resolvable workflow, or one whose run failed, must say
//! so on the card rather than sitting silent. Rather than add a `blocked_note`
//! column for a fact that is only true until the next tick, the drainer emits a
//! transient `roadmap:queue-note` event ([`QueueNote`]) the board renders inline
//! on the row. Notes are de-duplicated in memory *per row version* (see [`say`]),
//! so a permanently blocked item doesn't re-emit the same string every fifteen
//! seconds, while an item the user touched hears its explanation again.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;

use super::events::{self, EventActor, EventKind, ItemEvent, TrailEntry};
use super::review;
use super::types::{ItemPatch, ItemStatus, RoadmapItem};
use super::{emit_item, emit_item_event, store, Db};
use crate::workflow::spec::{self, Spec};
use crate::workflow::types::RunStatus;

/// How many roadmap-dispatched runs one project may have in flight at once.
///
/// One, for now. An autonomous queue that opens five PRs into the same repo in
/// parallel buys throughput with merge conflicts and a review backlog the user
/// didn't ask for; one run at a time keeps every dispatch reviewable and every
/// dependency edge meaningful (the next item forks from a tree that includes
/// the last one). Raising this is a one-line change once the merge sweep can
/// keep up — nothing else in this module assumes the value is 1.
pub const MAX_CONCURRENT_ROADMAP_RUNS: usize = 1;

/// How often the drainer wakes on its own. Short enough that a queued item
/// starts within a moment of its dependency landing, long enough that an idle
/// board costs nothing. Queue actions don't wait for it — see [`nudge`].
const TICK: Duration = Duration::from_secs(15);

/// Run statuses that count as "still in flight" for the concurrency cap. The
/// terminal three (`done`/`failed`/`canceled`) free the slot.
const LIVE_RUN_STATUSES: &str = "'pending','running','paused'";

// ───────────────────────────── the nudge ────────────────────────────────

/// Wakes the drainer between ticks. A single process-wide `Notify`: there is
/// one drainer, and `notify_one` stores a permit, so a nudge that arrives while
/// the tick is running is not lost.
fn signal() -> &'static Notify {
    static SIGNAL: OnceLock<Notify> = OnceLock::new();
    SIGNAL.get_or_init(Notify::new)
}

/// Ask the drainer to re-check now. Called by the roadmap commands after any
/// mutation — queueing an item is the obvious one, but so is marking a
/// dependency `done`, which may unblock something already queued. Cheap enough
/// to call unconditionally rather than guessing which patches matter.
pub(crate) fn nudge() {
    signal().notify_one();
}

// ───────────────────────────── queue notes ──────────────────────────────

/// The `roadmap:queue-note` payload: why an item is not moving, addressed to
/// the row that isn't moving. Transient by design — nothing persists it, and
/// the next state change on the row makes it stale, which is exactly when the
/// board drops it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueueNote {
    pub item_id: String,
    pub code: String,
    pub note: String,
}

/// What was last said about one item: the note, and the `updated_at` of the row
/// version it was computed from. Both halves are the dedup key — see [`say`].
type SaidNote = (String, i64);

/// The drainer's note-dedup memory, by item id. Behind a mutex because each tick
/// body runs in its own task (panic containment, see [`spawn`]) and the map has
/// to outlive any one of them; never held across an `.await`.
type SaidNotes = Mutex<HashMap<String, SaidNote>>;

// ───────────────────────────── pure decisions ───────────────────────────

/// What the drainer will do with a project's queue this tick. Computed by
/// [`pick_next`] from a snapshot, so the decision is unit-testable without a
/// database, a tokio runtime, or a clock.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Decision {
    /// Nothing queued at all.
    Empty,
    /// The project is already at [`MAX_CONCURRENT_ROADMAP_RUNS`].
    AtCapacity,
    /// Everything queued is waiting on a dependency. Carries the head of the
    /// queue and the codes it waits on, so the caller can say so on the card.
    Blocked {
        item_id: String,
        waiting_on: Vec<String>,
    },
    /// Dispatch this item (an index into the queue slice given to [`pick_next`]).
    Dispatch(usize),
}

/// Whether every code in `deps` counts as landed.
///
/// A dep is satisfied when its item is `done`. `in_review` is *not* done: the
/// PR is open, the work isn't in the base branch, and a dependant forked now
/// would build on a tree that doesn't contain it.
///
/// A dep code that resolves to no item at all is also satisfied. The item it
/// pointed at was deleted off the board, and a deleted item never ships — so
/// waiting for it would block the dependant forever on work nobody intends to
/// do. Treating the reference as stale is the only outcome the user can act on.
pub(crate) fn unsatisfied_deps(
    deps: &[String],
    done: &HashSet<String>,
    known: &HashSet<String>,
) -> Vec<String> {
    deps.iter()
        .filter(|d| known.contains(*d) && !done.contains(*d))
        .cloned()
        .collect()
}

/// Pick the highest-priority queued item whose dependencies have all landed.
///
/// `queued` must be in *rank* order — the DAO lists by `rank, created_at, rowid`
/// (0032), which is exactly the order the board draws, so the item the user
/// dragged to the top is the item this dispatches. An item with unsatisfied deps
/// is *skipped*, never failed — its turn comes when the thing it waits on lands.
pub(crate) fn pick_next(
    queued: &[RoadmapItem],
    live_runs: usize,
    done: &HashSet<String>,
    known: &HashSet<String>,
) -> Decision {
    if queued.is_empty() {
        return Decision::Empty;
    }
    if live_runs >= MAX_CONCURRENT_ROADMAP_RUNS {
        return Decision::AtCapacity;
    }
    let mut head_block: Option<(String, Vec<String>)> = None;
    for (i, item) in queued.iter().enumerate() {
        let waiting = unsatisfied_deps(&item.deps, done, known);
        if waiting.is_empty() {
            return Decision::Dispatch(i);
        }
        head_block.get_or_insert((item.id.clone(), waiting));
    }
    // Unreachable with a non-empty queue, but expressed as a fallback rather
    // than an unwrap so a future edit can't panic here.
    match head_block {
        Some((item_id, waiting_on)) => Decision::Blocked {
            item_id,
            waiting_on,
        },
        None => Decision::Empty,
    }
}

/// Which workflow definition to run an item under: the item's own override,
/// else the project's default. There is deliberately no hardcoded fallback
/// spec — inventing a workflow for work the user queued would be a worse
/// outcome than saying "pick one", which is what the caller does with `None`.
pub(crate) fn resolve_workflow(
    item: &RoadmapItem,
    project_default: Option<&str>,
) -> Option<String> {
    item.workflow_def_id
        .clone()
        .or_else(|| project_default.map(str::to_string))
}

/// The pull request a finished run opened, as `wf_run` records it (0029).
///
/// `number` is nullable independently of the URL: the columns are written
/// together from one `PrState`, but a row written before 0029 landed (or by a
/// path that only knew the URL) can still carry a link with nothing to poll.
/// The merge sweep needs the number, so the two are kept distinct rather than
/// pretending a URL implies one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FinalizedPr {
    pub url: String,
    pub number: Option<i64>,
}

/// What an `active` item's run says should happen to the item.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Settlement {
    /// The run is still going. Leave the item alone.
    Running,
    /// The run finished and opened a PR. The work isn't merged yet, so the item
    /// is `in_review` — [`super::merge_sweep`] is what moves it to `done` once
    /// GitHub says the PR landed.
    InReview,
    /// The run finished without opening a PR (a spec with `open_pr: false`, or
    /// a finalize that only pushed). Nothing else is coming, so the item is
    /// done as far as this app can tell.
    Done,
    /// The run failed, was canceled, or its row is gone. The item goes back to
    /// `open` with the reason on the card.
    ///
    /// Deliberately **not** back to `queued`: an auto-retry loop on a failing
    /// workflow burns tokens all night and re-opens the same broken PR. Losing
    /// a run is a decision point — the user re-queues once they know why.
    Released(&'static str),
}

/// Map a roadmap-dispatched run's state onto its item. `status` is `None` when
/// the run row no longer exists (a `wf_delete_run`, or a project half-deleted
/// under us).
pub(crate) fn settle(status: Option<RunStatus>, pr: Option<&FinalizedPr>) -> Settlement {
    match status {
        None => Settlement::Released("its run was deleted"),
        Some(RunStatus::Pending) | Some(RunStatus::Running) | Some(RunStatus::Paused) => {
            Settlement::Running
        }
        Some(RunStatus::Done) if pr.is_some() => Settlement::InReview,
        Some(RunStatus::Done) => Settlement::Done,
        Some(RunStatus::Failed) => Settlement::Released("its run failed"),
        Some(RunStatus::Canceled) => Settlement::Released("its run was canceled"),
    }
}

/// The history event a settlement writes alongside its item patch — `None` for
/// a run that is still going. The `run_failed` detail is the same reason string
/// the transient queue note wraps, so the durable record and the toast never
/// tell two stories; unlike the note, this one survives a reload.
pub(crate) fn settlement_event(
    outcome: &Settlement,
    pr: Option<&FinalizedPr>,
) -> Option<(EventKind, Option<String>)> {
    match outcome {
        Settlement::Running => None,
        Settlement::InReview => Some((EventKind::PrOpened, pr.map(|p| p.url.clone()))),
        Settlement::Done => Some((EventKind::Shipped, None)),
        Settlement::Released(why) => Some((EventKind::RunFailed, Some((*why).to_string()))),
    }
}

/// The task brief a dispatched run receives, built from the item.
///
/// A superset of the card's "Send to an agent" prompt (`ItemCard.briefFor`):
/// same code/title/why/acceptance shape, plus the two things a *non-interactive*
/// run needs and a human in a chat doesn't — what already landed underneath it,
/// and the instruction to stamp the code on the PR so the board can find its
/// way back to the work.
pub(crate) fn build_brief(item: &RoadmapItem, deps: &[&RoadmapItem]) -> String {
    let mut lines = vec![format!("{}: {}", item.code, item.title)];
    if !item.why.trim().is_empty() {
        lines.push(String::new());
        lines.push(item.why.trim().to_string());
    }
    if !item.accept.is_empty() {
        lines.push(String::new());
        lines.push("Done when:".to_string());
        lines.extend(item.accept.iter().map(|a| format!("- [ ] {a}")));
    }
    if !deps.is_empty() {
        lines.push(String::new());
        lines.push("Builds on work that has already landed:".to_string());
        lines.extend(
            deps.iter()
                .map(|d| format!("- {}: {} (done)", d.code, d.title)),
        );
    }
    lines.push(String::new());
    lines.push(format!(
        "Reference [{}] in the pull request title and description so this item \
         can be tracked back to the roadmap.",
        item.code
    ));
    lines.join("\n")
}

// ───────────────────────────── the task ─────────────────────────────────

/// Everything one dispatch needs, resolved under the connection lock so the
/// launch itself can be awaited with no guard held.
struct Plan {
    item: RoadmapItem,
    definition_id: String,
    spec: Spec,
    repo_path: String,
    brief: String,
}

/// Start the drainer. Called once from setup, after the [`WorkflowService`] is
/// managed — it launches through the same service the composer does, so runs it
/// starts are ordinary runs in every other respect (resumable, cancellable,
/// visible in the sidebar).
///
/// [`WorkflowService`]: crate::workflow::scheduler::WorkflowService
pub fn spawn(app: AppHandle, db: Db, service: Arc<crate::workflow::scheduler::WorkflowService>) {
    tauri::async_runtime::spawn(async move {
        // A note is re-emitted only when it changes, so a permanently blocked
        // item says its piece once instead of every tick. Shared rather than
        // owned by this loop because each tick runs in its own task.
        let said: Arc<SaidNotes> = Arc::new(Mutex::new(HashMap::new()));
        loop {
            tokio::select! {
                _ = tokio::time::sleep(TICK) => {}
                _ = signal().notified() => {}
            }
            // Panic containment (as the scheduler's drive tasks do): the tick
            // body runs in its own task, so a panic anywhere inside it comes
            // back as a `JoinError` here instead of unwinding this loop.
            // Autonomous dispatch has to survive one bad row — silently dead
            // until the next app start is the worst possible failure mode for a
            // queue nobody is watching.
            let ticked = {
                let (app, db, service, said) =
                    (app.clone(), db.clone(), service.clone(), said.clone());
                tauri::async_runtime::spawn(async move { tick(&app, &db, &service, &said).await })
                    .await
            };
            if ticked.is_err() {
                tracing::error!("roadmap drainer tick panicked — queue processing continues");
            }
        }
    });
}

async fn tick(
    app: &AppHandle,
    db: &Db,
    service: &Arc<crate::workflow::scheduler::WorkflowService>,
    said: &SaidNotes,
) {
    for project_id in projects_with_work(db) {
        settle_project(app, db, &project_id, said);
        // At most one dispatch per project per tick: the cap is re-read from
        // the database next tick, so a burst can never exceed it.
        if let Some(plan) = claim_next(app, db, &project_id, said) {
            dispatch(app, db, service, plan).await;
        }
    }
}

/// Projects with a roadmap item the drainer could act on. Scoping the tick to
/// these keeps an install with fifty projects and one live queue from walking
/// fifty boards.
fn projects_with_work(db: &Db) -> Vec<String> {
    let conn = db.lock();
    conn.prepare(
        "SELECT DISTINCT project_id FROM roadmap_items WHERE status IN ('queued','active')",
    )
    .and_then(|mut s| {
        s.query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
    })
    .unwrap_or_default()
}

// ───────────────────────────── settlement ───────────────────────────────

/// One `active` item's run, as this tick found it.
struct Settled {
    item: RoadmapItem,
    outcome: Settlement,
    /// The PR the run opened, when it opened one — stamped onto the item so the
    /// merge sweep can poll it without re-joining to the run.
    pr: Option<FinalizedPr>,
    /// The run this item is tied to, when the item's own `run_id` didn't name
    /// it — recovered through the `wf_run.roadmap_item_id` back-link and written
    /// back onto the row.
    adopted_run_id: Option<String>,
}

/// Reflect each `active` item's run back onto the item, and ask the PM to review
/// what it did ([`super::review`] — one turn per settlement, behind a
/// per-project dial, never for a run the queue didn't dispatch).
///
/// Items with an `agent_id` and no run are left alone: that's the manual "Send
/// to an agent" hand-off, which the queue doesn't own.
///
/// An item with *neither* is a claim whose launch never finished writing back —
/// the app died between `launch` (which inserts the run with the back-link) and
/// the drainer's `run_id` write. The back-link is what makes that recoverable:
/// the run is found by item id and adopted, provided it is still live (see
/// [`dispatched_run_id`] for why a terminal one is always stale). Only if there
/// is no such run is the item released. This can't misfire on a launch that is
/// merely still in flight: the drainer is one task, and `dispatch` is awaited
/// inside the tick, so a tick never observes its own pending launch.
fn settle_project(app: &AppHandle, db: &Db, project_id: &str, said: &SaidNotes) {
    let settled: Vec<Settled> = {
        let conn = db.lock();
        let items = match store::list(&conn, project_id) {
            Ok(items) => items,
            Err(e) => {
                tracing::warn!(project_id, error = %e, "roadmap drainer: cannot read board");
                return;
            }
        };
        items
            .into_iter()
            .filter(|i| i.status == ItemStatus::Active)
            // The manual hand-off: an agent is on it, no run to settle against.
            .filter(|i| i.run_id.is_some() || i.agent_id.is_none())
            .map(|item| {
                let adopted_run_id = match &item.run_id {
                    Some(_) => None,
                    None => dispatched_run_id(&conn, &item.id),
                };
                let run_id = item.run_id.clone().or_else(|| adopted_run_id.clone());
                let status = match &run_id {
                    Some(id) => run_status(&conn, id),
                    // No run row at all — neither named nor back-linked.
                    None => None,
                };
                let pr = match status {
                    // Only a finished run can have finalized.
                    Some(RunStatus::Done) => {
                        run_id.as_deref().and_then(|id| finalized_pr(&conn, id))
                    }
                    _ => None,
                };
                let outcome = match run_id {
                    // A claim whose run never reached the database. `settle`
                    // can't tell this from a deleted run — the item can, and
                    // "never started" is the honest thing to put on the card.
                    None => Settlement::Released("its run never started"),
                    Some(_) => settle(status, pr.as_ref()),
                };
                Settled {
                    item,
                    outcome,
                    pr,
                    adopted_run_id,
                }
            })
            // A still-running item needs no write unless its link was recovered.
            .filter(|s| s.outcome != Settlement::Running || s.adopted_run_id.is_some())
            .collect()
    };

    for Settled {
        item,
        outcome,
        pr,
        adopted_run_id,
    } in settled
    {
        let patch = match &outcome {
            Settlement::Running => {
                // Recovered only: repair the link and leave the item running.
                write_item(
                    app,
                    db,
                    &item.id,
                    ItemPatch {
                        run_id: Some(adopted_run_id.clone()),
                        ..Default::default()
                    },
                );
                tracing::info!(
                    item = %item.code,
                    run = ?adopted_run_id,
                    "roadmap drainer: re-attached item to its run"
                );
                continue;
            }
            Settlement::InReview => ItemPatch {
                status: Some(ItemStatus::InReview),
                // Copied off the run row onto the item, so the item's own
                // columns are authoritative from here on: the merge sweep
                // selects on `status = 'in_review' AND pr_number IS NOT NULL`
                // and never has to join back to a run that may since have been
                // deleted, nor to a run repo that has since been cleaned up.
                pr_url: Some(pr.as_ref().map(|p| p.url.clone())),
                pr_number: Some(pr.as_ref().and_then(|p| p.number)),
                ..Default::default()
            },
            Settlement::Done => ItemPatch {
                status: Some(ItemStatus::Done),
                ..Default::default()
            },
            Settlement::Released(_) => ItemPatch {
                status: Some(ItemStatus::Open),
                // The run is over; the item is not "the thing that run is
                // doing" any more. Clearing the link keeps a re-queue from
                // settling instantly against the old, terminal run.
                run_id: Some(None),
                ..Default::default()
            },
        };
        let landed = match settlement_event(&outcome, pr.as_ref()) {
            Some((kind, detail)) => write_item_with_event(app, db, &item.id, patch, kind, detail),
            // Unreachable — `Running` bailed above — but a settlement without
            // an event must still land its patch rather than vanish.
            None => {
                write_item(app, db, &item.id, patch);
                true
            }
        };
        // Ask the PM to review what the run actually did, once per settlement.
        // Gated on the write landing: a row deleted mid-tick has no outcome to
        // review and no card to record a deferral on. Fired before the notes
        // below because it is the only step that can wake a resting session, and
        // it needs no lock held (see [`review`]).
        if landed {
            if let Some(reviewable) = review::outcome_for(&outcome, pr.as_ref()) {
                review::request(app, db, &item, &reviewable);
            }
        }
        match outcome {
            Settlement::Released(why) => {
                tracing::info!(item = %item.code, %why, "roadmap drainer: released item");
                say(app, said, &item, &format!("Back on the board — {why}."));
            }
            // A new PR to watch. The sweep sleeps while nothing is in review,
            // so it has to be told rather than left to find this on a tick.
            Settlement::InReview => {
                forget(said, &item.id);
                super::merge_sweep::nudge();
            }
            // A settled-forward item is no longer waiting on anything, so any
            // note it carried is stale.
            _ => forget(said, &item.id),
        }
    }
}

/// The *live* run dispatched for this item, newest first — the reverse of the
/// item's own `run_id`, read through the `wf_run.roadmap_item_id` back-link. This
/// is what makes a crash between `launch` and the drainer's write-back
/// recoverable; see [`settle_project`].
///
/// Terminal runs are excluded (the same live statuses the concurrency cap
/// counts), because for a claimed item with no `run_id` a terminal back-linked
/// run is *always* stale: a run that completed legitimately had its item settled
/// — with `run_id` written — before it went terminal. So a newest-but-terminal
/// back-link means this claim's run never started, and adopting it would settle
/// the item against a previous cycle's outcome (resurrecting a dead PR as
/// `in_review`, or shipping an item on the strength of last week's run). Falling
/// through to `None` puts it on the existing "its run never started" release
/// path, which is the truth.
fn dispatched_run_id(conn: &Connection, item_id: &str) -> Option<String> {
    conn.query_row(
        &format!(
            "SELECT id FROM wf_run
              WHERE roadmap_item_id = ?1 AND status IN ({LIVE_RUN_STATUSES})
              ORDER BY created_at DESC LIMIT 1"
        ),
        [item_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// A run's status, or `None` when the row is gone.
fn run_status(conn: &Connection, run_id: &str) -> Option<RunStatus> {
    conn.query_row("SELECT status FROM wf_run WHERE id = ?1", [run_id], |r| {
        r.get::<_, String>(0)
    })
    .optional()
    .ok()
    .flatten()
    .and_then(|s| RunStatus::from_db(&s))
}

/// The PR a finished run opened, read off the run row (0029).
///
/// A blank or absent URL means no PR: a `push`-only finalize, a `pr create`
/// that failed (the reason is in the run's `finalize_pr` journal event), or a
/// run that finished before the columns existed. All three settle the item
/// straight to `done` — there is nothing to review.
fn finalized_pr(conn: &Connection, run_id: &str) -> Option<FinalizedPr> {
    let (url, number): (Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT pr_url, pr_number FROM wf_run WHERE id = ?1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten()?;
    let url = url.filter(|u| !u.trim().is_empty())?;
    Some(FinalizedPr { url, number })
}

// ───────────────────────────── dispatch ─────────────────────────────────

/// What one project's queue produced this tick. Returned out of the connection
/// guard so every emit — the claimed row, or the note explaining why there
/// isn't one — happens with no lock held.
enum Claim {
    /// Nothing to do and nothing to say.
    Nothing,
    /// Something is queued but can't run yet, and the card should say why.
    Note(Box<RoadmapItem>, String),
    /// An item was claimed (already `active`) and is ready to launch. Carries
    /// the `dispatched` history event recorded with the claim, so it can be
    /// emitted once the lock is dropped.
    Claimed(Box<Plan>, ItemEvent),
}

/// Decide what to dispatch for this project and *claim* it. Returns the plan
/// for a claimed item — already flipped to `active`, so neither a second tick
/// nor another writer can pick it up.
fn claim_next(app: &AppHandle, db: &Db, project_id: &str, said: &SaidNotes) -> Option<Plan> {
    let claim = {
        let conn = db.lock();
        plan_and_claim(&conn, project_id)
    };
    match claim {
        Claim::Nothing => None,
        Claim::Note(item, text) => {
            say(app, said, &item, &text);
            None
        }
        Claim::Claimed(plan, event) => {
            // Whatever was blocking this item no longer is.
            forget(said, &plan.item.id);
            emit_item(app, &plan.item);
            emit_item_event(app, &event);
            Some(*plan)
        }
    }
}

/// The whole decision, inside one connection guard: read the board, count live
/// runs, pick, resolve, and claim. No `.await`, no emits — so the read the
/// decision is made on and the write that acts on it cannot be interleaved with
/// another writer (the app has one connection behind one mutex).
fn plan_and_claim(conn: &Connection, project_id: &str) -> Claim {
    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!(project_id, error = %e, "roadmap drainer: cannot read board");
            return Claim::Nothing;
        }
    };
    let done: HashSet<String> = items
        .iter()
        .filter(|i| i.status == ItemStatus::Done)
        .map(|i| i.code.clone())
        .collect();
    let known: HashSet<String> = items.iter().map(|i| i.code.clone()).collect();
    // An item with an `agent_id` was handed to a specific agent by hand — the
    // hand-off *is* its dispatch, so the queue must never put a second builder
    // on it. The hand-off command refuses `queued`+ items and the card hides
    // Queue on handed-off rows, so this filter is the belt to those braces:
    // it holds even for a row a typed command queued directly.
    let queued: Vec<RoadmapItem> = items
        .iter()
        .filter(|i| i.status == ItemStatus::Queued && i.agent_id.is_none())
        .cloned()
        .collect();

    let live = live_run_count(conn, project_id);
    let item = match pick_next(&queued, live, &done, &known) {
        Decision::Dispatch(i) => queued[i].clone(),
        Decision::Blocked {
            item_id,
            waiting_on,
        } => {
            return match queued.into_iter().find(|i| i.id == item_id) {
                Some(item) => Claim::Note(
                    Box::new(item),
                    format!("Waiting on {}", waiting_on.join(", ")),
                ),
                None => Claim::Nothing,
            };
        }
        // Nothing to say: an empty queue is silence, and being at capacity is
        // the drainer working as intended.
        Decision::Empty | Decision::AtCapacity => return Claim::Nothing,
    };

    let project_default = project_setting(conn, project_id, DEFAULT_WORKFLOW_KEY);
    let Some(definition_id) = resolve_workflow(&item, project_default.as_deref()) else {
        return Claim::Note(
            Box::new(item),
            "No workflow to run it under. Pick one on this item, or set the project's \
             default workflow."
                .to_string(),
        );
    };
    let Some(spec) = definition_spec(conn, &definition_id) else {
        return Claim::Note(
            Box::new(item),
            "Its workflow is missing or no longer valid — pick another.".to_string(),
        );
    };
    let Some(repo_path) = primary_repo_path(conn, project_id) else {
        return Claim::Note(
            Box::new(item),
            "This project has no repo to run in.".to_string(),
        );
    };

    // Only deps that still resolve get quoted in the brief; a stale code counts
    // as satisfied (see `unsatisfied_deps`) but has nothing to say.
    let dep_rows: Vec<&RoadmapItem> = item
        .deps
        .iter()
        .filter_map(|code| items.iter().find(|i| &i.code == code))
        .collect();
    let brief = build_brief(&item, &dep_rows);

    // The claim. Runs under the same guard the decision was made under, so an
    // unqueue that raced this tick either already landed (and the item was
    // never in `queued` above) or lands after, against an `active` row.
    match claim_item(conn, &item.id, &definition_id) {
        Ok(Some((claimed, event))) => Claim::Claimed(
            Box::new(Plan {
                item: claimed,
                definition_id,
                spec,
                repo_path,
                brief,
            }),
            event,
        ),
        Ok(None) => Claim::Nothing,
        Err(e) => {
            tracing::warn!(item = %item.code, error = %e, "roadmap drainer: claim failed");
            Claim::Nothing
        }
    }
}

/// Flip a picked item `queued → active`, pin the workflow it will run under,
/// and record the `dispatched` history event — one lock-held sequence, so the
/// claim and its record cannot disagree. `None` when the row moved (or went)
/// between the pick and here: re-read first, so a racing unqueue is honoured
/// rather than overwritten.
fn claim_item(
    conn: &Connection,
    item_id: &str,
    definition_id: &str,
) -> rusqlite::Result<Option<(RoadmapItem, ItemEvent)>> {
    match store::get(conn, item_id)? {
        Some(fresh) if fresh.status == ItemStatus::Queued => {}
        _ => return Ok(None),
    }
    let Some(claimed) = store::update(
        conn,
        item_id,
        &ItemPatch {
            status: Some(ItemStatus::Active),
            // Pin the resolved definition on the item, so the card keeps
            // showing what it actually ran under even if the project default
            // moves afterwards.
            workflow_def_id: Some(Some(definition_id.to_string())),
            ..Default::default()
        },
    )?
    else {
        return Ok(None);
    };
    let event = events::record(
        conn,
        &claimed.id,
        &claimed.project_id,
        EventActor::Drainer,
        EventKind::Dispatched,
        // What it was dispatched under — the pinned definition id, the same
        // fact `workflow_def_id` now carries.
        Some(definition_id),
    )?;
    Ok(Some((claimed, event)))
}

/// Launch the claimed item's run. The only `.await` in the tick, and no DB
/// guard is held across it.
async fn dispatch(
    app: &AppHandle,
    db: &Db,
    service: &Arc<crate::workflow::scheduler::WorkflowService>,
    plan: Plan,
) {
    let Plan {
        item,
        definition_id,
        spec,
        repo_path,
        brief,
    } = plan;
    tracing::info!(item = %item.code, %definition_id, "roadmap drainer: dispatching");

    let launched = service
        .launch(
            spec,
            brief,
            item.project_id.clone(),
            repo_path,
            Some(definition_id),
            // No base branch: `launch` resolves the spec's `finalize.pr_base`,
            // then HEAD. A queued item has no opinion about where to fork from
            // that the workflow doesn't already carry.
            None,
            None,
            Vec::new(),
            None,
            Some(item.id.clone()),
        )
        .await;

    match launched {
        Ok(run_id) => write_item(
            app,
            db,
            &item.id,
            ItemPatch {
                run_id: Some(Some(run_id)),
                ..Default::default()
            },
        ),
        Err(e) => {
            // The claim already flipped the item to `active`; nothing is going
            // to run, so hand it back rather than leaving a phantom.
            tracing::warn!(item = %item.code, error = %e, "roadmap drainer: launch failed");
            // One reason string for both channels: the transient note the card
            // shows now, and the durable `run_failed` event that still says so
            // after a reload.
            let reason = format!("Couldn't start a run — {e}");
            write_item_with_event(
                app,
                db,
                &item.id,
                ItemPatch {
                    status: Some(ItemStatus::Open),
                    ..Default::default()
                },
                EventKind::RunFailed,
                Some(reason.clone()),
            );
            emit_note(
                app,
                &QueueNote {
                    item_id: item.id.clone(),
                    code: item.code.clone(),
                    note: reason,
                },
            );
        }
    }
}

// ───────────────────────────── db helpers ───────────────────────────────

/// `project_settings` key holding a project's default workflow definition id.
/// The same key the composer writes (`src/workflows/run/projectPipeline.ts`), so
/// "the workflow this project runs" means one thing on both sides.
const DEFAULT_WORKFLOW_KEY: &str = "workflow.default";

/// One per-project setting, trimmed, with a blank treated as absent. Shared with
/// [`super::review`], which reads its own dial the same way.
pub(super) fn project_setting(conn: &Connection, project_id: &str, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
        rusqlite::params![project_id, key],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

/// Roadmap-dispatched runs still in flight for a project. Human-launched runs
/// (`roadmap_item_id IS NULL`) are not throttled by the queue and don't count.
fn live_run_count(conn: &Connection, project_id: &str) -> usize {
    conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM wf_run
              WHERE project_id = ?1 AND roadmap_item_id IS NOT NULL
                AND status IN ({LIVE_RUN_STATUSES})"
        ),
        [project_id],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
    .max(0) as usize
}

/// The project's primary repo — the first attached, mirroring
/// `WorkspaceManager::project_repo_paths`. A run targets one repo; the queue
/// picks the same one the rest of the app calls primary.
///
/// Shared with [`super::merge_sweep`], which resolves `owner/repo` from this
/// checkout's remote: it is the one path tied to the *project* rather than to a
/// run, and so is still there after a finished run's directory is cleaned up.
pub(crate) fn primary_repo_path(conn: &Connection, project_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT path FROM repos WHERE project_id = ?1 ORDER BY created_at LIMIT 1",
        [project_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .filter(|p| !p.trim().is_empty())
}

/// A definition's spec, parsed and validated. An invalid stored spec is treated
/// as a missing workflow: `launch` would fail on it anyway, and saying "pick
/// another" is more useful than a failed run.
fn definition_spec(conn: &Connection, definition_id: &str) -> Option<Spec> {
    let spec_json: String = conn
        .query_row(
            "SELECT spec_json FROM wf_definition WHERE id = ?1",
            [definition_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    let spec: Spec = serde_json::from_str(&spec_json)
        .map_err(|e| tracing::warn!(definition_id, error = %e, "unreadable workflow spec"))
        .ok()?;
    // Same validation the save/import path runs, so a definition persisted
    // before a rule existed can't reach a launch.
    if let Err(errs) = spec::validate(&spec) {
        tracing::warn!(definition_id, errors = %errs.join("; "), "invalid workflow spec");
        return None;
    }
    Some(spec)
}

/// Apply a patch and announce the row. Every drainer write goes through here
/// (or [`write_item_with_event`]), so nothing it changes can reach the database
/// without reaching the board.
pub(crate) fn write_item(app: &AppHandle, db: &Db, id: &str, patch: ItemPatch) {
    let updated = {
        let conn = db.lock();
        store::update(&conn, id, &patch)
    };
    match updated {
        Ok(Some(row)) => emit_item(app, &row),
        // The row was deleted mid-tick; nothing to announce.
        Ok(None) => {}
        Err(e) => tracing::warn!(id, error = %e, "roadmap drainer: item write failed"),
    }
}

/// [`write_item`] for a *transition*: the patch and the history event that
/// names it land in one lock scope, then both are announced. Used for every
/// drainer write that moves an item's status; link repairs and `run_id`
/// write-backs stay on [`write_item`], because they are bookkeeping, not
/// history.
///
/// Returns whether the transition landed. A settlement's follow-on work (the PM
/// review turn) hangs off that: a row deleted mid-tick has no state to describe.
fn write_item_with_event(
    app: &AppHandle,
    db: &Db,
    id: &str,
    patch: ItemPatch,
    kind: EventKind,
    detail: Option<String>,
) -> bool {
    let updated = {
        let conn = db.lock();
        apply_and_record(&conn, id, None, &patch, EventActor::Drainer, kind, detail)
    };
    match updated {
        Ok(Some((row, event))) => {
            emit_item(app, &row);
            emit_item_event(app, &event);
            true
        }
        // The row was deleted mid-tick; its events cascaded with it.
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(id, error = %e, "roadmap drainer: item write failed");
            false
        }
    }
}

/// [`write_item_with_event`], but only when the row is still in `expected`
/// status — the transition-safe variant for a verdict decided *before* a wait.
/// The merge sweep decides over a network read, so by write time the row may
/// have moved (re-queued, re-dispatched); stamping a stale verdict over that
/// would orphan the fresh work. A miss writes and announces nothing — no row,
/// and no event, because history for a write that lost its race would be false.
pub(crate) fn write_item_where(
    app: &AppHandle,
    db: &Db,
    id: &str,
    expected: ItemStatus,
    patch: ItemPatch,
    entry: TrailEntry,
) {
    let updated = {
        let conn = db.lock();
        apply_and_record(
            &conn,
            id,
            Some(expected),
            &patch,
            entry.actor,
            entry.kind,
            entry.detail,
        )
    };
    match updated {
        Ok(Some((row, event))) => {
            emit_item(app, &row);
            emit_item_event(app, &event);
        }
        // Deleted, or no longer in `expected` — either way the verdict is
        // stale and the row's current owner wins.
        Ok(None) => tracing::debug!(
            id,
            "roadmap: row moved before a verdict landed — left alone"
        ),
        Err(e) => tracing::warn!(id, error = %e, "roadmap: item write failed"),
    }
}

/// The one lock-held write behind the event-carrying helpers: apply the
/// (possibly conditional) patch, and record the event only when the patch
/// actually landed — a miss must leave no trace anywhere.
fn apply_and_record(
    conn: &Connection,
    id: &str,
    expected: Option<ItemStatus>,
    patch: &ItemPatch,
    actor: EventActor,
    kind: EventKind,
    detail: Option<String>,
) -> rusqlite::Result<Option<(RoadmapItem, ItemEvent)>> {
    let updated = match expected {
        Some(expected) => store::update_where_status(conn, id, expected, patch)?,
        None => store::update(conn, id, patch)?,
    };
    let Some(row) = updated else {
        return Ok(None);
    };
    let event = events::record(
        conn,
        &row.id,
        &row.project_id,
        actor,
        kind,
        detail.as_deref(),
    )?;
    Ok(Some((row, event)))
}

// ───────────────────────────── notes ────────────────────────────────────

pub(crate) fn emit_note(app: &AppHandle, note: &QueueNote) {
    let _ = app.emit("roadmap:queue-note", note);
}

/// Emit a note unless the same thing was already said about this same version of
/// the row.
///
/// The dedup key is the note *and* the row's `updated_at`, never the note alone.
/// A blocked item nobody touches keeps both, so it stays quiet tick after tick —
/// which is the point of the dedup. But an item that was unqueued and re-queued
/// has a bumped `updated_at` (every write bumps it), so the identical string is
/// said again: recomputing "Waiting on FLT-100" for a row the user just put back
/// on the queue and swallowing it would leave the card with no explanation at all.
fn say(app: &AppHandle, said: &SaidNotes, item: &RoadmapItem, note: &str) {
    if !record_note(&mut said.lock(), item, note) {
        return;
    }
    emit_note(
        app,
        &QueueNote {
            item_id: item.id.clone(),
            code: item.code.clone(),
            note: note.to_string(),
        },
    );
}

/// Remember a note against the row version it describes, and report whether it
/// still needs emitting. Split out of [`say`] so the rule is testable without an
/// `AppHandle`.
fn record_note(said: &mut HashMap<String, SaidNote>, item: &RoadmapItem, note: &str) -> bool {
    let entry = (note.to_string(), item.updated_at);
    if said.get(&item.id) == Some(&entry) {
        return false;
    }
    said.insert(item.id.clone(), entry);
    true
}

fn forget(said: &SaidNotes, item_id: &str) {
    said.lock().remove(item_id);
}

#[cfg(test)]
mod tests;
