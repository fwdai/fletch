use rusqlite::Connection;
use serde_json::{json, Value};

use crate::roadmap::events::{self, EventActor, EventKind, ItemEvent};
use crate::roadmap::store;
use crate::rpc::Response;

use super::args::{parse_required, NoteArgs};

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
    let err = |msg: String| (Response::err(id, format!("roadmap_note: {msg}")), None);
    let args: NoteArgs = match parse_required(args) {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let note = args.note.trim();
    if note.is_empty() {
        return err("`note` is required — say what you observed, in one honest sentence".into());
    }
    // Counted in characters, not bytes: the cap is about how much a human will
    // read, and a byte limit would refuse a shorter note for containing an
    // em-dash.
    let length = note.chars().count();
    if length > MAX_NOTE {
        return err(format!(
            "`note` is {length} characters — keep it under {MAX_NOTE}. A note is one observation; \
             if it needs more than that, it is a proposal"
        ));
    }
    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => return err(e.to_string()),
    };
    let code = args.code.trim();
    let Some(item) = items.iter().find(|i| i.code == code) else {
        return err(format!(
            "no item {code:?} on this board — `roadmap_list` shows what exists"
        ));
    };
    let recorded = events::record(
        conn,
        &item.id,
        project_id,
        EventActor::Pm,
        EventKind::Note,
        Some(note),
    );
    let event = match recorded {
        Ok(event) => event,
        Err(e) => return err(e.to_string()),
    };

    let payload = json!({ "noted": { "code": item.code } });
    match serde_json::to_string(&payload) {
        Ok(stdout) => (Response::ok(id, 0, stdout, String::new()), Some(event)),
        // The note is on the card either way: say so, and still announce it.
        Err(e) => (
            Response::err(id, format!("roadmap_note: recorded, but {e}")),
            Some(event),
        ),
    }
}

#[cfg(test)]
#[path = "tests/notes.rs"]
mod tests;
