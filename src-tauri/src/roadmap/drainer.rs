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
//! 2. **Dispatches** queued items ([`pick_next`]) until the project is at its
//!    concurrency cap ([`concurrency_cap`]) or nothing is claimable.
//!
//! # The dial
//!
//! One run at a time is the *default*, not the design (B3,
//! .context/roadmap-pm-plan.md): `roadmap.max_concurrent` raises it per project,
//! and [`MAX_CONCURRENT_ROADMAP_RUNS`] is what an unset dial means. The honest
//! trade is unchanged by making it settable — N parallel runs on one repo buy
//! throughput with merge conflicts between their PRs and a review backlog only
//! the user can clear, and each fork sees a tree without the others' work in it.
//! So the dial is the user's, it clamps at [`MAX_CONCURRENT_ROADMAP_CEILING`],
//! and the default keeps a board that was never configured behaving exactly as it
//! did when the number was a constant.
//!
//! Each dispatch inside one tick re-runs the *whole* decision under the
//! connection lock — the live-run count, the board read, [`dispatchable`],
//! [`pick_next`] — so raising the cap widens what a tick may do and loosens none
//! of the gates it does it through.
//!
//! # Holds
//!
//! A hold ([`super::holds`]) is the brake on step 2 only. A held item is never
//! claimed ([`dispatchable`]) and a held project dispatches nothing at all
//! ([`plan_and_claim`], through [`holds::project_gate`]) — but step 1 keeps
//! running under either, because settling is not autonomy: it is the app noticing
//! that a run it already started has finished. Refusing to reflect that would
//! leave an `active` card lying about a run that ended hours ago, which is the
//! opposite of what a brake is for.
//!
//! A hold also stops everything *downstream* of the item, and that is stated
//! rather than inferred: the dep gate counts an item as landed only when it is
//! `done` **and not held** ([`done_codes`]). The inference it replaces ("a held
//! item never reaches `done`, so its dependants wait") was false — the merge
//! sweep ships an item when GitHub says its PR merged, which a teammate or an
//! armed auto-merge can do at any time, hold or no hold.
//!
//! Holds outrank the dial in both directions: a raised cap dispatches more of
//! what is *already* dispatchable and can't reach a held row, and autoqueue
//! ([`autoqueue`], read by the accept path in [`super::update_and_record`]) lands
//! an accepted item `open` rather than `queued` while a hold stands. A hold may
//! only ever reduce autonomy (invariant 2), so the dial can never override one.
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
//!
//! A transient note is the whole story for exactly one condition, though. The
//! partition is **self-resolving vs standing**:
//!
//! - *Self-resolving*: waiting on a dependency that is still being built. It ends
//!   when that work lands, without anyone deciding anything, and it is re-derived
//!   every tick — [`Claim::note`], transient only.
//! - *Standing*: a dependency loop (nothing in a loop is ever `done`, so
//!   [`unsatisfied_deps`] never resolves for any member), no resolvable workflow,
//!   an invalid stored spec, and a project with no repo. None of those ends without
//!   the user changing something, and the note's dedup is per row version *per
//!   process lifetime* — so a queued item in a repo-less project used to say its
//!   piece once and then wedge in silence forever. All four write one durable
//!   `blocked` event through [`Claim::wedge`], which is also what surfaces them on
//!   the board's "Needs you" strip.

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
use super::{deps, emit_item, emit_item_event, holds, store, Db};
use crate::workflow::spec::{self, Spec};
use crate::workflow::types::RunStatus;

/// How many roadmap-dispatched runs one project may have in flight when nobody
/// has said otherwise — the default behind [`MAX_CONCURRENT_KEY`], not a ceiling.
///
/// One, because that is the conservative reading of an autonomous queue: every
/// dispatch stays reviewable and every dependency edge stays meaningful (the next
/// item forks from a tree that includes the last one). A project that wants
/// throughput asks for it explicitly; see the module docs for what it buys and
/// what it costs.
pub const MAX_CONCURRENT_ROADMAP_RUNS: usize = 1;

/// The highest cap the dial offers, and the clamp every read applies.
///
/// Four, and the limit is not the machine. Parallel runs land parallel pull
/// requests into **one** repo: past a handful, the merge sweep is serializing
/// merges that increasingly conflict with each other, and the review bandwidth of
/// the single human who has to rule on them is the real bottleneck. A number
/// nobody can keep up with is not more autonomy, it is a backlog.
pub const MAX_CONCURRENT_ROADMAP_CEILING: usize = 4;

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
    /// The project is already at its cap ([`concurrency_cap`]).
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

/// The codes a dependant may treat as landed: `done` **and not held**.
///
/// The second half is what makes a hold *transitive*, and it is the whole answer
/// to review finding B2 (.context/roadmap-pm-plan.md). A hold's promise is that
/// nothing downstream of it proceeds, and the original implementation of that
/// promise was indirect — a held item never reaches `done`, so its dependants
/// wait. That reasoning had a hole: the merge sweep ships an item when *GitHub*
/// says its PR merged, and a teammate (or an auto-merge armed before the hold) can
/// merge a held item's PR at any time. The item then reached `done` without
/// anything autonomous deciding it should, and every dependant unblocked.
///
/// So "done" for dependency purposes is stated directly here instead of inferred:
/// a held item does not satisfy anyone's dependency, **however it got to `done`**.
/// Dependants of a held item stop, and stay stopped until the user lifts the hold
/// — at which point this set grows and the next tick dispatches them.
///
/// Only the item's own hold is read, not the board's: a held project never reaches
/// this function at all ([`plan_and_claim`] returns before the board is read).
pub(crate) fn done_codes(items: &[RoadmapItem]) -> HashSet<String> {
    items
        .iter()
        .filter(|i| i.status == ItemStatus::Done && !i.is_held())
        .map(|i| i.code.clone())
        .collect()
}

/// Whether every code in `deps` counts as landed.
///
/// A dep is satisfied when its item counts as done ([`done_codes`]). `in_review`
/// is *not* done: the PR is open, the work isn't in the base branch, and a
/// dependant forked now would build on a tree that doesn't contain it.
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

/// The queue, out of a whole board: the `queued` items this tick may actually
/// claim, in the order the board draws them.
///
/// Three rows are queued and still not dispatchable, for reasons that are not
/// about priority or dependencies, so they are filtered out before [`pick_next`]
/// ever sees them — otherwise the head of the queue would be an item nothing can
/// launch, and every ready item behind it would wait on a decision that isn't
/// coming:
///
/// - Not `queued` at all. The queue *is* the status (see the module docs).
/// - Handed to a named agent by hand (`agent_id`). The hand-off is its dispatch,
///   so claiming it would put two builders on one brief. The hand-off command
///   refuses `queued`+ rows and the card hides Queue on handed-off ones; this is
///   the belt to those braces, and it holds even for a row a typed command
///   queued directly.
/// - **Held** ([`super::holds`]). The reason is on the row, the card says it, and
///   the user is the only one who can lift it. Same shape as the agent-linked
///   skip on purpose: a hold is a fact about the row, not a state of the queue,
///   so it is filtered here rather than encoded as a [`Decision`] — the queue is
///   not "blocked", this row is simply not in it. Dependants of a held item stop
///   too, but *not* because of this filter: see [`done_codes`], which is where the
///   transitive half of a hold is stated.
///
/// Pure over a board snapshot, so both skips are unit-testable without a
/// database. **Autoqueue (B3) dispatches through here, and through the project
/// check in [`plan_and_claim`], like everything else**: it changes only which
/// status an accepted item lands in, so an autoqueued row is an ordinary `queued`
/// row this filter reads exactly as it reads a hand-queued one. It also refuses to
/// queue a held row at all ([`super::accept_landing`]), which is belt to this
/// brace — the mode autoqueue creates is the one holds exist to make safe.
pub(crate) fn dispatchable(items: &[RoadmapItem]) -> Vec<RoadmapItem> {
    items
        .iter()
        .filter(|i| i.status == ItemStatus::Queued && i.agent_id.is_none() && !i.is_held())
        .cloned()
        .collect()
}

/// Pick the highest-priority queued item whose dependencies have all landed.
///
/// `queued` is [`dispatchable`]'s output: in *rank* order — the DAO lists by
/// `rank, created_at, rowid` (0032), which is exactly the order the board draws,
/// so the item the user dragged to the top is the item this dispatches — and
/// already stripped of the rows nothing may claim. An item with unsatisfied deps
/// is *skipped*, never failed — its turn comes when the thing it waits on lands.
///
/// `cap` is the project's [`concurrency_cap`], passed in rather than read from a
/// constant so the whole decision stays pure: one dispatch at cap 1 and up to
/// four at cap 4 are the *same* rule seen at two settings. Nothing about
/// parallelism needs new graph logic — deps already serialize dependants, so a
/// second slot can only ever go to work that was independent anyway.
pub(crate) fn pick_next(
    queued: &[RoadmapItem],
    live_runs: usize,
    cap: usize,
    done: &HashSet<String>,
    known: &HashSet<String>,
) -> Decision {
    if queued.is_empty() {
        return Decision::Empty;
    }
    if live_runs >= cap {
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

/// The five ways a claim ends with the item back on the board, as the reason
/// strings [`Settlement::Released`] carries.
///
/// Consts rather than literals at the call sites because they are read twice: once
/// as prose (the card's note, and the PM's review turn, which quotes the reason
/// verbatim) and once as a *fact* — [`release_kind`] maps each one to the event
/// kind that names it. A release whose reason is spelled somewhere else is a
/// release whose card would read "run failed" about a run nobody ran.
pub(crate) const RUN_FAILED: &str = "its run failed";
/// The user stopped it. Deliberate, and not a failure — see [`release_kind`].
pub(crate) const RUN_CANCELED: &str = "its run was canceled";
/// The run row is gone (`wf_delete_run`, or a project half-deleted under us).
pub(crate) const RUN_DELETED: &str = "its run was deleted";
/// Claimed, but the launch never wrote a run row — the crash window between
/// `claim_item` and the `run_id` write-back, with no live back-link to adopt.
pub(crate) const RUN_NEVER_STARTED: &str = "its run never started";
/// The launch itself was refused (no such spec on disk, no repo to clone, the
/// scheduler declined). Nothing ran, and the reason came from the launcher.
pub(crate) const RUN_UNLAUNCHABLE: &str = "its run couldn't be started";

/// The fact a release names, as a history kind.
///
/// Three endings, not one. `run_failed` used to carry all of them, which made the
/// card claim a failure about a run the *user* cancelled and about a run row
/// somebody deleted — and the PM, whose instructions tell it to hold an item when
/// runs keep failing, read the same flattened line (review finding S1 in
/// .context/roadmap-pm-plan.md). A user cancelling three runs is not a failing
/// pattern, and an event vocabulary that can't tell the difference manufactures
/// one.
///
/// The fallback is a failure on purpose: an unrecognized reason is one this
/// function has not been taught, and showing it as a failure is the safe way to be
/// wrong — the reason string itself is the detail either way.
pub(crate) fn release_kind(why: &str) -> EventKind {
    match why {
        RUN_CANCELED => EventKind::RunCanceled,
        RUN_DELETED => EventKind::RunDeleted,
        _ => EventKind::RunFailed,
    }
}

/// Map a roadmap-dispatched run's state onto its item. `status` is `None` when
/// the run row no longer exists (a `wf_delete_run`, or a project half-deleted
/// under us).
pub(crate) fn settle(status: Option<RunStatus>, pr: Option<&FinalizedPr>) -> Settlement {
    match status {
        None => Settlement::Released(RUN_DELETED),
        Some(RunStatus::Pending) | Some(RunStatus::Running) | Some(RunStatus::Paused) => {
            Settlement::Running
        }
        Some(RunStatus::Done) if pr.is_some() => Settlement::InReview,
        Some(RunStatus::Done) => Settlement::Done,
        Some(RunStatus::Failed) => Settlement::Released(RUN_FAILED),
        Some(RunStatus::Canceled) => Settlement::Released(RUN_CANCELED),
    }
}

/// The history event a settlement writes alongside its item patch — `None` for
/// a run that is still going. A release's detail is the same reason string the
/// transient queue note wraps, so the durable record and the toast never tell two
/// stories; unlike the note, this one survives a reload. Its *kind* is the fact
/// that reason names ([`release_kind`]).
pub(crate) fn settlement_event(
    outcome: &Settlement,
    pr: Option<&FinalizedPr>,
) -> Option<(EventKind, Option<String>)> {
    match outcome {
        Settlement::Running => None,
        Settlement::InReview => Some((EventKind::PrOpened, pr.map(|p| p.url.clone()))),
        Settlement::Done => Some((EventKind::Shipped, None)),
        Settlement::Released(why) => Some((release_kind(why), Some((*why).to_string()))),
    }
}

/// The item patch a settlement implies.
///
/// Paired with [`settlement_event`] over the same two inputs, so the row and its
/// history line are one projection of one ending rather than two open-coded
/// answers that can drift. [`Settlement::Running`] patches nothing: the caller
/// handles the run-link repair, which is bookkeeping and not an ending at all.
pub(crate) fn settlement_patch(outcome: &Settlement, pr: Option<&FinalizedPr>) -> ItemPatch {
    match outcome {
        Settlement::Running => ItemPatch::default(),
        Settlement::InReview => ItemPatch {
            status: Some(ItemStatus::InReview),
            // Copied off the run row onto the item, so the item's own columns are
            // authoritative from here on: the merge sweep selects on
            // `status = 'in_review' AND pr_number IS NOT NULL` and never has to
            // join back to a run that may since have been deleted, nor to a run
            // repo that has since been cleaned up.
            pr_url: Some(pr.map(|p| p.url.clone())),
            pr_number: Some(pr.and_then(|p| p.number)),
            ..Default::default()
        },
        Settlement::Done => ItemPatch {
            status: Some(ItemStatus::Done),
            ..Default::default()
        },
        Settlement::Released(_) => ItemPatch {
            status: Some(ItemStatus::Open),
            // The run is over; the item is not "the thing that run is doing" any
            // more. Clearing the link keeps a re-queue from settling instantly
            // against the old, terminal run.
            run_id: Some(None),
            ..Default::default()
        },
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
        let cap = {
            let conn = db.lock();
            concurrency_cap(&conn, &project_id)
        };
        // Claim until the project is full or nothing is claimable, and never more
        // than `cap` times in one tick.
        //
        // The bound is the cap for two reasons. It is enough: an idle project can
        // fill every slot in a single tick, which is what makes a raised dial
        // felt immediately rather than one item per fifteen seconds. And it is
        // *needed*: a claim whose launch fails hands its item back to the board
        // (see [`dispatch`]), so an unbounded loop would burn through a whole
        // queue of failing dispatches in one tick — at cap 1 that is exactly the
        // one-attempt-per-tick behaviour this had before the dial existed, kept.
        //
        // Sequential, and each iteration re-reads everything under the lock:
        // `dispatch` is awaited before the next claim, so the run row it inserts
        // is already counted by the next `live_run_count` and the loop cannot
        // overshoot by racing itself.
        for _ in 0..cap {
            let Some(plan) = claim_next(app, db, &project_id, cap, said) else {
                break;
            };
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
                    None => Settlement::Released(RUN_NEVER_STARTED),
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
        if outcome == Settlement::Running {
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
        conclude(app, db, &item, &outcome, pr.as_ref(), None);
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

/// End one item's turn as `active`: the patch, the durable line, and the PM's
/// review turn — in that order, once.
///
/// **Every way an item leaves `active` comes through here.** That is the point:
/// there used to be two endings, and only one of them was complete. A settlement
/// wrote its event *and* asked the PM to review the outcome; a launch that failed
/// open-coded a second ending beside it — same `run_failed` kind, same hand-back
/// to `open` — and silently skipped the review, so the one failure the PM never
/// heard about was the one where nothing ran at all (review finding S5 in
/// .context/roadmap-pm-plan.md). Making the projection *total* over the ways an
/// item stops being active is what closes that class of gap rather than the one
/// instance of it.
///
/// `detail` overrides the reason [`settlement_event`] projects, for the one caller
/// with something more specific to say: the launcher's own error. The *kind* still
/// comes from the projection, so the card can't disagree with itself about what
/// sort of ending this was.
///
/// Returns whether the write landed. A row deleted mid-tick has no state to
/// describe, so nothing follows it — no event, and no review turn.
fn conclude(
    app: &AppHandle,
    db: &Db,
    item: &RoadmapItem,
    outcome: &Settlement,
    pr: Option<&FinalizedPr>,
    detail: Option<String>,
) -> bool {
    let patch = settlement_patch(outcome, pr);
    let landed = match settlement_event(outcome, pr) {
        Some((kind, projected)) => {
            write_item_with_event(app, db, &item.id, patch, kind, detail.or(projected))
        }
        // Unreachable — `Running` is not an ending, and its one caller bails
        // before here — but an ending without an event must still land its patch
        // rather than vanish.
        None => {
            write_item(app, db, &item.id, patch);
            true
        }
    };
    // Ask the PM to review what the run actually did, once per ending. Gated on
    // the write landing: a row deleted mid-tick has no outcome to review and no
    // card to record a deferral on. Fired before the caller's notes because it is
    // the only step that can wake a resting session, and it needs no lock held
    // (see [`review`]).
    if landed {
        if let Some(reviewable) = review::outcome_for(outcome, pr) {
            review::request(app, db, item, &reviewable);
        }
    }
    landed
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
    ///
    /// `recorded` is the durable `blocked` event a **standing** blockage also
    /// writes — see [`Claim::wedge`] and [`record_wedge`]. It is `None` for the one
    /// *self-resolving* condition, an ordinary "waiting on …": that resolves itself
    /// the moment the dependency lands, so persisting it would bury the trail.
    Note {
        item: Box<RoadmapItem>,
        text: String,
        recorded: Option<ItemEvent>,
    },
    /// An item was claimed (already `active`) and is ready to launch. Carries
    /// the `dispatched` history event recorded with the claim, so it can be
    /// emitted once the lock is dropped.
    Claimed(Box<Plan>, ItemEvent),
}

impl Claim {
    /// A **self-resolving** blockage: the transient note and nothing else.
    ///
    /// One condition qualifies — waiting on a dependency that is still being built.
    /// It ends without anyone deciding anything, and it is re-derived every tick,
    /// so a durable line would be a history of "still waiting" nobody reads.
    fn note(item: RoadmapItem, text: String) -> Self {
        Claim::Note {
            item: Box::new(item),
            text,
            recorded: None,
        }
    }

    /// A **standing** blockage: the transient note *and* the durable line
    /// ([`record_wedge`], which writes it once and survives restarts).
    ///
    /// The partition is "does this resolve itself", not "is it a dependency loop".
    /// The loop was the first standing condition anyone noticed, so it got the
    /// durable line and its three siblings in this same function did not — no
    /// resolvable workflow, an invalid stored spec, a project with no repo. None of
    /// those ends on its own either, and the transient note is emitted at most once
    /// per row version *per process lifetime*, so a queued item in a repo-less
    /// project said its piece once and then wedged in silence forever (invariant 3,
    /// review finding S1 in .context/roadmap-pm-plan.md). All four come through
    /// here now, which is also what puts them on the "Needs you" strip — the strip
    /// reads `blocked` events, and a wedge nobody can see is a wedge nobody fixes.
    ///
    /// Called with the connection lock held; the event is emitted by [`claim_next`]
    /// once it drops, like every other event this module writes.
    fn wedge(conn: &Connection, item: RoadmapItem, text: String) -> Self {
        let recorded = record_wedge(conn, &item, &text);
        Claim::Note {
            item: Box::new(item),
            text,
            recorded,
        }
    }
}

/// Decide what to dispatch for this project and *claim* it. Returns the plan
/// for a claimed item — already flipped to `active`, so neither a second tick
/// nor another writer can pick it up.
fn claim_next(
    app: &AppHandle,
    db: &Db,
    project_id: &str,
    cap: usize,
    said: &SaidNotes,
) -> Option<Plan> {
    let claim = {
        let conn = db.lock();
        plan_and_claim(&conn, project_id, cap)
    };
    match claim {
        Claim::Nothing => None,
        Claim::Note {
            item,
            text,
            recorded,
        } => {
            say(app, said, &item, &text);
            // The durable half, when there was one: emitted after the lock
            // dropped, exactly like every other event this module writes.
            if let Some(event) = &recorded {
                emit_item_event(app, event);
            }
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
///
/// The project hold is the first thing checked, and it is checked *here* rather
/// than in the tick so that settlement is unaffected: the tick settles before it
/// calls this, so a held board still reflects the runs it already started
/// (see the module docs — settling is not autonomy). A held project says nothing
/// on the cards either: the reason is one banner above the board, and repeating
/// it per row would be the same sentence five times. **Autoqueue comes through
/// this function** like every other queued row: it writes a status, and this is
/// still the only thing that turns a status into a run.
///
/// `cap` is the project's dial, read once per tick by the caller and re-applied
/// here on every claim — the *count* it is compared against ([`live_run_count`])
/// is re-read inside this guard, which is the half that has to be fresh.
fn plan_and_claim(conn: &Connection, project_id: &str, cap: usize) -> Claim {
    // One authority for "is progress stopped here", fail-closed inside it: a hold
    // we can't read is not a licence to dispatch.
    if let Some(reason) = holds::project_gate(conn, project_id) {
        tracing::debug!(project_id, %reason, "roadmap drainer: project held");
        return Claim::Nothing;
    }
    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!(project_id, error = %e, "roadmap drainer: cannot read board");
            return Claim::Nothing;
        }
    };
    let done = done_codes(&items);
    let known: HashSet<String> = items.iter().map(|i| i.code.clone()).collect();
    // What this tick may actually claim — see [`dispatchable`] for the three
    // rows that are queued and still not in the queue.
    let queued = dispatchable(&items);

    let live = live_run_count(conn, project_id);
    let item = match pick_next(&queued, live, cap, &done, &known) {
        Decision::Dispatch(i) => queued[i].clone(),
        Decision::Blocked {
            item_id,
            waiting_on,
        } => {
            let Some(item) = queued.into_iter().find(|i| i.id == item_id) else {
                return Claim::Nothing;
            };
            // Two kinds of blocked, and only one of them is news. Waiting on a
            // dependency that is still being built resolves itself — the note
            // says so and nothing persists. A *loop* never resolves: every
            // member waits on the next, so this item is skipped on every tick
            // from here to the end of the app's life. That is a durable fact
            // about the board, so it lands as a `blocked` event naming the loop
            // (see .context/roadmap-pm-plan.md, A4). A dependency that was
            // *rejected* is the same kind of standing fact wearing a waiting
            // note's clothes: the code still resolves, is never `done`, and no
            // run is ever coming to make it so — only the user editing the dep
            // list ends it, so it takes the durable path too.
            return match deps::find_cycle(&deps::graph_of(&items), &item.code) {
                Some(cycle) => Claim::wedge(
                    conn,
                    item,
                    format!("Stuck in a dependency loop: {}", deps::loop_path(&cycle)),
                ),
                None => {
                    let rejected: Vec<&str> = waiting_on
                        .iter()
                        .filter(|code| {
                            items
                                .iter()
                                .any(|i| &i.code == *code && i.status == ItemStatus::Rejected)
                        })
                        .map(String::as_str)
                        .collect();
                    match rejected.as_slice() {
                        [] => Claim::note(item, format!("Waiting on {}", waiting_on.join(", "))),
                        [code] => Claim::wedge(
                            conn,
                            item,
                            format!(
                                "Waiting on {code}, which was rejected — remove or replace \
                                 that dependency."
                            ),
                        ),
                        many => Claim::wedge(
                            conn,
                            item,
                            format!(
                                "Waiting on {}, which were rejected — remove or replace \
                                 those dependencies.",
                                many.join(", ")
                            ),
                        ),
                    }
                }
            };
        }
        // Nothing to say: an empty queue is silence, and being at capacity is
        // the drainer working as intended.
        Decision::Empty | Decision::AtCapacity => return Claim::Nothing,
    };

    // The three standing conditions past dep selection. None of them resolves
    // without the user changing something, so all three take the durable path —
    // see [`Claim::wedge`] for why that is the partition rather than "is it a loop".
    let project_default = project_setting(conn, project_id, DEFAULT_WORKFLOW_KEY);
    let Some(definition_id) = resolve_workflow(&item, project_default.as_deref()) else {
        return Claim::wedge(
            conn,
            item,
            "No workflow to run it under. Pick one on this item, or set the project's \
             default workflow."
                .to_string(),
        );
    };
    let Some(spec) = definition_spec(conn, &definition_id) else {
        return Claim::wedge(
            conn,
            item,
            "Its workflow is missing or no longer valid — pick another.".to_string(),
        );
    };
    let Some(repo_path) = primary_repo_path(conn, project_id) else {
        return Claim::wedge(
            conn,
            item,
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

    // The name the card and the sidebar call this workflow, read here so the
    // history line says "Dispatched — Build & review" rather than a raw uuid.
    // The definition row is already in hand from `definition_spec`'s query; this
    // is the same row, one cheap read further.
    let workflow_name = definition_name(conn, &definition_id);

    // The claim. Runs under the same guard the decision was made under, so an
    // unqueue that raced this tick either already landed (and the item was
    // never in `queued` above) or lands after, against an `active` row.
    match claim_item(conn, &item.id, &definition_id, workflow_name.as_deref()) {
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

/// Record a standing blockage's `blocked` event — once, not once a tick.
///
/// The one durable path for every wedge, wherever it is noticed: the drainer's
/// four conditions ([`Claim::wedge`]) and the merge sweep's watched pull request
/// that stopped answering ([`super::merge_sweep`]).
///
/// The de-dup is a query rather than the in-memory [`SaidNotes`] map: that map is
/// process-local and empty after a restart, which is fine for a transient note
/// and useless for a durable table (every app start would append another
/// identical row). So the check is "is the item's *newest* event already this
/// same `blocked` line?" — newest rather than any, because a wedge that was fixed
/// and re-formed is news again, and the events in between are what say so.
///
/// Called with the connection lock held, in the same guard as the read the wedge
/// was detected from. A failure is logged and dropped: the transient note still
/// reaches the card, and refusing to dispatch anything else because history
/// couldn't be written would be a worse outcome than a missing line.
pub(crate) fn record_wedge(
    conn: &Connection,
    item: &RoadmapItem,
    detail: &str,
) -> Option<ItemEvent> {
    match events::latest_for_item(conn, &item.id) {
        Ok(Some(last))
            if last.kind == EventKind::Blocked && last.detail.as_deref() == Some(detail) =>
        {
            None
        }
        Ok(_) => events::record(
            conn,
            &item.id,
            &item.project_id,
            EventActor::Drainer,
            EventKind::Blocked,
            Some(detail),
        )
        .map_err(|e| tracing::warn!(item = %item.code, error = %e, "roadmap drainer: blocked event not recorded"))
        .ok(),
        Err(e) => {
            tracing::warn!(item = %item.code, error = %e, "roadmap drainer: cannot read item history");
            None
        }
    }
}

/// Flip a picked item `queued → active`, pin the workflow it will run under,
/// and record the `dispatched` history event — one lock-held sequence, so the
/// claim and its record cannot disagree. `None` when the row moved (or went)
/// between the pick and here: re-read first, so a racing unqueue is honoured
/// rather than overwritten.
///
/// `workflow_name` is what the event says it was dispatched under. `None` (a
/// definition renamed out from under us, or deleted between the read and here)
/// falls back to the id — a uuid on the card is poor, but an unexplained
/// dispatch line is worse.
fn claim_item(
    conn: &Connection,
    item_id: &str,
    definition_id: &str,
    workflow_name: Option<&str>,
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
        // What it was dispatched under, in the words the rest of the UI uses for
        // that workflow — this is the most common line in an item's trail, and
        // the id it pins on the row (`workflow_def_id`) is already the machine
        // half of the same fact.
        Some(workflow_name.unwrap_or(definition_id)),
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
            // shows now, and the durable event that still says so after a reload.
            let reason = format!("Couldn't start a run — {e}");
            // The same ending every other exit from `active` takes ([`conclude`]),
            // rather than a second open-coded one beside it. The launcher's error
            // is more specific than the projection's reason, so it is what the
            // durable line carries; the PM's review turn quotes the projection.
            conclude(
                app,
                db,
                &item,
                &Settlement::Released(RUN_UNLAUNCHABLE),
                None,
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

/// `project_settings` key holding how many roadmap runs this project may have in
/// flight. Absent (or unreadable) means [`MAX_CONCURRENT_ROADMAP_RUNS`]; every
/// value is clamped to `1..=`[`MAX_CONCURRENT_ROADMAP_CEILING`]. Written by the
/// Roadmap section of project settings (`RoadmapSection.tsx`, which mirrors these
/// spellings).
const MAX_CONCURRENT_KEY: &str = "roadmap.max_concurrent";

/// `project_settings` key for the autonomy dial's other half: when on, accepting
/// a proposed item lands it `queued` instead of `open`, so one click is the whole
/// distance from the PM's suggestion to a running build. Default **off** — the
/// conservative reading, and the one every existing board already has.
///
/// Read by the accept path ([`super::update_and_record`]) rather than by this
/// module: the dial decides where an accept *lands*, and dispatch then treats the
/// result as the ordinary `queued` row it is.
const AUTOQUEUE_KEY: &str = "roadmap.autoqueue";

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

/// One per-project boolean dial. `default` is what an absent row means, and also
/// what an unrecognized value means — a setting nobody can read is not a mandate
/// in either direction.
///
/// Shared by every roadmap dial ([`autoqueue`] here, `roadmap.settle_review` in
/// [`super::review`]) so "off" is spelled the same way for all of them: whatever
/// a checkbox writes (`1`/`0`), whatever a hand-edited row plausibly says
/// (`true`/`false`, `on`/`off`, `yes`/`no`).
pub(super) fn project_flag(conn: &Connection, project_id: &str, key: &str, default: bool) -> bool {
    parse_flag(project_setting(conn, project_id, key).as_deref(), default)
}

/// [`project_flag`]'s rule, without the database.
pub(crate) fn parse_flag(raw: Option<&str>, default: bool) -> bool {
    match raw.map(|v| v.to_ascii_lowercase()) {
        None => default,
        Some(v) => match v.as_str() {
            "1" | "true" | "on" | "yes" => true,
            "0" | "false" | "off" | "no" => false,
            _ => default,
        },
    }
}

/// Does accepting a proposed item queue it straight away for this project?
pub(super) fn autoqueue(conn: &Connection, project_id: &str) -> bool {
    project_flag(conn, project_id, AUTOQUEUE_KEY, false)
}

/// How many roadmap runs this project may have in flight at once.
pub(crate) fn concurrency_cap(conn: &Connection, project_id: &str) -> usize {
    parse_cap(project_setting(conn, project_id, MAX_CONCURRENT_KEY).as_deref())
}

/// [`concurrency_cap`]'s rule, without the database: parse, clamp, and fall back.
///
/// Garbage falls back to the default rather than to zero or to the ceiling. Zero
/// would stop the queue with no hold to explain it (that is what holds are for,
/// and they are visible); the ceiling would answer a typo with four parallel runs.
pub(crate) fn parse_cap(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(MAX_CONCURRENT_ROADMAP_RUNS)
        .min(MAX_CONCURRENT_ROADMAP_CEILING)
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

/// A definition's display name — the string the board's workflow chip and the
/// sidebar show. Read off the row rather than out of the spec so the history
/// line and the rest of the UI can't call one workflow two things (a rename
/// writes the column; the embedded `spec.name` may lag).
fn definition_name(conn: &Connection, definition_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT name FROM wf_definition WHERE id = ?1",
        [definition_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .map(|n| n.trim().to_string())
    .filter(|n| !n.is_empty())
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
