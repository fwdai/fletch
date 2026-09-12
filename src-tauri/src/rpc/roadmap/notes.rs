use rusqlite::Connection;
use serde_json::{json, Value};

use crate::roadmap::events::{self, EventActor, EventKind, ItemEvent};
use crate::roadmap::store;
use crate::rpc::Response;

use super::args::{parse_required, wrote, NoteArgs};

/// Longest note this op will store. A note is a line on a card and a line in the
/// PM's next listing — past a couple of sentences it stops being an observation
/// and starts being an essay nobody reads, and the thing it should have been is
/// a proposal.
const MAX_NOTE: usize = 500;

/// `roadmap_note`: record one durable observation on an item.
///
/// The PM's only direct write, and it is allowed precisely because it advances
/// nothing: no status moves, no field changes, no queue is touched. It raises
/// attention — the conservative direction of invariant 2 — where every ask that
/// would *do* something stays a proposal the user rules on.
///
/// Unlike the propose ops, the target may be at **any** status. The observation
/// worth recording most often concerns an item that is already `active`,
/// `in_review` or `done` ("this shipped, but it solved a narrower problem than
/// MCA-104 asked for"), and that is exactly the item a proposal is refused on.
/// Refusing the note too would leave the PM with nowhere to put the one thing it
/// is uniquely positioned to notice.
///
/// Returns the recorded event alongside the response so the dispatcher can
/// announce it (`roadmap:item-event`) once the lock drops — the card's trail
/// grows mid-conversation.
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
    // Counted in characters, not bytes: the cap is about how much a human will
    // read, and a byte limit would refuse a shorter note for containing an
    // em-dash.
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
