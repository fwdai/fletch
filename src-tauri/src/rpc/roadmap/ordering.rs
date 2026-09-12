use rusqlite::Connection;
use serde_json::{json, Value};

use crate::roadmap::order_proposals::{self, OrderProposal};
use crate::roadmap::store;
use crate::rpc::Response;

use super::args::{clean, clean_list, parked, parse_required, ProposeOrderArgs};

/// `roadmap_propose_order`: park a whole-board order ask, replacing any the
/// project already has.
///
/// The sequence must be *exactly* the board's orderable set — refused otherwise,
/// naming what's missing or what doesn't belong. That is what makes the ask mean
/// one thing: it IS the new backlog order, not a hint about part of one, so the
/// user can rule on it without reconstructing where the unnamed items went.
/// Nothing is applied here; the ruling rewrites the ranks.
pub(super) fn propose_order_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<OrderProposal>) {
    parked(
        id,
        "roadmap_propose_order",
        park_order(conn, project_id, args),
    )
}

fn park_order(
    conn: &Connection,
    project_id: &str,
    args: &Value,
) -> Result<(Value, OrderProposal), String> {
    let args: ProposeOrderArgs = parse_required(args)?;
    let items = store::list(conn, project_id).map_err(|e| e.to_string())?;
    let codes = clean_list(&args.codes);
    // Validated here *and* at ruling time, against the same function: the board
    // moves while an ask is pending, and the user's click must not apply a
    // sequence that no longer covers it.
    order_proposals::validate_order(&codes, &items)?;
    let note = clean(args.note.as_deref());
    let stored = order_proposals::upsert(conn, project_id, &codes, note.as_deref())
        .map_err(|e| e.to_string())?;
    Ok((json!({ "proposed": { "order": codes } }), stored))
}

#[cfg(test)]
#[path = "tests/ordering.rs"]
mod tests;
