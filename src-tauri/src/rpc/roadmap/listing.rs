use std::collections::{HashMap, HashSet};

use rusqlite::Connection;
use serde_json::{json, Map, Value};

use crate::roadmap::events::{self, ItemEvent};
use crate::roadmap::memory;
use crate::roadmap::proposals::{self, Proposal};
use crate::roadmap::store;
use crate::roadmap::types::{ItemStatus, RoadmapItem};
use crate::rpc::Response;

use super::args::{one_of, parse_args, read, ListArgs};

/// The fields the agent reasons about, empties omitted; ids, timestamps and run
/// back-links stay hidden. `age` is relative: an epoch means nothing to
/// "since we last spoke".
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
    // Quoted only while the item can still be ruled; a stale ask would read as
    // "still waiting on the user".
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

/// Coarsest true unit; `None` under a minute or for a clock that ran backwards.
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

fn compact_rejected(item: &RoadmapItem) -> Value {
    let mut o = Map::new();
    o.insert("code".into(), json!(item.code));
    o.insert("title".into(), json!(item.title));
    if let Some(reason) = &item.close_reason {
        o.insert("close_reason".into(), json!(reason));
    }
    Value::Object(o)
}

/// Rejected rows never ride `items`; they arrive under `not_doing` in
/// [`memory::not_doing`]'s order and cap.
pub(super) fn list_op(conn: &Connection, project_id: &str, id: &str, args: &Value) -> Response {
    read(id, "roadmap_list", board(conn, project_id, args))
}

fn board(conn: &Connection, project_id: &str, args: &Value) -> Result<Value, String> {
    let args: ListArgs = parse_args(args)?;
    let filter = match &args.status {
        None => None,
        Some(raw) => {
            let mut set = HashSet::new();
            for s in raw {
                match ItemStatus::from_db(s.trim()) {
                    // `rejected` parses but is refused: a filter that always matched nothing
                    // would read as an empty decision log.
                    Some(ItemStatus::Rejected) => {
                        return Err("`status` filters the live board — rejected items always \
                                    arrive under `not_doing`, so there is nothing to filter for"
                            .into())
                    }
                    Some(st) => {
                        set.insert(st.as_str());
                    }
                    None => {
                        return Err(format!(
                            "unknown status {s:?} — expected {}",
                            one_of(&["proposed", "open", "queued", "active", "in_review", "done"])
                        ))
                    }
                }
            }
            Some(set)
        }
    };

    let items = store::list(conn, project_id).map_err(|e| e.to_string())?;
    let pending = proposals::list_for_project(conn, project_id).map_err(|e| e.to_string())?;
    let by_item: HashMap<&str, &Proposal> =
        pending.iter().map(|p| (p.item_id.as_str(), p)).collect();
    // One query for the whole board, not one per row.
    let last = events::latest_by_item(conn, project_id).map_err(|e| e.to_string())?;
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
    Ok(Value::Object(payload))
}

#[cfg(test)]
#[path = "tests/listing.rs"]
mod tests;
