use rusqlite::Connection;
use serde_json::{json, Value};

use crate::roadmap::order_proposals::{self, OrderProposal};
use crate::roadmap::store;
use crate::rpc::Response;

use super::args::{clean, clean_list, parked, parse_required, ProposeOrderArgs};

/// Refused unless `codes` is exactly the orderable set, so the ask is
/// unambiguous. Nothing is applied here.
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
    // Also validated at ruling time: the board moves while an ask is pending.
    order_proposals::validate_order(&codes, &items)?;
    let note = clean(args.note.as_deref());
    let stored = order_proposals::upsert(conn, project_id, &codes, note.as_deref())
        .map_err(|e| e.to_string())?;
    Ok((json!({ "proposed": { "order": codes } }), stored))
}

#[cfg(test)]
#[path = "tests/ordering.rs"]
mod tests;
