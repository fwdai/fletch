use rusqlite::Connection;
use serde_json::{json, Map, Value};

use crate::roadmap::memory::{self, BriefProposal};
use crate::rpc::Response;

use super::args::{clean, parked, parse_args, parse_required, read, BriefArgs, ProposeBriefArgs};
use super::listing::age;

/// The brief is injected at spawn; this re-read is for sessions that outlive it.
pub(super) fn brief_op(conn: &Connection, project_id: &str, id: &str, args: &Value) -> Response {
    read(id, "roadmap_brief", read_brief(conn, project_id, args))
}

fn read_brief(conn: &Connection, project_id: &str, args: &Value) -> Result<Value, String> {
    parse_args::<BriefArgs>(args).map_err(|e| format!("takes no args — {e}"))?;
    let brief = memory::load(conn, project_id).map_err(|e| e.to_string())?;
    Ok(match brief {
        // Explicit `null`, so "nothing written yet" can't read as a failed read.
        None => json!({ "brief": null }),
        Some(brief) => {
            let mut o = Map::new();
            o.insert("content".into(), json!(brief.content));
            if let Some(age) = age(crate::database::now_millis(), brief.updated_at) {
                o.insert("age".into(), json!(age));
            }
            json!({ "brief": Value::Object(o) })
        }
    })
}

/// Proposal-gated: a brief the user never read would let the agent launder its
/// conclusions into the user's position.
pub(super) fn propose_brief_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<BriefProposal>) {
    parked(
        id,
        "roadmap_propose_brief_update",
        park_brief(conn, project_id, args),
    )
}

fn park_brief(
    conn: &Connection,
    project_id: &str,
    args: &Value,
) -> Result<(Value, BriefProposal), String> {
    let args: ProposeBriefArgs = parse_required(args)?;
    let content = memory::clean_content(&args.content)?;
    let note = clean(args.note.as_deref());
    let stored =
        memory::propose(conn, project_id, &content, note.as_deref()).map_err(|e| e.to_string())?;
    Ok((
        json!({ "proposed": { "brief": { "bytes": content.len() } } }),
        stored,
    ))
}

#[cfg(test)]
#[path = "tests/brief.rs"]
mod tests;
