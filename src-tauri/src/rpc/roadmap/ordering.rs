use rusqlite::Connection;
use serde_json::{json, Value};

use crate::roadmap::order_proposals::{self, OrderProposal};
use crate::roadmap::store;
use crate::rpc::Response;

use super::args::{clean, clean_list, parse_required, ProposeOrderArgs};

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
    let err = |msg: String| {
        (
            Response::err(id, format!("roadmap_propose_order: {msg}")),
            None,
        )
    };
    let args: ProposeOrderArgs = match parse_required(args) {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => return err(e.to_string()),
    };
    let codes = clean_list(&args.codes);
    // Validated here *and* at ruling time, against the same function: the board
    // moves while an ask is pending, and the user's click must not apply a
    // sequence that no longer covers it.
    if let Err(e) = order_proposals::validate_order(&codes, &items) {
        return err(e);
    }
    let note = clean(args.note.as_deref());
    let stored = match order_proposals::upsert(conn, project_id, &codes, note.as_deref()) {
        Ok(p) => p,
        Err(e) => return err(e.to_string()),
    };

    let payload = json!({ "proposed": { "order": codes } });
    match serde_json::to_string(&payload) {
        Ok(stdout) => (Response::ok(id, 0, stdout, String::new()), Some(stored)),
        Err(e) => err(e.to_string()),
    }
}

#[cfg(test)]
#[path = "tests/ordering.rs"]
mod tests;
