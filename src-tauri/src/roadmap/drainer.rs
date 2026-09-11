//! Background queue: turns `queued` items into workflows and reflects runs onto the board.
//!
//! Status *is* the queue (`queued` + `rank`); no separate queue table. Horizon does not
//! gate dispatch. Brakes stop claim/dispatch only — settle still runs (reflecting a
//! finished run is not autonomy). A hold is transitive for deps: landed means `done`
//! and not held. Cap/autoqueue never override a hold (invariant 2).
//!
//! DB work stays inside a `parking_lot` guard with no `.await`; the claim
//! (`queued → active`) is atomic under that lock. `in_review` leaves this module —
//! [`super::merge_sweep`] owns it from there.


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
use super::{deps, emit_item, emit_item_event, brakes, store, Db};
use crate::workflow::spec::{self, Spec};
use crate::workflow::types::RunStatus;

/// Default concurrent roadmap runs when the project dial is unset.
pub const MAX_CONCURRENT_ROADMAP_RUNS: usize = 1;

/// Hard clamp on `roadmap.max_concurrent`.
pub const MAX_CONCURRENT_ROADMAP_CEILING: usize = 4;

// Idle wake interval; nudges also wake immediately.
const TICK: Duration = Duration::from_secs(15);

const LIVE_RUN_STATUSES: &str = "'pending','running','paused'";

fn signal() -> &'static Notify {
    static SIGNAL: OnceLock<Notify> = OnceLock::new();
    SIGNAL.get_or_init(Notify::new)
}

/// Wake between ticks after roadmap mutations.
pub(crate) fn nudge() {
    signal().notify_one();
}

#[derive(Debug, Clone, PartialEq, Serialize)]
/// Transient `roadmap:queue-note` — not persisted (unlike [`super::events`]).
pub struct QueueNote {
    pub item_id: String,
    pub code: String,
    pub note: String,
}

type SaidNote = (String, i64);

type SaidNotes = Mutex<HashMap<String, SaidNote>>;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Decision {
    Empty,
    AtCapacity,
    Blocked {
        item_id: String,
        waiting_on: Vec<String>,
    },
    Dispatch(usize),
}

/// `done` and not held — holds are transitive for dependants.
pub(crate) fn done_codes(items: &[RoadmapItem]) -> HashSet<String> {
    items
        .iter()
        .filter(|i| i.status == ItemStatus::Done && !i.is_held())
        .map(|i| i.code.clone())
        .collect()
}

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

/// Queued, unheld items in board rank order (project brake checked at claim).
pub(crate) fn dispatchable(items: &[RoadmapItem]) -> Vec<RoadmapItem> {
    items
        .iter()
        .filter(|i| i.status == ItemStatus::Queued && i.agent_id.is_none() && !i.is_held())
        .cloned()
        .collect()
}

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
    match head_block {
        Some((item_id, waiting_on)) => Decision::Blocked {
            item_id,
            waiting_on,
        },
        None => Decision::Empty,
    }
}

pub(crate) fn resolve_workflow(
    item: &RoadmapItem,
    project_default: Option<&str>,
) -> Option<String> {
    item.workflow_def_id
        .clone()
        .or_else(|| project_default.map(str::to_string))
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FinalizedPr {
    pub url: String,
    pub number: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Settlement {
    Running,
    InReview,
    Done,
    Released(&'static str),
}

pub(crate) const RUN_FAILED: &str = "its run failed";
pub(crate) const RUN_CANCELED: &str = "its run was canceled";
pub(crate) const RUN_DELETED: &str = "its run was deleted";
pub(crate) const RUN_NEVER_STARTED: &str = "its run never started";
pub(crate) const RUN_UNLAUNCHABLE: &str = "its run couldn't be started";

pub(crate) fn release_kind(why: &str) -> EventKind {
    match why {
        RUN_CANCELED => EventKind::RunCanceled,
        RUN_DELETED => EventKind::RunDeleted,
        _ => EventKind::RunFailed,
    }
}

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

pub(crate) fn settlement_patch(outcome: &Settlement, pr: Option<&FinalizedPr>) -> ItemPatch {
    match outcome {
        Settlement::Running => ItemPatch::default(),
        Settlement::InReview => ItemPatch {
            status: Some(ItemStatus::InReview),
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
            run_id: Some(None),
            ..Default::default()
        },
    }
}

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

struct Plan {
    item: RoadmapItem,
    definition_id: String,
    spec: Spec,
    repo_path: String,
    brief: String,
}

pub fn spawn(app: AppHandle, db: Db, service: Arc<crate::workflow::scheduler::WorkflowService>) {
    tauri::async_runtime::spawn(async move {
        let said: Arc<SaidNotes> = Arc::new(Mutex::new(HashMap::new()));
        loop {
            tokio::select! {
                _ = tokio::time::sleep(TICK) => {}
                _ = signal().notified() => {}
            }
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
        for _ in 0..cap {
            let Some(plan) = claim_next(app, db, &project_id, cap, said) else {
                break;
            };
            dispatch(app, db, service, plan).await;
        }
    }
}

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

struct Settled {
    item: RoadmapItem,
    outcome: Settlement,
    pr: Option<FinalizedPr>,
    adopted_run_id: Option<String>,
}

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
            .filter(|i| i.run_id.is_some() || i.agent_id.is_none())
            .map(|item| {
                let adopted_run_id = match &item.run_id {
                    Some(_) => None,
                    None => dispatched_run_id(&conn, &item.id),
                };
                let run_id = item.run_id.clone().or_else(|| adopted_run_id.clone());
                let status = match &run_id {
                    Some(id) => run_status(&conn, id),
                    None => None,
                };
                let pr = match status {
                    Some(RunStatus::Done) => {
                        run_id.as_deref().and_then(|id| finalized_pr(&conn, id))
                    }
                    _ => None,
                };
                let outcome = match run_id {
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
            Settlement::InReview => {
                forget(said, &item.id);
                super::merge_sweep::nudge();
            }
            _ => forget(said, &item.id),
        }
    }
}

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
        None => {
            write_item(app, db, &item.id, patch);
            true
        }
    };
    if landed {
        if let Some(reviewable) = review::outcome_for(outcome, pr) {
            review::request(app, db, item, &reviewable);
        }
    }
    landed
}

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

fn run_status(conn: &Connection, run_id: &str) -> Option<RunStatus> {
    conn.query_row("SELECT status FROM wf_run WHERE id = ?1", [run_id], |r| {
        r.get::<_, String>(0)
    })
    .optional()
    .ok()
    .flatten()
    .and_then(|s| RunStatus::from_db(&s))
}

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

enum Claim {
    Nothing,
    Note {
        item: Box<RoadmapItem>,
        text: String,
        recorded: Option<ItemEvent>,
    },
    Claimed(Box<Plan>, ItemEvent),
}

impl Claim {
    fn note(item: RoadmapItem, text: String) -> Self {
        Claim::Note {
            item: Box::new(item),
            text,
            recorded: None,
        }
    }

    fn wedge(conn: &Connection, item: RoadmapItem, text: String) -> Self {
        let recorded = record_wedge(conn, &item, &text);
        Claim::Note {
            item: Box::new(item),
            text,
            recorded,
        }
    }
}

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
            if let Some(event) = &recorded {
                emit_item_event(app, event);
            }
            None
        }
        Claim::Claimed(plan, event) => {
            forget(said, &plan.item.id);
            emit_item(app, &plan.item);
            emit_item_event(app, &event);
            Some(*plan)
        }
    }
}

/// Re-decide under the lock each claim; brakes project_gate fail-closed.
fn plan_and_claim(conn: &Connection, project_id: &str, cap: usize) -> Claim {
    if let Some(reason) = brakes::project_gate(conn, project_id) {
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
        Decision::Empty | Decision::AtCapacity => return Claim::Nothing,
    };

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

    let dep_rows: Vec<&RoadmapItem> = item
        .deps
        .iter()
        .filter_map(|code| items.iter().find(|i| &i.code == code))
        .collect();
    let brief = build_brief(&item, &dep_rows);

    let workflow_name = definition_name(conn, &definition_id);

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
        Some(workflow_name.unwrap_or(definition_id)),
    )?;
    Ok(Some((claimed, event)))
}

// Drop DB lock before WorkflowService::launch await.
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
            tracing::warn!(item = %item.code, error = %e, "roadmap drainer: launch failed");
            let reason = format!("Couldn't start a run — {e}");
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

const DEFAULT_WORKFLOW_KEY: &str = "workflow.default";

const MAX_CONCURRENT_KEY: &str = "roadmap.max_concurrent";

// Read by accept path; holds trump autoqueue.
const AUTOQUEUE_KEY: &str = "roadmap.autoqueue";

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

pub(super) fn project_flag(conn: &Connection, project_id: &str, key: &str, default: bool) -> bool {
    parse_flag(project_setting(conn, project_id, key).as_deref(), default)
}

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

pub(super) fn autoqueue(conn: &Connection, project_id: &str) -> bool {
    project_flag(conn, project_id, AUTOQUEUE_KEY, false)
}

pub(crate) fn concurrency_cap(conn: &Connection, project_id: &str) -> usize {
    parse_cap(project_setting(conn, project_id, MAX_CONCURRENT_KEY).as_deref())
}

pub(crate) fn parse_cap(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(MAX_CONCURRENT_ROADMAP_RUNS)
        .min(MAX_CONCURRENT_ROADMAP_CEILING)
}

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
    if let Err(errs) = spec::validate(&spec) {
        tracing::warn!(definition_id, errors = %errs.join("; "), "invalid workflow spec");
        return None;
    }
    Some(spec)
}

pub(crate) fn write_item(app: &AppHandle, db: &Db, id: &str, patch: ItemPatch) {
    let updated = {
        let conn = db.lock();
        store::update(&conn, id, &patch)
    };
    match updated {
        Ok(Some(row)) => emit_item(app, &row),
        Ok(None) => {}
        Err(e) => tracing::warn!(id, error = %e, "roadmap drainer: item write failed"),
    }
}

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
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(id, error = %e, "roadmap drainer: item write failed");
            false
        }
    }
}

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
        Ok(None) => tracing::debug!(
            id,
            "roadmap: row moved before a verdict landed — left alone"
        ),
        Err(e) => tracing::warn!(id, error = %e, "roadmap: item write failed"),
    }
}

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

pub(crate) fn emit_note(app: &AppHandle, note: &QueueNote) {
    let _ = app.emit("roadmap:queue-note", note);
}

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
#[path = "tests/drainer.rs"]
mod tests;
