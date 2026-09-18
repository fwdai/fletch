use rusqlite::Connection;
use serde_json::{json, Value};

use crate::roadmap::events::{self, EventActor, EventKind, ItemEvent};
use crate::roadmap::store;
use crate::rpc::Response;

use super::args::{parse_required, wrote, NoteArgs};

const MAX_NOTE: usize = 500;

/// Allowed at any status: the observation most worth recording is on an
/// `active`/`in_review`/`done` item, exactly where a proposal is refused. It
/// advances nothing.
pub(super) fn note_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<ItemEvent>) {
    wrote(
        id,
        "roadmap_note",
        "recorded",
        record_note(conn, project_id, args),
    )
}

fn record_note(
    conn: &Connection,
    project_id: &str,
    args: &Value,
) -> Result<(Value, ItemEvent), String> {
    let args: NoteArgs = parse_required(args)?;
    let note = args.note.trim();
    if note.is_empty() {
        return Err("`note` is required — say what you observed, in one honest sentence".into());
    }
    // Characters, not bytes: a byte cap would refuse a shorter note for an em-dash.
    let length = note.chars().count();
    if length > MAX_NOTE {
        return Err(format!(
            "`note` is {length} characters — keep it under {MAX_NOTE}. A note is one observation; \
             if it needs more than that, it is a proposal"
        ));
    }
    let items = store::list(conn, project_id).map_err(|e| e.to_string())?;
    let code = args.code.trim();
    let item = items.iter().find(|i| i.code == code).ok_or_else(|| {
        format!("no item {code:?} on this board — `roadmap_list` shows what exists")
    })?;
    let event = events::record(
        conn,
        &item.id,
        project_id,
        EventActor::Pm,
        EventKind::Note,
        Some(note),
    )
    .map_err(|e| e.to_string())?;
    Ok((json!({ "noted": { "code": item.code } }), event))
}

#[cfg(test)]
#[path = "tests/notes.rs"]
mod tests;
