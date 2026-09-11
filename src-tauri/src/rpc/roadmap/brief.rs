use rusqlite::Connection;
use serde_json::{json, Map, Value};

use crate::roadmap::memory::{self, BriefProposal};
use crate::rpc::Response;

use super::args::{clean, parse_args, parse_required, BriefArgs, ProposeBriefArgs};
use super::listing::age;

/// `roadmap_brief`: read the project's product brief.
///
/// Redundant at spawn — the brief is already in this chat's instructions
/// (`instructions::roadmap_block`) — and that is exactly why the op exists: a PM
/// chat outlives its spawn. The user rules a change in mid-conversation, a
/// standup opens hours later, another window edits the board; the injected copy
/// is then the *old* memory, and an agent citing it is worse than one that
/// re-reads.
///
/// `age` rather than the raw `updated_at`, for the same reason `last_event`
/// carries one: an epoch means nothing to an agent reasoning about "since we last
/// spoke", and a timestamp it has to diff itself is arithmetic waiting to go
/// wrong. Absent when the brief was written in the last minute.
pub(super) fn brief_op(conn: &Connection, project_id: &str, id: &str, args: &Value) -> Response {
    if let Err(e) = parse_args::<BriefArgs>(args) {
        return Response::err(id, format!("roadmap_brief: takes no args — {e}"));
    }
    let brief = match memory::load(conn, project_id) {
        Ok(brief) => brief,
        Err(e) => return Response::err(id, format!("roadmap_brief: {e}")),
    };
    let payload = match brief {
        // The empty marker: this project has no product memory yet. Explicit
        // rather than an absent key, so "nothing written yet" can't be misread as
        // a failed read.
        None => json!({ "brief": null }),
        Some(brief) => {
            let mut o = Map::new();
            o.insert("content".into(), json!(brief.content));
            if let Some(age) = age(crate::database::now_millis(), brief.updated_at) {
                o.insert("age".into(), json!(age));
            }
            json!({ "brief": Value::Object(o) })
        }
    };
    match serde_json::to_string(&payload) {
        Ok(stdout) => Response::ok(id, 0, stdout, String::new()),
        Err(e) => Response::err(id, format!("roadmap_brief: {e}")),
    }
}

/// `roadmap_propose_brief_update`: park an ask to replace the product brief,
/// replacing any the project already has.
///
/// Proposal-gated for the reason the whole memory seam is worth building on top of
/// this grammar: the brief is what the PM will quote back as "what we agreed", so
/// a version of it the user never read is a way for the agent to launder its own
/// conclusions into the user's position. Nothing here writes memory — the typed
/// ruling (`roadmap_accept_brief_proposal`) does.
///
/// Board scoped, like the order ask: no item event, and the ask's own row is the
/// durable object until the user rules.
pub(super) fn propose_brief_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<BriefProposal>) {
    let err = |msg: String| {
        (
            Response::err(id, format!("roadmap_propose_brief_update: {msg}")),
            None,
        )
    };
    let args: ProposeBriefArgs = match parse_required(args) {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let content = match memory::clean_content(&args.content) {
        Ok(content) => content,
        Err(e) => return err(e),
    };
    let note = clean(args.note.as_deref());
    let stored = match memory::propose(conn, project_id, &content, note.as_deref()) {
        Ok(p) => p,
        Err(e) => return err(e.to_string()),
    };

    let payload = json!({ "proposed": { "brief": { "bytes": content.len() } } });
    match serde_json::to_string(&payload) {
        Ok(stdout) => (Response::ok(id, 0, stdout, String::new()), Some(stored)),
        Err(e) => err(e.to_string()),
    }
}

#[cfg(test)]
#[path = "tests/brief.rs"]
mod tests;
