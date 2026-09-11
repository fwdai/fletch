use std::collections::{HashMap, HashSet};

use rusqlite::Connection;
use serde_json::{json, Map, Value};

use crate::roadmap::events::{self, ItemEvent};
use crate::roadmap::memory;
use crate::roadmap::proposals::{self, Proposal};
use crate::roadmap::store;
use crate::roadmap::types::{ItemStatus, RoadmapItem};
use crate::rpc::Response;

use super::args::{one_of, parse_args, ListArgs};

/// One row as the agent sees it: the fields it reasons about, with the empties
/// omitted. Not [`RoadmapItem`]'s full serialization — ids, timestamps and run
/// back-links are the app's business, and the agent addresses items by `code`.
///
/// `pending` is the item's outstanding delta, if any, summarized as
/// `pending_proposal` — so the PM knows what it has already asked for and
/// never re-proposes blind (or mistakes "not applied yet" for "declined").
/// A hold projects the same way, as `held`: the brake is a state the PM can set
/// and cannot lift, so it has to be able to read it.
///
/// `last` is the item's newest history row, projected as `last_event`. This is
/// what turns the listing from an intake queue into an execution report: the
/// status says *where* an item is, the last event says *what happened* — a
/// failure reason, a workflow, a note somebody left. `age` is relative on
/// purpose, computed against `now`: an absolute epoch means nothing to an agent
/// reasoning about "since we last spoke", and a wall-clock timestamp it would
/// have to diff itself is a round trip and a mistake waiting to happen.
///
/// The PR link rides along as `pr` for the same reason — the diff is where a
/// review actually happens, and the item's own `status` already says whether
/// that PR is still open (`in_review`) or landed (`done`), so no polled state is
/// invented here. Raw ids stay hidden throughout: run ids, item ids and PR
/// numbers are the app's handles, not the PM's vocabulary.
fn compact(
    item: &RoadmapItem,
    pending: Option<&Proposal>,
    last: Option<&ItemEvent>,
    now: i64,
) -> Value {
    let mut o = Map::new();
    o.insert("code".into(), json!(item.code));
    o.insert("title".into(), json!(item.title));
    o.insert("horizon".into(), json!(item.horizon.as_str()));
    o.insert("status".into(), json!(item.status.as_str()));
    if !item.why.is_empty() {
        o.insert("why".into(), json!(item.why));
    }
    if let Some(area) = &item.area {
        o.insert("area".into(), json!(area));
    }
    if !item.accept.is_empty() {
        o.insert("accept".into(), json!(item.accept));
    }
    if !item.deps.is_empty() {
        o.insert("deps".into(), json!(item.deps));
    }
    if let Some(e) = last {
        let mut le = Map::new();
        le.insert("kind".into(), json!(e.kind.as_str()));
        if let Some(detail) = &e.detail {
            le.insert("detail".into(), json!(detail));
        }
        if let Some(age) = age(now, e.created_at) {
            le.insert("age".into(), json!(age));
        }
        o.insert("last_event".into(), Value::Object(le));
    }
    if let Some(url) = item
        .pr_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        o.insert("pr".into(), json!({ "url": url }));
    }
    // `by` matters: only one of the two answers is the PM's own doing.
    if let Some(reason) = &item.hold_reason {
        let mut h = Map::new();
        h.insert("reason".into(), json!(reason));
        if let Some(by) = item.held_by {
            h.insert("by".into(), json!(by.as_str()));
        }
        o.insert("held".into(), Value::Object(h));
    }
    // Quoted only while the item can still be ruled: an ask whose item has
    // advanced past the gate has no card to rule it from, and quoting it
    // forever would read as "still waiting on the user" when nothing is.
    if let Some(p) = pending.filter(|_| item.status.is_rulable()) {
        let mut pp = Map::new();
        pp.insert("kind".into(), json!(p.kind.as_str()));
        if let Some(note) = &p.note {
            pp.insert("note".into(), json!(note));
        }
        let fields = p.fields();
        if !fields.is_empty() {
            pp.insert("fields".into(), json!(fields));
        }
        o.insert("pending_proposal".into(), Value::Object(pp));
    }
    Value::Object(o)
}

/// How long ago something happened, in the coarsest unit that is still true:
/// `"4m"`, `"2h"`, `"3d"`. `None` for anything under a minute (and for a clock
/// that ran backwards) — "just now" is what the absence means, and inventing
/// `"0m"` would read as staler than it is.
///
/// Coarse deliberately: the PM reasons in "since we last spoke", and a precise
/// duration would invite arithmetic it has no reason to do.
pub(super) fn age(now: i64, then: i64) -> Option<String> {
    let ms = now.checked_sub(then).filter(|d| *d > 0)?;
    let minutes = ms / 60_000;
    match minutes {
        0 => None,
        m if m < 60 => Some(format!("{m}m")),
        m if m < 60 * 24 => Some(format!("{}h", m / 60)),
        m => Some(format!("{}d", m / (60 * 24))),
    }
}

/// One rejected row, as the archive shows it: the decision and its reason,
/// stripped of everything that would make it look workable — no horizon, no
/// deps, no status the PM could try to advance.
fn compact_rejected(item: &RoadmapItem) -> Value {
    let mut o = Map::new();
    o.insert("code".into(), json!(item.code));
    o.insert("title".into(), json!(item.title));
    if let Some(reason) = &item.close_reason {
        o.insert("close_reason".into(), json!(reason));
    }
    Value::Object(o)
}

/// `roadmap_list`: the project's board on stdout — the live rows under
/// `items`, and the decision log under `not_doing`.
///
/// Rejected rows are deliberately *not* in `items`: an archive entry that
/// renders like a live row is how the PM comes to treat a killed idea as a
/// workable one. They arrive as their own key, in [`memory::not_doing`]'s
/// order and under its cap — the same selection the spawn-time digest makes,
/// so the two surfaces can't tell different stories.
///
/// Pure over the connection so it is testable without an app handle; the
/// dispatcher holds the lock around it.
pub(super) fn list_op(conn: &Connection, project_id: &str, id: &str, args: &Value) -> Response {
    let args: ListArgs = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return Response::err(id, format!("roadmap_list: {e}")),
    };
    let filter = match &args.status {
        None => None,
        Some(raw) => {
            let mut set = HashSet::new();
            for s in raw {
                match ItemStatus::from_db(s.trim()) {
                    // `rejected` parses, and is refused anyway: it is not a
                    // live status, and a filter that always matched nothing
                    // would read as "the decision log is empty".
                    Some(ItemStatus::Rejected) => {
                        return Response::err(
                            id,
                            "roadmap_list: `status` filters the live board — rejected items \
                             always arrive under `not_doing`, so there is nothing to filter for"
                                .to_string(),
                        )
                    }
                    Some(st) => {
                        set.insert(st.as_str());
                    }
                    None => {
                        return Response::err(
                            id,
                            format!(
                                "roadmap_list: unknown status {s:?} — expected {}",
                                one_of(&[
                                    "proposed",
                                    "open",
                                    "queued",
                                    "active",
                                    "in_review",
                                    "done"
                                ])
                            ),
                        )
                    }
                }
            }
            Some(set)
        }
    };

    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => return Response::err(id, format!("roadmap_list: {e}")),
    };
    let pending = match proposals::list_for_project(conn, project_id) {
        Ok(list) => list,
        Err(e) => return Response::err(id, format!("roadmap_list: {e}")),
    };
    let by_item: HashMap<&str, &Proposal> =
        pending.iter().map(|p| (p.item_id.as_str(), p)).collect();
    // One query for the whole board, not one per row.
    let last = match events::latest_by_item(conn, project_id) {
        Ok(map) => map,
        Err(e) => return Response::err(id, format!("roadmap_list: {e}")),
    };
    let now = crate::database::now_millis();
    let keep = |i: &RoadmapItem| match &filter {
        None => true,
        Some(f) => f.contains(i.status.as_str()),
    };
    let rows: Vec<Value> = items
        .iter()
        .filter(|i| i.status != ItemStatus::Rejected)
        .filter(|i| keep(i))
        .map(|i| compact(i, by_item.get(i.id.as_str()).copied(), last.get(&i.id), now))
        .collect();
    let mut payload = Map::new();
    payload.insert("items".into(), json!(rows));
    let (rejected, omitted) = memory::not_doing(&items);
    if !rejected.is_empty() {
        payload.insert(
            "not_doing".into(),
            json!(rejected
                .iter()
                .map(|i| compact_rejected(i))
                .collect::<Vec<_>>()),
        );
        if omitted > 0 {
            payload.insert("not_doing_omitted".into(), json!(omitted));
        }
    }
    match serde_json::to_string(&Value::Object(payload)) {
        Ok(stdout) => Response::ok(id, 0, stdout, String::new()),
        Err(e) => Response::err(id, format!("roadmap_list: {e}")),
    }
}

#[cfg(test)]
#[path = "tests/listing.rs"]
mod tests;
