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

const MAX_BATCH: usize = 20;

/// `deps` may name a board code or another batch item as `"#n"` (1-based); the
/// merged graph must stay acyclic. `"#n"` survives untouched — only the insert
/// transaction knows the codes.
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
            workflow_def_id: None,
            // Only the issue funnel sets `issue_url`; a PM-supplied one would fake provenance.
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

/// Runs inside the insert transaction after every row exists, so forward
/// references resolve.
fn resolve_batch_deps(raw: &[String], created: &[RoadmapItem]) -> Vec<String> {
    raw.iter()
        .map(|d| match deps::batch_index(d, created.len()) {
            Some(i) => created[i].code.clone(),
            None => d.clone(),
        })
        .collect()
}

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
    // Against the pre-insert board, so a batch never warns about itself.
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
