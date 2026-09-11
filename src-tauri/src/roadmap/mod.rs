//! Roadmap Tauri commands + row-level emits. DAO is [`store`] (lock held).
//!
//! `roadmap_items` is deliberately absent from `database::validate` CRUD: codes
//! must be allocated under the connection lock, `*_json` marshalled here, and
//! every mutation must emit — a raw `db_insert` would skip all three.
//!
//! Emits are best-effort after the lock drops; a failed emit never rolls back
//! persistence. Hold *release* is user-only (invariant 2); the PM may place holds
//! but cannot lift them. Dep writes must stay acyclic ([`deps`]) or the queue wedges.

pub mod assignments;
pub mod autonomy;
pub mod brakes;
pub mod client_events;
mod commands;
pub mod deps;
pub mod drainer;
pub mod events;
pub mod memory;
pub mod merge_sweep;
mod mutations;
pub mod order_proposals;
pub mod pr_review;
pub mod proposals;
pub mod review;
pub mod rulings;
pub mod store;
pub mod types;

use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::Connection;

/// Shared with `rpc::roadmap`; same mutex handle.
pub type Db = Arc<Mutex<Connection>>;

pub use commands::*;

pub(crate) use client_events::{
    emit_brief_proposal, emit_item, emit_item_event, emit_order_proposal, emit_project_hold,
    emit_proposal,
};

pub(crate) use autonomy::Landing;
pub(crate) use brakes::{hold_with_event as hold_item, release_with_event as release_item};

#[cfg(test)]
#[path = "tests/commands.rs"]
mod tests;

// Names the command tests pull via `use super::*` (formerly parent imports).
#[cfg(test)]
use events::EventActor;
#[cfg(test)]
use merge_sweep::merge_hold_gate;
#[cfg(test)]
use mutations::{create_checked, update_and_record};
#[cfg(test)]
use proposals::{Proposal, ProposalKind, ProposalPatch};
#[cfg(test)]
use types::{ItemPatch, ItemStatus, ItemUpdate, NewItem, RoadmapItem};
