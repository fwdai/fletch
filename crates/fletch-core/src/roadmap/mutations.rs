//! Evented item create/update under the caller's connection lock.

use std::borrow::Cow;

use rusqlite::Connection;

use super::autonomy::{self, Landing};
use super::deps;
use super::events::{self, EventActor, EventKind, ItemEvent};
use super::store;
use super::types::{Horizon, ItemPatch, ItemSource, ItemStatus, ItemUpdate, NewItem, RoadmapItem};

/// Dep-check then insert + open event under caller's lock.
pub(super) fn create_checked(
    conn: &Connection,
    project_id: &str,
    item: &NewItem,
) -> Result<(RoadmapItem, ItemEvent), String> {
    if !item.deps.is_empty() {
        let board = store::list(conn, project_id).map_err(|e| e.to_string())?;
        deps::validate_new(
            &deps::graph_of(&board),
            &deps::rejected_of(&board),
            &item.deps,
        )?;
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let created = store::create(&tx, project_id, item).map_err(|e| e.to_string())?;
    let detail = (created.status == ItemStatus::Captured).then_some("captured");
    let event = events::record(
        &tx,
        &created.id,
        project_id,
        EventActor::User,
        EventKind::Created,
        detail,
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok((created, event))
}

/// Longest title a capture keeps; the whole text survives in `why` when the
/// first line runs past it.
pub(crate) const CAPTURE_TITLE_MAX: usize = 120;

/// A capture as a new row: the first line is the title, the rest the why. A
/// first line too long to be a title (a dictated note is often one line) is
/// clipped, and the full text goes to `why` so nothing is lost.
pub(crate) fn captured_item(text: &str) -> Result<NewItem, String> {
    let text = text.trim();
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    let first = first.trim();
    if first.is_empty() {
        return Err("a capture needs some text".into());
    }
    let (title, why) = if first.chars().count() > CAPTURE_TITLE_MAX {
        let clipped: String = first.chars().take(CAPTURE_TITLE_MAX - 1).collect();
        (format!("{}…", clipped.trim_end()), text.to_string())
    } else {
        (first.to_string(), rest.trim().to_string())
    };
    Ok(NewItem {
        title,
        why,
        horizon: Some(Horizon::Later),
        status: Some(ItemStatus::Captured),
        source: Some(ItemSource::User),
        ..Default::default()
    })
}

/// Apply patch + history under one lock; refuse writing `rejected` via generic edit.
pub(super) fn update_and_record(
    conn: &Connection,
    id: &str,
    patch: &ItemPatch,
    expect_status: Option<ItemStatus>,
    queue: bool,
) -> Result<(Option<ItemUpdate>, Option<ItemEvent>), String> {
    if patch.status == Some(ItemStatus::Rejected) {
        return Err(
            "an item is ruled off the board with roadmap_reject_item, which requires a \
             reason — not a status edit"
                .into(),
        );
    }
    if let Some(new_deps) = &patch.deps {
        if let Some(current) = store::get(conn, id).map_err(|e| e.to_string())? {
            deps::check_edit(conn, &current, new_deps)?;
        }
    }
    let landing = autonomy::is_accept(expect_status, patch)
        .then(|| autonomy::landing_for(conn, id, queue))
        .flatten();
    let mut effective = Cow::Borrowed(patch);
    if let Some(landing) = landing {
        effective.to_mut().status = Some(landing.status());
    }
    let updated = match expect_status {
        Some(expected) => store::update_where_status(conn, id, expected, &effective),
        None => store::update(conn, id, &effective),
    }
    .map_err(|e| e.to_string())?;
    match updated {
        Some(item) => {
            let kind = events::transition_kind(expect_status, patch.status);
            let event = events::record(
                conn,
                &item.id,
                &item.project_id,
                EventActor::User,
                kind,
                landing.and_then(Landing::detail),
            )
            .map_err(|e| e.to_string())?;
            Ok((
                Some(ItemUpdate {
                    applied: true,
                    item,
                }),
                Some(event),
            ))
        }
        None => match expect_status {
            Some(_) => Ok((
                store::get(conn, id)
                    .map_err(|e| e.to_string())?
                    .map(|item| ItemUpdate {
                        applied: false,
                        item,
                    }),
                None,
            )),
            None => Ok((None, None)),
        },
    }
}
