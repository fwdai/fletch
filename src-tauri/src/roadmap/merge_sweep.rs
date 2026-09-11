//! Moves `in_review` items to `done` when their GitHub PR merges.
//!
//! Host-side so the queue keeps draining with the window shut. Respects
//! [`super::brakes::gate`] — a held item/board must not be auto-shipped.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use tauri::AppHandle;
use tokio::sync::Notify;

use super::drainer::{self, QueueNote};
use super::events::{EventActor, EventKind, TrailEntry};
use super::store;
use super::types::{ItemPatch, ItemStatus, RoadmapItem};
use super::{brakes, Db};
use crate::github::PrStatus;

/// Fail closed: user merge must not bypass an item/project hold.
pub(super) fn merge_hold_gate(db: &Db, item_id: &str) -> Result<(), String> {
    let conn = db.lock();
    let Some(item) = store::get(&conn, item_id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    match brakes::gate(&conn, &item) {
        Some(reason) => Err(format!(
            "{} is held — {reason}. Release the hold before merging its pull request.",
            item.code
        )),
        None => Ok(()),
    }
}

const SWEEP: Duration = Duration::from_secs(120);

fn signal() -> &'static Notify {
    static SIGNAL: OnceLock<Notify> = OnceLock::new();
    SIGNAL.get_or_init(Notify::new)
}

pub(crate) fn nudge() {
    signal().notify_one();
}

const UNANSWERED_LIMIT: u32 = 5;

type Misses = (i64, u32);

type Unanswered = Mutex<HashMap<String, Misses>>;

pub(crate) fn pollable(items: &[RoadmapItem]) -> Vec<&RoadmapItem> {
    items
        .iter()
        .filter(|i| i.status == ItemStatus::InReview && i.pr_number.is_some())
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Verdict {
    Waiting,
    Landed,
    Abandoned,
}

pub(crate) fn verdict(state: Option<PrStatus>) -> Verdict {
    match state {
        Some(PrStatus::Merged) => Verdict::Landed,
        Some(PrStatus::Closed) => Verdict::Abandoned,
        Some(PrStatus::Open) | None => Verdict::Waiting,
    }
}

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

pub(crate) const SHIPPED_WHILE_HELD: &str =
    "its PR merged while this was held — the hold stands, so nothing waiting on it moves";

pub(crate) fn event_for(verdict: &Verdict, held: bool) -> Option<(EventKind, Option<String>)> {
    match verdict {
        Verdict::Waiting => None,
        Verdict::Landed => Some((
            EventKind::Shipped,
            held.then(|| SHIPPED_WHILE_HELD.to_string()),
        )),
        Verdict::Abandoned => Some((
            EventKind::PrClosed,
            Some("nothing merged — the item is back on the board".to_string()),
        )),
    }
}

fn still_watching(seen: &HashMap<String, Misses>, id: &str, updated_at: i64) -> bool {
    match seen.get(id) {
        Some((version, misses)) => *version != updated_at || *misses < UNANSWERED_LIMIT,
        None => true,
    }
}

fn record_miss(seen: &mut HashMap<String, Misses>, id: &str, updated_at: i64) -> bool {
    let entry = seen.entry(id.to_string()).or_insert((updated_at, 0));
    if entry.0 != updated_at {
        *entry = (updated_at, 0);
    }
    entry.1 += 1;
    entry.1 == UNANSWERED_LIMIT
}

fn answered(seen: &mut HashMap<String, Misses>, id: &str) {
    seen.remove(id);
}

pub(crate) fn unreachable_note(number: i64) -> String {
    format!(
        "Can't reach PR #{number} — nothing has answered for {UNANSWERED_LIMIT} sweeps. It may \
         have been deleted, or this repo's remote or token can't see it. Merge it yourself and \
         mark this done, or put the item back on the board."
    )
}

pub(crate) fn abandoned_note(number: i64) -> String {
    format!("PR #{number} was closed without merging — back on the board.")
}

struct Watched {
    id: String,
    code: String,
    project_id: String,
    number: i64,
    updated_at: i64,
}

// Background sweep while anything is in_review.
pub fn spawn(app: AppHandle, db: Db) {
    tauri::async_runtime::spawn(async move {
        let seen: Arc<Unanswered> = Arc::new(Mutex::new(HashMap::new()));
        loop {
            let pass = {
                let (app, db, seen) = (app.clone(), db.clone(), seen.clone());
                tauri::async_runtime::spawn(async move {
                    let watching = watch_list(&db);
                    let idle = watching.is_empty();
                    if !idle {
                        sweep(&app, &db, watching, &seen).await;
                    }
                    idle
                })
                .await
            };
            match pass {
                Ok(true) => signal().notified().await,
                Ok(false) => tokio::select! {
                    _ = tokio::time::sleep(SWEEP) => {}
                    _ = signal().notified() => {}
                },
                Err(e) => {
                    tracing::error!(error = %e, "roadmap merge sweep pass panicked — sweeping continues");
                    tokio::select! {
                        _ = tokio::time::sleep(SWEEP) => {}
                        _ = signal().notified() => {}
                    }
                }
            }
        }
    });
}

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
    pollable(&items)
        .into_iter()
        .filter_map(|i| {
            Some(Watched {
                id: i.id.clone(),
                code: i.code.clone(),
                project_id: i.project_id.clone(),
                number: i.pr_number?,
                updated_at: i.updated_at,
            })
        })
        .collect()
}

async fn sweep(app: &AppHandle, db: &Db, watching: Vec<Watched>, seen: &Unanswered) {
    let mut repos: HashMap<String, Option<PathBuf>> = HashMap::new();
    for w in watching {
        if !still_watching(&seen.lock(), &w.id, w.updated_at) {
            continue;
        }
        let repo = repos
            .entry(w.project_id.clone())
            .or_insert_with(|| project_repo(db, &w.project_id))
            .clone();
        let state = match &repo {
            Some(repo) => poll(repo, w.number).await,
            None => {
                tracing::debug!(item = %w.code, "roadmap merge sweep: no repo to resolve the PR against");
                None
            }
        };
        if state.is_none() {
            if record_miss(&mut seen.lock(), &w.id, w.updated_at) {
                unreachable(app, db, &w);
            }
            continue;
        }
        answered(&mut seen.lock(), &w.id);
        let outcome = verdict(state);
        let Some(patch) = patch_for(&outcome) else {
            continue;
        };
        let held = matches!(outcome, Verdict::Landed) && held_now(db, &w.id);
        let (kind, detail) = event_for(&outcome, held).expect("a verdict that writes also records");
        tracing::info!(item = %w.code, pr = w.number, ?outcome, held, "roadmap merge sweep");
        drainer::write_item_where(
            app,
            db,
            &w.id,
            ItemStatus::InReview,
            patch,
            TrailEntry {
                actor: EventActor::Sweep,
                kind,
                detail,
            },
        );
        match outcome {
            Verdict::Landed if held => tracing::info!(
                item = %w.code,
                "roadmap merge sweep: shipped a held item — nothing waiting on it may move"
            ),
            Verdict::Landed => {
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

fn unreachable(app: &AppHandle, db: &Db, w: &Watched) {
    let text = unreachable_note(w.number);
    tracing::warn!(
        item = %w.code,
        pr = w.number,
        "roadmap merge sweep: giving up on a PR that stopped answering"
    );
    let recorded = {
        let conn = db.lock();
        match super::store::get(&conn, &w.id) {
            Ok(Some(item)) => drainer::record_wedge(&conn, &item, &text),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(item = %w.code, error = %e, "roadmap merge sweep: cannot read the item");
                None
            }
        }
    };
    if let Some(event) = &recorded {
        super::emit_item_event(app, event);
    }
    drainer::emit_note(
        app,
        &QueueNote {
            item_id: w.id.clone(),
            code: w.code.clone(),
            note: text,
        },
    );
}

fn held_now(db: &Db, item_id: &str) -> bool {
    let conn = db.lock();
    match super::store::get(&conn, item_id) {
        Ok(Some(item)) => brakes::gate(&conn, &item).is_some(),
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(item_id, error = %e, "roadmap merge sweep: cannot read the item's hold");
            true
        }
    }
}

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

fn project_repo(db: &Db, project_id: &str) -> Option<PathBuf> {
    let conn = db.lock();
    drainer::primary_repo_path(&conn, project_id).map(PathBuf::from)
}

#[cfg(test)]
#[path = "tests/merge_sweep.rs"]
mod tests;
