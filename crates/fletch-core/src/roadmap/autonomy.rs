//! Autonomy dial: where an accepted item lands on the board vs the queue.
//!
//! Holds trump autoqueue (invariant 2).

use rusqlite::Connection;

use super::brakes;
use super::drainer;
use super::store;
use super::types::{ItemPatch, ItemStatus};

pub(super) fn is_accept(expect_status: Option<ItemStatus>, patch: &ItemPatch) -> bool {
    expect_status == Some(ItemStatus::Proposed) && patch.status == Some(ItemStatus::Open)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Landing {
    Board,
    Queue,
    HeldBack,
}

impl Landing {
    pub(super) fn status(self) -> ItemStatus {
        match self {
            Landing::Queue => ItemStatus::Queued,
            Landing::Board | Landing::HeldBack => ItemStatus::Open,
        }
    }

    pub(super) fn detail(self) -> Option<&'static str> {
        match self {
            Landing::Board => None,
            Landing::Queue => Some("auto-queued"),
            Landing::HeldBack => Some("left off the queue — this is held"),
        }
    }
}

pub(crate) fn accept_landing(queue_requested: bool, autoqueue: bool, held: bool) -> Landing {
    if !queue_requested && !autoqueue {
        return Landing::Board;
    }
    if held {
        return Landing::HeldBack;
    }
    Landing::Queue
}

pub(super) fn landing_for(conn: &Connection, id: &str, queue: bool) -> Option<Landing> {
    let item = store::get(conn, id).ok().flatten()?;
    Some(accept_landing(
        queue,
        drainer::autoqueue(conn, &item.project_id),
        brakes::gate(conn, &item).is_some(),
    ))
}
