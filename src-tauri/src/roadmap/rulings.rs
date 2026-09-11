//! User rulings: reject/reopen items, and accept/reject PM asks (item, order, brief).


use rusqlite::Connection;

use super::brakes;
use super::deps;
use super::events::{self, EventActor, EventKind, ItemEvent};
use super::order_proposals;
use super::proposals::{self, Proposal, ProposalKind, ProposalPatch};
use super::store;
use super::types::{ItemPatch, ItemStatus, ItemUpdate, RoadmapItem};

pub(super) fn reject_item(
    conn: &Connection,
    item_id: &str,
    reason: &str,
) -> Result<(RoadmapItem, ItemEvent, Option<Proposal>), String> {
    let reason = brakes::clean_reason_for(reason, brakes::ReasonKind::Reject)?;
    let current = store::require(conn, item_id)?;
    match current.status {
        ItemStatus::Proposed | ItemStatus::Open | ItemStatus::Queued => {}
        ItemStatus::Active | ItemStatus::InReview => {
            return Err(format!(
                "{} is {} — an agent is on it; cancel or settle the run before ruling \
                 this off the board",
                current.code,
                current.status.as_str()
            ))
        }
        ItemStatus::Done => {
            return Err(format!(
                "{} is done — shipped work can't be un-decided",
                current.code
            ))
        }
        ItemStatus::Rejected => {
            return Err(format!("{} is already rejected", current.code));
        }
    }
    let pending = proposals::for_item(conn, item_id).map_err(|e| e.to_string())?;
    if let Some(p) = &pending {
        proposals::delete(conn, &p.id).map_err(|e| e.to_string())?;
    }
    let item = store::reject(conn, item_id, &reason)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| store::missing(item_id))?;
    let event = events::record(
        conn,
        &item.id,
        &item.project_id,
        EventActor::User,
        EventKind::Rejected,
        Some(&reason),
    )
    .map_err(|e| e.to_string())?;
    Ok((item, event, pending))
}

pub(super) fn reopen_item(
    conn: &Connection,
    item_id: &str,
) -> Result<(RoadmapItem, Option<ItemEvent>, Vec<ItemEvent>), String> {
    let current = store::get(conn, item_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| store::missing(item_id))?;
    let Some(item) = store::reopen(conn, item_id).map_err(|e| e.to_string())? else {
        return Ok((current, None, Vec::new()));
    };
    let detail = match &current.close_reason {
        Some(reason) => format!("was rejected — {reason}"),
        None => "was rejected".to_string(),
    };
    let event = events::record(
        conn,
        &item.id,
        &item.project_id,
        EventActor::User,
        EventKind::Reopened,
        Some(&detail),
    )
    .map_err(|e| e.to_string())?;
    let corrections = correct_wedged_dependants(conn, &item)?;
    Ok((item, Some(event), corrections))
}

pub(super) fn correct_wedged_dependants(
    conn: &Connection,
    reopened: &RoadmapItem,
) -> Result<Vec<ItemEvent>, String> {
    let board = store::list(conn, &reopened.project_id).map_err(|e| e.to_string())?;
    let mut notes = Vec::new();
    for dep in board
        .iter()
        .filter(|i| i.status == ItemStatus::Queued && i.deps.contains(&reopened.code))
    {
        let latest = events::list_for_item(conn, &dep.id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .next();
        let wedged = latest.is_some_and(|e| {
            e.kind == EventKind::Blocked
                && e.detail
                    .as_deref()
                    .is_some_and(|d| d.contains(&reopened.code) && d.contains("rejected"))
        });
        if !wedged {
            continue;
        }
        let note = events::record(
            conn,
            &dep.id,
            &dep.project_id,
            EventActor::User,
            EventKind::Note,
            Some(&format!(
                "{} was reopened — waiting on it normally again",
                reopened.code
            )),
        )
        .map_err(|e| e.to_string())?;
        notes.push(note);
    }
    Ok(notes)
}

pub(super) enum Ruling {
    Updated {
        item: Box<RoadmapItem>,
        event: Box<ItemEvent>,
    },
    Stale { message: String },
}

/// Same rulable set as `rpc::roadmap::proposable` — keep in sync.
pub(crate) fn proposal_gate(item: &RoadmapItem) -> Result<(), String> {
    if item.status.is_rulable() {
        return Ok(());
    }
    let why = match item.status {
        ItemStatus::Done => "shipped work can't be reshaped by proposal",
        ItemStatus::Rejected => {
            "an item ruled off the board can't be reshaped by proposal — reopen it first"
        }
        _ => "an item being built or reviewed can't be reshaped by proposal",
    };
    Err(format!("{} is {} — {why}", item.code, item.status.as_str()))
}

pub(super) fn ruling_detail(verb: &str, note: Option<&str>) -> String {
    match note {
        Some(note) => format!("{verb} a PM proposal — {note}"),
        None => format!("{verb} a PM proposal"),
    }
}

pub(super) fn accept_proposal(conn: &Connection, proposal_id: &str) -> Result<Ruling, String> {
    let proposal = proposals::get(conn, proposal_id)
        .map_err(|e| e.to_string())?
        .ok_or("this proposal has already been ruled on")?;
    let item = store::get(conn, &proposal.item_id)
        .map_err(|e| e.to_string())?
        .ok_or("the item this proposal targets no longer exists")?;

    if let Err(message) = proposal_gate(&item) {
        proposals::delete(conn, proposal_id).map_err(|e| e.to_string())?;
        return Ok(Ruling::Stale { message });
    }

    match proposal.kind {
        ProposalKind::Update => {
            let patch: ProposalPatch =
                serde_json::from_value(proposal.patch.clone().ok_or("proposal carries no patch")?)
                    .map_err(|e| e.to_string())?;
            if let Some(new_deps) = &patch.deps {
                if let Err(why) = deps::check_edit(conn, &item, new_deps) {
                    proposals::delete(conn, proposal_id).map_err(|e| e.to_string())?;
                    return Ok(Ruling::Stale {
                        message: format!("the board changed since the PM asked — {why}"),
                    });
                }
            }
            let updated = store::update(conn, &item.id, &patch.to_item_patch())
                .map_err(|e| e.to_string())?
                .ok_or("the item this proposal targets no longer exists")?;
            finish_accept(
                conn,
                updated,
                EventKind::Edited,
                "Accepted",
                proposal.note.as_deref(),
                proposal_id,
            )
        }
        ProposalKind::Discard => {
            let reason = proposal
                .note
                .as_deref()
                .unwrap_or("discarded at the PM's ask");
            let rejected = store::reject(conn, &item.id, reason)
                .map_err(|e| e.to_string())?
                .ok_or("the item this proposal targets no longer exists")?;
            finish_accept(
                conn,
                rejected,
                EventKind::Rejected,
                "Rejected",
                proposal.note.as_deref(),
                proposal_id,
            )
        }
    }
}

fn finish_accept(
    conn: &Connection,
    item: RoadmapItem,
    kind: EventKind,
    verb: &str,
    note: Option<&str>,
    proposal_id: &str,
) -> Result<Ruling, String> {
    let event = events::record(
        conn,
        &item.id,
        &item.project_id,
        EventActor::User,
        kind,
        Some(&ruling_detail(verb, note)),
    )
    .map_err(|e| e.to_string())?;
    proposals::delete(conn, proposal_id).map_err(|e| e.to_string())?;
    Ok(Ruling::Updated {
        item: Box::new(item),
        event: Box::new(event),
    })
}

pub(super) fn reject_proposal(conn: &Connection, proposal_id: &str) -> Result<ItemEvent, String> {
    let proposal = proposals::get(conn, proposal_id)
        .map_err(|e| e.to_string())?
        .ok_or("this proposal has already been ruled on")?;
    proposals::delete(conn, proposal_id).map_err(|e| e.to_string())?;
    events::record(
        conn,
        &proposal.item_id,
        &proposal.project_id,
        EventActor::User,
        EventKind::Note,
        Some(&ruling_detail("Declined", proposal.note.as_deref())),
    )
    .map_err(|e| e.to_string())
}

pub(super) enum OrderRuling {
    Applied(Vec<RoadmapItem>),
    Stale(String),
}

pub(super) fn accept_order(conn: &Connection, project_id: &str) -> Result<OrderRuling, String> {
    let proposal = order_proposals::get(conn, project_id)
        .map_err(|e| e.to_string())?
        .ok_or("this order proposal has already been ruled on")?;
    let items = store::list(conn, project_id).map_err(|e| e.to_string())?;
    match order_proposals::validate_order(&proposal.codes, &items) {
        Err(message) => {
            order_proposals::delete(conn, project_id).map_err(|e| e.to_string())?;
            Ok(OrderRuling::Stale(format!(
                "the board changed since the PM proposed this order — {message}"
            )))
        }
        Ok(ids) => {
            let rows = store::set_ranks(conn, &ids).map_err(|e| e.to_string())?;
            order_proposals::delete(conn, project_id).map_err(|e| e.to_string())?;
            Ok(OrderRuling::Applied(rows))
        }
    }
}

