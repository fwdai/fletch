//! The `roadmap_*` RPC ops for the project-manager chat. Wraps [`GitDispatcher`]
//! and delegates everything else, so the PM keeps the ordinary git surface.
//!
//! The project id is stamped at construction, never taken from `args`. Every
//! `propose_*` op parks an ask the user rules on; only `roadmap_note` and
//! `roadmap_hold` write directly, and neither advances an item. There is no
//! release op, so an agent can never lift its own brake.

mod args;
mod brakes;
mod brief;
mod deltas;
mod duplicates;
mod intake;
mod listing;
mod notes;
mod ordering;

#[cfg(test)]
#[path = "tests/support.rs"]
mod test_support;

use rusqlite::Connection;
use serde_json::Value;
use tauri::AppHandle;

use crate::roadmap::Db;
use crate::roadmap::{
    emit_brief_proposal, emit_item, emit_item_event, emit_order_proposal, emit_project_hold,
    emit_proposal,
};
use crate::rpc::git::GitDispatcher;
use crate::rpc::{Response, RpcDispatcher, RpcEvent, RpcFuture};

use brakes::{hold_op, Held};
use brief::{brief_op, propose_brief_op};
use deltas::{propose_discard_op, propose_update_op};
use intake::propose_op;
use listing::list_op;
use notes::note_op;
use ordering::propose_order_op;

/// Pinned by a test against the instruction block so the two can't drift.
pub const OPS: [&str; 9] = [
    "roadmap_list",
    "roadmap_propose",
    "roadmap_propose_update",
    "roadmap_propose_discard",
    "roadmap_propose_order",
    "roadmap_note",
    "roadmap_hold",
    "roadmap_brief",
    "roadmap_propose_brief_update",
];

/// The whole namespace, so a typo'd op gets an error naming the real ones.
fn is_roadmap_op(op: &str) -> bool {
    op.starts_with("roadmap_")
}

pub struct RoadmapDispatcher {
    /// `None` only in tests, which have no window.
    app: Option<AppHandle>,
    db: Db,
    project_id: String,
    git: GitDispatcher,
}

impl RoadmapDispatcher {
    pub fn new(app: AppHandle, db: Db, project_id: String, git: GitDispatcher) -> Self {
        Self {
            app: Some(app),
            db,
            project_id,
            git,
        }
    }

    /// Announces only after the lock guard has dropped.
    fn announce<T>(
        &self,
        op: impl FnOnce(&Connection, &str) -> (Response, Option<T>),
        emit: impl FnOnce(&AppHandle, &T),
    ) -> (Response, Vec<RpcEvent>) {
        let (resp, stored) = {
            let conn = self.db.lock();
            op(&conn, &self.project_id)
        };
        if let (Some(app), Some(stored)) = (&self.app, &stored) {
            emit(app, stored);
        }
        (resp, Vec::new())
    }
}

impl RpcDispatcher for RoadmapDispatcher {
    fn dispatch<'a>(
        &'a self,
        id: &'a str,
        op: &'a str,
        args: &'a Value,
    ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
        Box::pin(async move {
            if !is_roadmap_op(op) {
                return self.git.dispatch(id, op, args).await;
            }
            match op {
                "roadmap_list" => {
                    let conn = self.db.lock();
                    (list_op(&conn, &self.project_id, id, args), Vec::new())
                }
                "roadmap_propose" => self.announce(
                    |conn, project| propose_op(conn, project, id, args),
                    |app, (created, recorded)| {
                        for item in created {
                            emit_item(app, item);
                        }
                        for event in recorded {
                            emit_item_event(app, event);
                        }
                    },
                ),
                "roadmap_propose_update" => self.announce(
                    |conn, project| propose_update_op(conn, project, id, args),
                    emit_proposal,
                ),
                "roadmap_propose_discard" => self.announce(
                    |conn, project| propose_discard_op(conn, project, id, args),
                    emit_proposal,
                ),
                "roadmap_propose_order" => self.announce(
                    |conn, project| propose_order_op(conn, project, id, args),
                    emit_order_proposal,
                ),
                "roadmap_note" => self.announce(
                    |conn, project| note_op(conn, project, id, args),
                    emit_item_event,
                ),
                "roadmap_hold" => self.announce(
                    |conn, project| hold_op(conn, project, id, args),
                    |app, held| match held {
                        Held::Item(item, event) => {
                            emit_item(app, item);
                            emit_item_event(app, event);
                        }
                        Held::Project(hold) => emit_project_hold(app, hold),
                    },
                ),
                "roadmap_brief" => {
                    let conn = self.db.lock();
                    (brief_op(&conn, &self.project_id, id, args), Vec::new())
                }
                "roadmap_propose_brief_update" => self.announce(
                    |conn, project| propose_brief_op(conn, project, id, args),
                    emit_brief_proposal,
                ),
                other => (
                    Response::err(
                        id,
                        format!(
                            "unknown roadmap op: {other} — this chat has {}",
                            OPS.join(", ")
                        ),
                    ),
                    Vec::new(),
                ),
            }
        })
    }
}

#[cfg(test)]
#[path = "tests/dispatcher.rs"]
mod tests;
