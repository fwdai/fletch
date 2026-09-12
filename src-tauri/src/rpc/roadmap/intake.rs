use rusqlite::Connection;
use serde_json::{json, Map, Value};

use crate::roadmap::deps;
use crate::roadmap::events::{self, EventActor, EventKind, ItemEvent};
use crate::roadmap::store;
use crate::roadmap::types::{Horizon, ItemPatch, ItemSource, ItemStatus, NewItem, RoadmapItem};
use crate::rpc::Response;

use super::args::{clean, clean_list, one_of, parse_args, wrote, ProposeArgs, ProposedItem};
use super::duplicates::duplicate_warnings;

pub(super) type Proposed = (Vec<RoadmapItem>, Vec<ItemEvent>);

/// Most items one `roadmap_propose` call may carry. A proposal is a thing a
/// human reads and accepts; past a score of rows that stops being true, and the
/// PM should be slicing rather than dumping a backlog.
const MAX_BATCH: usize = 20;

/// Turn the agent's items into validated [`NewItem`]s, or explain what's wrong
/// with the batch. `existing` is the project's board: `deps` may name a code
/// already on it, or another item in *this* batch as `"#n"` (1-based), which is
/// what lets one call express an ordered plan. The whole merged graph — the
/// batch's own edges plus the board's — has to stay acyclic
/// ([`deps::validate_batch`]).
///
/// The `"#n"` entries survive into the returned [`NewItem`]s untouched; only the
/// insert transaction can resolve them, because that is where codes are
/// allocated (see [`resolve_batch_deps`]).
fn validate(items: &[ProposedItem], existing: &[RoadmapItem]) -> Result<Vec<NewItem>, String> {
    if items.is_empty() {
        return Err("`items` must be a non-empty array of tickets".into());
    }
    if items.len() > MAX_BATCH {
        return Err(format!(
            "{} items is too many for one proposal (max {MAX_BATCH}) — propose the next slice \
             once these are accepted",
            items.len()
        ));
    }
    let mut out: Vec<NewItem> = Vec::with_capacity(items.len());
    for (n, it) in items.iter().enumerate() {
        // 1-based: "item 1" is the first thing the agent wrote.
        let at = n + 1;
        let title = it.title.trim();
        if title.is_empty() {
            return Err(format!("item {at}: `title` is required"));
        }
        let horizon = match it.horizon.as_deref().map(str::trim) {
            None | Some("") => {
                return Err(format!(
                    "item {at} ({title:?}): `horizon` is required — {}",
                    one_of(&["now", "next", "later"])
                ))
            }
            Some(h) => Horizon::from_db(h).ok_or_else(|| {
                format!(
                    "item {at} ({title:?}): unknown horizon {h:?} — expected {}",
                    one_of(&["now", "next", "later"])
                )
            })?,
        };
        out.push(NewItem {
            title: title.to_string(),
            why: it.why.trim().to_string(),
            horizon: Some(horizon),
            status: Some(ItemStatus::Proposed),
            area: clean(it.area.as_deref()),
            source: Some(ItemSource::Pm),
            accept: clean_list(&it.accept),
            deps: clean_list(&it.deps),
            // Which workflow builds it is the user's call, not the PM's.
            workflow_def_id: None,
            // Only the issue funnel's create call sets `issue_url`: a PM-supplied
            // tracker URL would fake provenance and collide with the funnel's dedup key.
            issue_url: None,
        });
    }
    // Whole batch at once: an intra-batch loop is only visible at that scope.
    let lists: Vec<Vec<String>> = out.iter().map(|n| n.deps.clone()).collect();
    deps::validate_batch(
        &deps::graph_of(existing),
        &deps::rejected_of(existing),
        &lists,
    )
    .map_err(|r| format!("item {} ({:?}): {}", r.at + 1, out[r.at].title, r.message))?;
    Ok(out)
}

/// Rewrite a batch item's `"#n"` references into the codes the insert allocated.
///
/// Called *inside* the insert transaction, once every row exists: `"#2"` means
/// "the second ticket in this call", and only the transaction knows what code
/// that ticket got. Forward references work for the same reason — the rewrite
/// happens after all the inserts, not during them.
///
/// A dep that isn't a batch reference is left exactly as written; validation has
/// already established it is a code on the board.
fn resolve_batch_deps(raw: &[String], created: &[RoadmapItem]) -> Vec<String> {
    raw.iter()
        .map(|d| match deps::batch_index(d, created.len()) {
            Some(i) => created[i].code.clone(),
            None => d.clone(),
        })
        .collect()
}

/// `roadmap_propose`: validate the batch, insert it as `proposed` rows in one
/// transaction, and hand back the allocated codes — plus a `warnings` array
/// when a title looks like an item already on the board
/// ([`duplicate_warnings`]), so the PM learns about the near-duplicate in the
/// same breath as the codes and can raise it instead of ignoring it.
///
/// Returns the created rows — and the `proposed` history events recorded with
/// them — alongside the response so the caller can announce both to the
/// frontend: the board grows ghost rows live, mid-conversation.
pub(super) fn propose_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<Proposed>) {
    wrote(
        id,
        "roadmap_propose",
        "created",
        insert_batch(conn, project_id, args),
    )
}

fn insert_batch(
    conn: &Connection,
    project_id: &str,
    args: &Value,
) -> Result<(Value, Proposed), String> {
    let args: ProposeArgs = parse_args(args)?;
    let existing = store::list(conn, project_id).map_err(|e| e.to_string())?;
    let news = validate(&args.items, &existing)?;
    // Computed against the board the batch was validated against, before the
    // insert — a batch must not warn about itself.
    let warnings = duplicate_warnings(&news, &existing);

    // All or nothing. The `proposed` events and the `"#n"` rewrite ride the same
    // transaction: a ghost never exists without its record, a plan never half-applies.
    let (created, recorded) = (|| -> rusqlite::Result<Proposed> {
        let tx = conn.unchecked_transaction()?;
        let mut created = Vec::with_capacity(news.len());
        let mut recorded = Vec::with_capacity(news.len());
        for new in &news {
            let item = store::create(&tx, project_id, new)?;
            recorded.push(events::record(
                &tx,
                &item.id,
                project_id,
                EventActor::Pm,
                EventKind::Proposed,
                None,
            )?);
            created.push(item);
        }
        // Second pass: every ticket has a code now, so `"#n"` can resolve.
        for (n, new) in news.iter().enumerate() {
            if !new.deps.iter().any(|d| d.starts_with(deps::BATCH_PREFIX)) {
                continue;
            }
            let resolved = resolve_batch_deps(&new.deps, &created);
            if let Some(row) = store::update(
                &tx,
                &created[n].id,
                &ItemPatch {
                    deps: Some(resolved),
                    ..Default::default()
                },
            )? {
                created[n] = row;
            }
        }
        tx.commit()?;
        Ok((created, recorded))
    })()
    .map_err(|e| e.to_string())?;

    let mut payload = Map::new();
    payload.insert(
        "created".into(),
        json!(created
            .iter()
            .map(|i| json!({ "code": i.code, "title": i.title }))
            .collect::<Vec<_>>()),
    );
    // Absent rather than empty, so it can't read as a partial failure.
    if !warnings.is_empty() {
        payload.insert("warnings".into(), json!(warnings));
    }
    Ok((Value::Object(payload), (created, recorded)))
}

#[cfg(test)]
#[path = "tests/intake.rs"]
mod tests;
