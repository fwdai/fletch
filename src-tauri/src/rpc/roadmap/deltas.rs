use rusqlite::Connection;
use serde_json::{json, Value};

use crate::roadmap::deps;
use crate::roadmap::proposals::{self, Proposal, ProposalKind, ProposalPatch};
use crate::roadmap::store;
use crate::roadmap::types::{ItemStatus, RoadmapItem};
use crate::rpc::Response;

use super::args::{clean, clean_list, parse_required, ProposeDiscardArgs, ProposeUpdateArgs};

/// Find the item an ask targets and check it may still be reshaped. Anything
/// from `active` on belongs to its run: a proposal against it would be ruled
/// on against work that no longer matches the diff. A `rejected` item is out
/// for the opposite reason — the user ruled it off the board, and an ask
/// against it would be lobbying against a decision already made.
///
/// `roadmap::rulings::proposal_gate` re-checks the same [`ItemStatus::is_rulable`]
/// set at ruling time, with its own user-facing message.
pub(super) fn proposable<'a>(
    items: &'a [RoadmapItem],
    code: &str,
) -> Result<&'a RoadmapItem, String> {
    let item = items.iter().find(|i| i.code == code).ok_or_else(|| {
        format!("no item {code:?} on this board — `roadmap_list` shows what exists")
    })?;
    if item.status.is_rulable() {
        Ok(item)
    } else {
        // Name the actual objection: telling the PM a rejected item is "being
        // built" would invite it to wait for a run that is never coming.
        let why = match item.status {
            ItemStatus::Done => "shipped work can't be reshaped by proposal",
            ItemStatus::Rejected => "the user ruled it off the board; only the user can reopen it",
            _ => "an item being built or reviewed can't be reshaped by proposal",
        };
        Err(format!(
            "{} is {} — {why}; use the codes `roadmap_list` shows as proposed, open, \
             or queued",
            item.code,
            item.status.as_str()
        ))
    }
}

/// Normalize and validate an update's patch against the board, or say exactly
/// what's wrong: same rules the batch propose applies, plus the two a patch can
/// break that a new ticket can't — depending on yourself, and closing a loop
/// with an item that already depends on you ([`deps::validate_edit`]).
///
/// Checked here *and* again when the user rules on the ask: the board moves in
/// between, and an accepted loop is a permanently wedged queue.
fn validate_patch(
    patch: &ProposalPatch,
    item: &RoadmapItem,
    board: &[RoadmapItem],
) -> Result<ProposalPatch, String> {
    if patch.is_empty() {
        return Err("`patch` must change at least one field — \
                    title | why | horizon | area | accept | deps"
            .into());
    }
    let mut out = patch.clone();
    if let Some(title) = &out.title {
        let title = title.trim();
        if title.is_empty() {
            return Err("`title` cannot be blank".into());
        }
        out.title = Some(title.to_string());
    }
    if let Some(why) = &out.why {
        out.why = Some(why.trim().to_string());
    }
    if let Some(Some(area)) = &out.area {
        let area = area.trim();
        if area.is_empty() {
            return Err("`area` cannot be blank — send `\"area\": null` to clear it".into());
        }
        out.area = Some(Some(area.to_string()));
    }
    if let Some(accept) = &out.accept {
        out.accept = Some(clean_list(accept));
    }
    if let Some(patched) = &out.deps {
        let patched = clean_list(patched);
        deps::validate_edit(
            &deps::graph_of(board),
            &deps::rejected_of(board),
            &item.code,
            &patched,
        )?;
        out.deps = Some(patched);
    }
    Ok(out)
}

/// `roadmap_propose_update`: park a validated patch as the item's pending
/// delta, replacing any it already has. Nothing is applied here — the user's
/// ruling does that — so there is no history event either: the ruling writes
/// history, not the ask.
///
/// Returns the stored proposal alongside the response so the dispatcher can
/// announce it (`roadmap:proposal`) after the lock drops.
pub(super) fn propose_update_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<Proposal>) {
    let err = |msg: String| {
        (
            Response::err(id, format!("roadmap_propose_update: {msg}")),
            None,
        )
    };
    let args: ProposeUpdateArgs = match parse_required(args) {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => return err(e.to_string()),
    };
    let item = match proposable(&items, args.code.trim()) {
        Ok(item) => item,
        Err(e) => return err(e),
    };
    let patch = match validate_patch(&args.patch, item, &items) {
        Ok(patch) => patch,
        Err(e) => return err(e),
    };
    let note = clean(args.note.as_deref());
    let stored = match proposals::upsert(
        conn,
        project_id,
        &item.id,
        ProposalKind::Update,
        Some(&patch),
        note.as_deref(),
    ) {
        Ok(p) => p,
        Err(e) => return err(e.to_string()),
    };

    let payload = json!({ "proposed": { "code": item.code, "fields": patch.fields() } });
    match serde_json::to_string(&payload) {
        Ok(stdout) => (Response::ok(id, 0, stdout, String::new()), Some(stored)),
        Err(e) => err(e.to_string()),
    }
}

/// `roadmap_propose_discard`: park a removal ask as the item's pending delta.
/// Same shape as the update — nothing is deleted until the user rules.
pub(super) fn propose_discard_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<Proposal>) {
    let err = |msg: String| {
        (
            Response::err(id, format!("roadmap_propose_discard: {msg}")),
            None,
        )
    };
    let args: ProposeDiscardArgs = match parse_required(args) {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let reason = args.reason.trim();
    if reason.is_empty() {
        return err("`reason` is required — say why this should leave the board".into());
    }
    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => return err(e.to_string()),
    };
    let item = match proposable(&items, args.code.trim()) {
        Ok(item) => item,
        Err(e) => return err(e),
    };
    let stored = match proposals::upsert(
        conn,
        project_id,
        &item.id,
        ProposalKind::Discard,
        None,
        Some(reason),
    ) {
        Ok(p) => p,
        Err(e) => return err(e.to_string()),
    };

    let payload = json!({ "proposed": { "code": item.code, "kind": "discard" } });
    match serde_json::to_string(&payload) {
        Ok(stdout) => (Response::ok(id, 0, stdout, String::new()), Some(stored)),
        Err(e) => err(e.to_string()),
    }
}

#[cfg(test)]
#[path = "tests/deltas.rs"]
mod tests;
