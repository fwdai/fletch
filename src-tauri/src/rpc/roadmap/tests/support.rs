use super::brakes::{hold_op, Held};
use super::brief::{brief_op, propose_brief_op};
use super::deltas::{propose_discard_op, propose_update_op};
use super::intake::propose_op;
use super::listing::list_op;
use super::notes::note_op;
use super::ordering::propose_order_op;
use super::*;

use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::database::get_migrations;
use crate::roadmap::events::ItemEvent;
use crate::roadmap::memory::BriefProposal;
use crate::roadmap::order_proposals::OrderProposal;
use crate::roadmap::proposals::Proposal;
use crate::roadmap::Db;
use crate::rpc::caps::AgentCaps;
use crate::rpc::git::GitDispatcher;
use crate::rpc::Response;

/// A migrated in-memory DB with one project, matching how the store's own
/// tests set up (the FK to `projects` is real).
pub(super) fn test_db(project_id: &str) -> Db {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    get_migrations().to_latest(&mut conn).unwrap();
    conn.execute(
        "INSERT INTO projects (id, name, created_at) VALUES (?1, 'my-cool-app', 0)",
        params![project_id],
    )
    .unwrap();
    Arc::new(Mutex::new(conn))
}

/// A dispatcher with no window to emit into — everything but the events.
pub(super) fn dispatcher(db: &Db, project_id: &str) -> RoadmapDispatcher {
    RoadmapDispatcher {
        app: None,
        db: db.clone(),
        project_id: project_id.to_string(),
        git: GitDispatcher::new(
            std::env::temp_dir(),
            "main".to_string(),
            AgentCaps::advisory(),
        ),
    }
}

/// The ops are synchronous under the lock, so most tests exercise them
/// directly and only the routing tests go through `dispatch`.
pub(super) fn propose(db: &Db, args: Value) -> Response {
    let conn = db.lock();
    propose_op(&conn, "p1", "r1", &args).0
}

pub(super) fn list(db: &Db, args: Value) -> Response {
    let conn = db.lock();
    list_op(&conn, "p1", "r1", &args)
}

/// The live rows of a `roadmap_list` response — the payload's `items`.
/// Most tests read only the board half; `not_doing` has its own tests.
pub(super) fn board_rows(resp: &Response) -> Vec<Value> {
    let payload: Value = serde_json::from_str(resp.stdout.as_ref().unwrap()).unwrap();
    payload["items"].as_array().unwrap().clone()
}

pub(super) fn propose_update(db: &Db, args: Value) -> (Response, Option<Proposal>) {
    let conn = db.lock();
    propose_update_op(&conn, "p1", "r1", &args)
}

pub(super) fn propose_discard(db: &Db, args: Value) -> (Response, Option<Proposal>) {
    let conn = db.lock();
    propose_discard_op(&conn, "p1", "r1", &args)
}

pub(super) fn propose_order(db: &Db, args: Value) -> (Response, Option<OrderProposal>) {
    let conn = db.lock();
    propose_order_op(&conn, "p1", "r1", &args)
}

pub(super) fn brief(db: &Db, args: Value) -> Response {
    let conn = db.lock();
    brief_op(&conn, "p1", "r1", &args)
}

pub(super) fn propose_brief(db: &Db, args: Value) -> (Response, Option<BriefProposal>) {
    let conn = db.lock();
    propose_brief_op(&conn, "p1", "r1", &args)
}

pub(super) fn note(db: &Db, args: Value) -> (Response, Option<ItemEvent>) {
    let conn = db.lock();
    note_op(&conn, "p1", "r1", &args)
}

pub(super) fn hold(db: &Db, args: Value) -> (Response, Option<Held>) {
    let conn = db.lock();
    hold_op(&conn, "p1", "r1", &args)
}

pub(super) fn one_item(title: &str) -> Value {
    json!({ "items": [{ "title": title, "why": "because", "horizon": "next" }] })
}
