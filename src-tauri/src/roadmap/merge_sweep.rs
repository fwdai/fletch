//! The roadmap merge sweep: the background task that closes the loop by moving
//! an `in_review` item to `done` once its pull request merges on GitHub.
//!
//! # Why this exists in Rust at all
//!
//! Every other PR poll in the app runs in the webview — the Git panel's fast
//! tick, the fleet sweep in `supervisor::session_sync`. Those are *views*: they
//! keep a badge honest while you are looking at it, and stopping when
//! `document.hidden` is exactly right for them.
//!
//! The roadmap queue is not a view. Its whole promise is that you can queue five
//! items, close the window, and come back to five merged PRs — and `done` is
//! what unblocks the next item's dependants ([`drainer::nudge`] below). A merge
//! detector that only runs while the board is on screen would stall the queue
//! behind whichever PR you happened not to be watching. So this one poll lives
//! host-side, on the same footing as the drainer.
//!
//! # The sweep
//!
//! Every [`SWEEP`] while anything is `in_review`, for each such item with a PR
//! number: one conditional REST read of the PR, then
//!
//! - **merged** → `done`, and [`drainer::nudge`] so a dependant queued behind it
//!   dispatches immediately rather than waiting out the drainer's own tick.
//! - **closed, not merged** → back to `open`, with a note saying so.
//! - **still open, or no answer** → untouched, retried next sweep.
//!
//! When nothing is in review the task sleeps on its [`nudge`] instead of ticking:
//! an install whose board is all `done` costs nothing and makes no requests.
//!
//! # Why polling, and why it is nearly free
//!
//! GitHub can push this (webhooks) only to a public endpoint, which a desktop
//! app does not have. Polling is the available mechanism, and the cost is held
//! down by reusing [`crate::github::pr_state_live`]: an ETag-conditional REST
//! read whose `304 Not Modified` is not billed against the rate limit. An
//! unchanged PR costs one round-trip and no budget, so a slow-moving review
//! queue is effectively free to watch.
//!
//! # What it never does
//!
//! It never *blanks* state. A failed fetch, an unresolvable repo, a missing
//! token — all leave the item exactly as it was and try again later, the same
//! policy `supervisor::resolve_pr_state` follows. The only writes are the two
//! definite verdicts GitHub gave us.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use tauri::AppHandle;
use tokio::sync::Notify;

use super::drainer::{self, QueueNote};
use super::events::{EventActor, EventKind};
use super::types::{ItemPatch, ItemStatus, RoadmapItem};
use super::Db;
use crate::github::PrStatus;

/// How often the sweep re-reads its watch list while anything is in review.
///
/// Two minutes. A merge is a human act at the end of a review, so nobody is
/// waiting on sub-minute latency here — and the thing that *is* waiting (a
/// dependant item) is unblocked within one drainer tick of the flip. Longer
/// than the drainer's 15s because each pass costs network, and shorter than the
/// span in which "I merged it, why is the board stale" becomes a real thought.
const SWEEP: Duration = Duration::from_secs(120);

// ───────────────────────────── the nudge ────────────────────────────────

/// Wakes the sweep. Separate from the drainer's signal on purpose: the two
/// tasks wake on different events (a queue mutation vs. an item entering
/// review), and sharing one `Notify` would have each spuriously waking the
/// other — the drainer walking every board because a PR merged, the sweep
/// making a network round-trip because someone edited an item's title.
fn signal() -> &'static Notify {
    static SIGNAL: OnceLock<Notify> = OnceLock::new();
    SIGNAL.get_or_init(Notify::new)
}

/// Ask the sweep to look now. Called by the drainer when it settles an item into
/// `in_review`, which is the only way an item acquires a PR to watch.
pub(crate) fn nudge() {
    signal().notify_one();
}

// ───────────────────────────── pure decisions ───────────────────────────

/// The items a sweep polls: `in_review` with a PR number to poll with.
///
/// An `in_review` item *without* a number is left alone rather than guessed at.
/// It means the run finished, opened something, and the number never made it
/// onto the row — polling would need a number invented from the URL, and a
/// wrong number is a wrong verdict written to the board. The card still links
/// the PR; the user merges it and marks the item done by hand.
pub(crate) fn pollable(items: &[RoadmapItem]) -> Vec<&RoadmapItem> {
    items
        .iter()
        .filter(|i| i.status == ItemStatus::InReview && i.pr_number.is_some())
        .collect()
}

/// What one polled PR means for the item watching it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Verdict {
    /// Still open, or no answer this round. Leave the item alone.
    Waiting,
    /// Merged. The work is in the base branch; the item shipped.
    Landed,
    /// Closed without merging. The work was proposed and rejected.
    Abandoned,
}

/// Map a PR's state onto its item. `None` is "we didn't learn anything this
/// round" — a fetch error, a rate-limit pause, a repo we couldn't resolve — and
/// is deliberately indistinguishable from `Open` here: both mean *don't write*.
pub(crate) fn verdict(state: Option<PrStatus>) -> Verdict {
    match state {
        Some(PrStatus::Merged) => Verdict::Landed,
        Some(PrStatus::Closed) => Verdict::Abandoned,
        Some(PrStatus::Open) | None => Verdict::Waiting,
    }
}

/// The write a verdict implies, or `None` when the item stays as it is.
///
/// **Landed** sets only the status: `pr_url`/`pr_number` stay, because a shipped
/// item's PR is the record of how it shipped.
///
/// **Abandoned** returns the item to `open` rather than to `done` or to some
/// `rejected` state the board has no column for. A closed PR is a decision the
/// user made outside this app, and the honest reflection of it is "this is back
/// on your board" — not a silent completion, and not a new lifecycle state that
/// every consumer would have to learn. The PR columns are kept for history (the
/// card keeps its "PR #N" link), but `run_id` is cleared for the same reason
/// [`drainer::Settlement::Released`] clears it: the run is over, and leaving the
/// link would have a re-queue settle instantly against that old terminal run
/// instead of dispatching a fresh one.
pub(crate) fn patch_for(verdict: &Verdict) -> Option<ItemPatch> {
    match verdict {
        Verdict::Waiting => None,
        Verdict::Landed => Some(ItemPatch {
            status: Some(ItemStatus::Done),
            ..Default::default()
        }),
        Verdict::Abandoned => Some(ItemPatch {
            status: Some(ItemStatus::Open),
            run_id: Some(None),
            ..Default::default()
        }),
    }
}

/// The history event a verdict writes alongside its patch — the durable half of
/// the answer, paired with [`patch_for`] (`None` exactly when that is). The
/// `shipped` event's timestamp is the item's `done_at`.
pub(crate) fn event_for(verdict: &Verdict) -> Option<(EventKind, Option<String>)> {
    match verdict {
        Verdict::Waiting => None,
        Verdict::Landed => Some((EventKind::Shipped, None)),
        Verdict::Abandoned => Some((
            EventKind::Abandoned,
            Some("PR closed without merging".to_string()),
        )),
    }
}

/// What the board is told when a PR is closed unmerged. Nothing persists it —
/// same transient `roadmap:queue-note` channel the drainer explains a stuck
/// queue on, for the same reason: it is true until the user does something
/// about it, and the row's own next change supersedes it. The durable record is
/// the `abandoned` event [`event_for`] pairs with the same verdict.
pub(crate) fn abandoned_note(number: i64) -> String {
    format!("PR #{number} was closed without merging — back on the board.")
}

// ───────────────────────────── the task ─────────────────────────────────

/// One item the sweep is watching, flattened out of the connection guard.
struct Watched {
    id: String,
    code: String,
    project_id: String,
    number: i64,
}

/// Start the sweep. Called once from setup, beside [`drainer::spawn`].
///
/// Takes no `WorkflowService`: by the time an item is `in_review` its run is
/// finished and the only remaining authority is GitHub.
pub fn spawn(app: AppHandle, db: Db) {
    tauri::async_runtime::spawn(async move {
        loop {
            // Each pass runs in its own task so a panic is contained: silently
            // dead until the next app start is the worst possible failure mode
            // for the thing that ships merged work — the same guard the
            // drainer's tick carries.
            let pass = {
                let app = app.clone();
                let db = db.clone();
                tauri::async_runtime::spawn(async move {
                    let watching = watch_list(&db);
                    let idle = watching.is_empty();
                    if !idle {
                        sweep(&app, &db, watching).await;
                    }
                    idle
                })
                .await
            };
            match pass {
                // Nothing in review: sleep until the drainer says there is.
                // Checked before waiting, not after, so a board that is already
                // mid-review at launch is swept without needing a start nudge.
                Ok(true) => signal().notified().await,
                Ok(false) => tokio::select! {
                    _ = tokio::time::sleep(SWEEP) => {}
                    _ = signal().notified() => {}
                },
                Err(e) => {
                    tracing::error!(error = %e, "roadmap merge sweep pass panicked — sweeping continues");
                    // Don't park: the watch list may be non-empty, and the only
                    // thing that nudges a parked sweep is a drainer settlement.
                    tokio::select! {
                        _ = tokio::time::sleep(SWEEP) => {}
                        _ = signal().notified() => {}
                    }
                }
            }
        }
    });
}

/// Every item under review that has a PR to poll, across all projects. One
/// query rather than per-project, because the sweep's unit of work is a PR and
/// its cost is the network read, not the row.
fn watch_list(db: &Db) -> Vec<Watched> {
    let conn = db.lock();
    let items = conn
        .prepare(&format!(
            "SELECT {} FROM roadmap_items
              WHERE status = 'in_review' AND pr_number IS NOT NULL",
            super::types::COLUMNS
        ))
        .and_then(|mut s| {
            s.query_map([], RoadmapItem::from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()
        })
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "roadmap merge sweep: cannot read the watch list");
            Vec::new()
        });
    // The query already narrows to what `pollable` selects; running it anyway
    // keeps one definition of "watchable" and costs a filter over a short list.
    pollable(&items)
        .into_iter()
        .filter_map(|i| {
            Some(Watched {
                id: i.id.clone(),
                code: i.code.clone(),
                project_id: i.project_id.clone(),
                number: i.pr_number?,
            })
        })
        .collect()
}

/// Poll each watched PR and apply whatever GitHub says. Sequential on purpose:
/// in-review items *accumulate* — [`drainer::MAX_CONCURRENT_ROADMAP_RUNS`] caps
/// live runs, not open reviews, and a settled run frees its slot — but the list
/// only grows as fast as runs finish, and conditional requests make a repeat
/// read of an unchanged PR nearly free (a 304 isn't billed).
async fn sweep(app: &AppHandle, db: &Db, watching: Vec<Watched>) {
    // One repo path per project per sweep — resolving it is a database read,
    // and every item in a project shares the answer.
    let mut repos: HashMap<String, Option<PathBuf>> = HashMap::new();
    for w in watching {
        let repo = repos
            .entry(w.project_id.clone())
            .or_insert_with(|| project_repo(db, &w.project_id))
            .clone();
        let Some(repo) = repo else {
            // No repo row, or the project was deleted under us: there is no
            // remote to ask. Say nothing and touch nothing.
            tracing::debug!(item = %w.code, "roadmap merge sweep: no repo to resolve the PR against");
            continue;
        };
        let state = poll(&repo, w.number).await;
        let outcome = verdict(state);
        let Some(patch) = patch_for(&outcome) else {
            continue;
        };
        // `patch_for` and `event_for` are `Some` for exactly the same verdicts;
        // the expect documents that rather than inventing a fallback event.
        let (kind, detail) = event_for(&outcome).expect("a verdict that writes also records");
        tracing::info!(item = %w.code, pr = w.number, ?outcome, "roadmap merge sweep");
        // Conditional on the row still being in review: the verdict was decided
        // over a network read, and a row the user moved meanwhile (marked done
        // by hand, re-queued after a delete) must not be stamped over. A miss
        // records no event either — the row's current owner writes its own.
        drainer::write_item_where(
            app,
            db,
            &w.id,
            ItemStatus::InReview,
            patch,
            EventActor::Sweep,
            kind,
            detail,
        );
        match outcome {
            Verdict::Landed => {
                // The item is `done`, which is what a dependant's dep gate is
                // waiting for. Nudge rather than let it sit until the drainer's
                // next tick — this is the moment the queue can move.
                drainer::nudge();
            }
            Verdict::Abandoned => drainer::emit_note(
                app,
                &QueueNote {
                    item_id: w.id.clone(),
                    code: w.code.clone(),
                    note: abandoned_note(w.number),
                },
            ),
            Verdict::Waiting => {}
        }
    }
}

/// One PR's state, or `None` when we learned nothing. Every failure mode —
/// no token, a non-GitHub remote, a rate-limit pause, a deleted PR — collapses
/// to `None` here, because they all mean the same thing to the caller: leave
/// the item alone and ask again later.
async fn poll(repo: &std::path::Path, number: i64) -> Option<PrStatus> {
    let number = u32::try_from(number).ok()?;
    match crate::github::pr_state_live(repo, number).await {
        Ok(Some(state)) => Some(state.state),
        Ok(None) => None,
        Err(e) => {
            tracing::debug!(pr = number, error = %e, "roadmap merge sweep: PR read failed");
            None
        }
    }
}

/// The checkout the sweep resolves `owner/repo` from: the project's primary
/// repo, the same one the drainer launches runs in.
///
/// Deliberately *not* the run's own repo (`~/.fletch/runs/<id>/repo`): that
/// directory is a scratch clone that may be cleaned up once the run finishes,
/// and the sweep's whole job happens after that point. The project's checkout
/// shares the origin remote and outlives every run in it.
fn project_repo(db: &Db, project_id: &str) -> Option<PathBuf> {
    let conn = db.lock();
    drainer::primary_repo_path(&conn, project_id).map(PathBuf::from)
}

#[cfg(test)]
mod tests;
