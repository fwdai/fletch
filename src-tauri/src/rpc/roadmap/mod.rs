//! The roadmap RPC ops: how the project-manager chat reads the board and puts
//! tickets on it.
//!
//! Same shape as [`crate::workflow::comms::WorkflowCommsDispatcher`]: this
//! dispatcher wraps the standard [`GitDispatcher`], owns the `roadmap_*` ops,
//! and delegates everything else — so the PM keeps the same read-only git
//! surface every other agent has (its `AgentCaps::advisory()` still refuses the
//! publish ops, one mechanism checked once).
//!
//! Nine ops, all scoped to the project this chat belongs to. The project id is
//! stamped at construction from the workspace record, never taken from `args`:
//! a chat can only ever read and write its own project's board.
//!
//! - `roadmap_list` — the whole board, compact: `items` in board order (which
//!   is rank order, i.e. dispatch order), including `done` items (the PM needs
//!   to know what already shipped before it proposes more). Each row carries
//!   its `last_event` and its PR link when it has one, so "why did MCA-104
//!   fail?" is answerable from this one call — the PM oversees execution, not
//!   just intake. Rejected items ride separately, under `not_doing`, as bare
//!   `code`/`title`/`close_reason` entries: they are the decision log, and
//!   mixing an archive entry into the live listing is how it gets mistaken for
//!   a workable row.
//! - `roadmap_propose` — creates rows with `status = "proposed"`, `source =
//!   "pm"`. A proposed row is a *ghost* on the board: it renders where it would
//!   land, counts for nothing, and only becomes real when the user accepts it
//!   (`proposed → open`) or vanishes when they discard it. That is the whole
//!   safety property of this tool — the agent can suggest, never commit. A
//!   batch item's `deps` may name another item in the same batch as `"#n"`,
//!   resolved to real codes inside the insert transaction, so an ordered plan
//!   is one call rather than one call per link. A title that looks like an
//!   item already on the board — a rejected one above all — comes back with a
//!   `warnings` line naming the match; the proposal still lands, because the
//!   user's ruling is the real gate and the PM is informed, never refused.
//! - `roadmap_propose_update` / `roadmap_propose_discard` — the same contract
//!   for items that already exist: the ask lands as a pending delta
//!   ([`crate::roadmap::proposals`], at most one per item, a newer one
//!   replacing it) that only the user's ruling applies. The PM can reshape the
//!   board it argued for without ever holding the pen.
//! - `roadmap_propose_order` — the same contract for the board's *order*: a
//!   whole-board ask ([`crate::roadmap::order_proposals`]) naming every orderable item in
//!   the sequence the PM argues for. Board scoped rather than item scoped, and
//!   refused unless it covers the orderable set exactly, so what the user rules
//!   on is unambiguous.
//! - `roadmap_brief` / `roadmap_propose_brief_update` — the PM's memory of the
//!   *product*, not of the board ([`crate::roadmap::memory`]). The brief is
//!   injected into this chat's instructions at spawn, so the read op exists for
//!   the sessions that outlive their spawn (a long conversation, a standup after
//!   the user ruled a change in). The write is proposal-gated like every other
//!   ask: the PM maintains its memory and the user owns it, so it cannot quietly
//!   rewrite the position it will cite back tomorrow.
//! - `roadmap_note` and `roadmap_hold` — the two ops that write *directly*, and
//!   the whole of the PM's direct-write licence (invariant 2 in
//!   .context/roadmap-pm-plan.md): it may raise a hand or pull the brake, never
//!   move a piece. `roadmap_note` advances nothing (a durable `note` on the
//!   item's history — attention, not action); `roadmap_hold` only ever *reduces*
//!   autonomy, stopping dispatch on one item or the whole board until the user
//!   signs off. There is deliberately no release op: releasing is the user's
//!   alone, so an agent can never lift its own brake.
//!
//! Validation rejects the whole batch rather than creating a partial one: the
//! PM gets one precise error it can fix and retry, and the user never sees half
//! a proposal. The inserts run in a transaction for the same reason.

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

use serde_json::Value;
use tauri::AppHandle;

use crate::roadmap::Db;
use crate::rpc::git::GitDispatcher;
use crate::rpc::{Response, RpcDispatcher, RpcEvent, RpcFuture};

use brakes::{hold_op, Held};
use brief::{brief_op, propose_brief_op};
use deltas::{propose_discard_op, propose_update_op};
use intake::propose_op;
use listing::list_op;
use notes::note_op;
use ordering::propose_order_op;

/// The ops this dispatcher owns. Pinned by a test against the instruction block
/// so the two can't drift — an agent told about an op that doesn't exist (or
/// given one it was never told about) is a silently broken tool.
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

/// Is `op` one this dispatcher owns? The whole `roadmap_` namespace, not just
/// the known names, so a typo'd op gets a precise error naming the real ones
/// instead of the git dispatcher's generic "unknown op".
fn is_roadmap_op(op: &str) -> bool {
    op.starts_with("roadmap_")
}

/// Adds the roadmap ops to a project-manager chat, over the standard git
/// dispatcher. Constructed in `supervisor::lifecycle` when a workspace's
/// `purpose` is `roadmap-pm`; no other agent is given one, which is why the ops
/// need no cap of their own.
pub struct RoadmapDispatcher {
    /// Where row changes are announced (`roadmap:item`), so the board follows a
    /// proposal live. `None` only in this module's tests, which have no window
    /// — the same shape `GitDispatcher::approval` uses.
    app: Option<AppHandle>,
    db: Db,
    /// The project this chat's board belongs to, stamped at spawn.
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
            // Every write op validates and stores under the lock, and announces
            // to the window only after the guard drops.
            match op {
                "roadmap_list" => {
                    let conn = self.db.lock();
                    (list_op(&conn, &self.project_id, id, args), Vec::new())
                }
                "roadmap_propose" => {
                    let (resp, created, recorded) = {
                        let conn = self.db.lock();
                        propose_op(&conn, &self.project_id, id, args)
                    };
                    if let Some(app) = &self.app {
                        for item in &created {
                            crate::roadmap::emit_item(app, item);
                        }
                        for event in &recorded {
                            crate::roadmap::emit_item_event(app, event);
                        }
                    }
                    (resp, Vec::new())
                }
                "roadmap_propose_update" | "roadmap_propose_discard" => {
                    let (resp, stored) = {
                        let conn = self.db.lock();
                        if op == "roadmap_propose_update" {
                            propose_update_op(&conn, &self.project_id, id, args)
                        } else {
                            propose_discard_op(&conn, &self.project_id, id, args)
                        }
                    };
                    if let (Some(app), Some(p)) = (&self.app, &stored) {
                        crate::roadmap::emit_proposal(app, p);
                    }
                    (resp, Vec::new())
                }
                "roadmap_propose_order" => {
                    let (resp, stored) = {
                        let conn = self.db.lock();
                        propose_order_op(&conn, &self.project_id, id, args)
                    };
                    if let (Some(app), Some(p)) = (&self.app, &stored) {
                        crate::roadmap::emit_order_proposal(app, p);
                    }
                    (resp, Vec::new())
                }
                "roadmap_note" => {
                    let (resp, recorded) = {
                        let conn = self.db.lock();
                        note_op(&conn, &self.project_id, id, args)
                    };
                    if let (Some(app), Some(event)) = (&self.app, &recorded) {
                        crate::roadmap::emit_item_event(app, event);
                    }
                    (resp, Vec::new())
                }
                "roadmap_hold" => {
                    let (resp, held) = {
                        let conn = self.db.lock();
                        hold_op(&conn, &self.project_id, id, args)
                    };
                    if let (Some(app), Some(held)) = (&self.app, &held) {
                        match held {
                            Held::Item(item, event) => {
                                crate::roadmap::emit_item(app, item);
                                crate::roadmap::emit_item_event(app, event);
                            }
                            Held::Project(hold) => crate::roadmap::emit_project_hold(app, hold),
                        }
                    }
                    (resp, Vec::new())
                }
                "roadmap_brief" => {
                    let conn = self.db.lock();
                    (brief_op(&conn, &self.project_id, id, args), Vec::new())
                }
                "roadmap_propose_brief_update" => {
                    let (resp, stored) = {
                        let conn = self.db.lock();
                        propose_brief_op(&conn, &self.project_id, id, args)
                    };
                    if let (Some(app), Some(p)) = (&self.app, &stored) {
                        crate::roadmap::emit_brief_proposal(app, p);
                    }
                    (resp, Vec::new())
                }
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
