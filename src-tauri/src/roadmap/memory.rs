//! Product memory: what the PM knows about the product itself, across sessions
//! (migration 0034).
//!
//! **This module is a seam, not an answer.** Real product memory is a research
//! problem — what is worth remembering, how it stays true as the codebase moves,
//! which slice of it belongs in a given prompt. What is settled is the *shape of
//! the hole* it plugs into, and that is what this file fixes in place. Exactly
//! three surfaces cross the boundary:
//!
//! 1. **Load** — [`load`], used twice: the PM's spawn-time instruction block
//!    (`instructions::roadmap_block`, threaded from `supervisor::lifecycle`) and
//!    the `roadmap_brief` read op, which is how a long-lived chat or a standup
//!    re-reads the current state without being respawned.
//! 2. **Write** — [`propose`], behind the `roadmap_propose_brief_update` op. The
//!    PM may propose its own memory and never commit it: the user's ruling
//!    ([`accept`] / [`delete_proposal`], driven by the typed commands in
//!    [`super`]) is the only writer of [`save`]. That is invariant 2
//!    (.context/roadmap-pm-plan.md) applied to memory — a brief the agent could
//!    rewrite silently is a place for it to talk itself into a direction the user
//!    never agreed to, and then cite itself.
//! 3. **Render** — the same [`Brief`] the load returns, carried to the Product
//!    brief tab by `roadmap_get_brief` and `roadmap:brief`.
//!
//! Everything *behind* those three is replaceable. V1 is the most naive honest
//! implementation: one markdown document per project, written by the user's
//! ruling, injected whole. A future mental-model system (extracted entities,
//! per-turn retrieval, derived-from-the-repo domains) reimplements this module's
//! internals and its row shape; the RPC ops, the instruction wiring, the events
//! and the tab keep working, because none of them knows how the answer was made.
//!
//! Why a table and not `project_settings`: a brief is a document with a
//! timestamp, the settings table is one value column, and "how stale is the
//! memory" is a question both the tab and the PM ask. Why board-scoped proposals
//! rather than [`super::proposals`]: that table is *item* scoped
//! (`item_id NOT NULL` is what "one ask per item" is built on) — the same reason
//! the order ask has its own table ([`super::order`]), whose shape this mirrors.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::database::now_millis;

/// The project's product brief, as every surface sees it: the markdown, and when
/// the user last ruled a change in.
///
/// `project_id` rides along even though callers pass it in, for the same reason
/// [`super::holds::ProjectHold`] carries it: this struct *is* the
/// `roadmap:brief` payload, and a board-scoped listener has nothing else to
/// filter on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Brief {
    pub project_id: String,
    /// The brief itself — markdown, rendered by the tab and injected verbatim
    /// into the PM's instructions.
    pub content: String,
    pub updated_at: i64,
}

/// The PM's pending ask to replace the brief. One per project, replaced by a
/// newer one, applied only by the user's ruling — the order ask's grammar
/// ([`super::order::OrderProposal`]) at the same altitude.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BriefProposal {
    pub project_id: String,
    /// The *whole* proposed brief, not a diff: a partial memory update would be
    /// ambiguous about what it leaves behind, and the user is ruling on the
    /// document they will get.
    pub content: String,
    /// The PM's one line on what changed and why — quoted on the tab's bar.
    pub note: Option<String>,
    pub created_at: i64,
}

/// Longest brief either surface will store, in bytes.
///
/// A page of markdown is around 4 KiB, so this is deliberately generous: the cap
/// exists to refuse a *category* error — a brief that has become a dump of the
/// codebase, a transcript, or the board restated — not to police an honest
/// document. Counted in bytes because that is what "32 KiB" means; the number is
/// large enough that the character/byte distinction cannot decide a real case
/// (unlike a hold's 300-character reason, where an em-dash could).
pub const MAX_CONTENT: usize = 32 * 1024;

/// Normalize a proposed brief, or say exactly what is wrong with it.
///
/// The two refusals are the two ways the seam gets abused: an empty document
/// (which would erase the memory through a surface meant to improve it) and one
/// that has stopped being a brief. Both messages say what to do instead, because
/// the error text is the PM's only channel for fixing its own call.
pub fn clean_content(content: &str) -> Result<String, String> {
    let content = content.trim();
    if content.is_empty() {
        return Err(
            "`content` is required — send the whole brief you want the project to have, in \
             markdown. (There is no way to erase the brief from here: propose the version that \
             should stand instead.)"
                .into(),
        );
    }
    let bytes = content.len();
    if bytes > MAX_CONTENT {
        return Err(format!(
            "`content` is {bytes} bytes — keep the brief under {MAX_CONTENT} ({} KiB). It is a \
             page the user reads and re-reads: vision, domains, constraints, rejected \
             directions. Anything longer is either the board restated (the items are the \
             board's job) or a document that wanted to be one of them",
            MAX_CONTENT / 1024
        ));
    }
    Ok(content.to_string())
}

const BRIEF_COLUMNS: &str = "project_id, content, updated_at";
const PROPOSAL_COLUMNS: &str = "project_id, content, note, created_at";

impl Brief {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            project_id: r.get("project_id")?,
            content: r.get("content")?,
            updated_at: r.get("updated_at")?,
        })
    }
}

impl BriefProposal {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            project_id: r.get("project_id")?,
            content: r.get("content")?,
            note: r.get("note")?,
            created_at: r.get("created_at")?,
        })
    }
}

/// **Surface 1 (read).** The project's brief, or `None` when the PM has never
/// been given one. Must be called with the connection lock held.
///
/// The one function every reader goes through — the spawn-time injection, the
/// read op, the tab's fetch — so a future implementation has exactly one entry
/// point to replace.
pub fn load(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<Brief>> {
    conn.query_row(
        &format!("SELECT {BRIEF_COLUMNS} FROM roadmap_briefs WHERE project_id = ?1"),
        [project_id],
        Brief::from_row,
    )
    .optional()
}

/// Write the brief, replacing whatever stood before it, and return the stored
/// row for emitting after the lock drops.
///
/// Not reachable from the agent surface: the only caller is the accept path
/// ([`accept`]), which is driven by the user's typed command. `updated_at` is
/// therefore "when the user last ruled a change in", which is exactly the date
/// the tab should show — not when the PM drafted it.
pub fn save(conn: &Connection, project_id: &str, content: &str) -> rusqlite::Result<Brief> {
    conn.execute(
        "INSERT INTO roadmap_briefs (project_id, content, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(project_id) DO UPDATE SET
           content = excluded.content,
           updated_at = excluded.updated_at",
        params![project_id, content, now_millis()],
    )?;
    load(conn, project_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// **Surface 2 (write).** Park the PM's ask to replace the brief, replacing any
/// the project already has. Returns the stored row for emitting after the lock
/// drops. Must be called with the connection lock held.
///
/// Replacing rather than queueing: the user rules on the PM's *current* position,
/// the same way an item delta and an order ask work. Two pending versions of one
/// document would be two answers to one question.
pub fn propose(
    conn: &Connection,
    project_id: &str,
    content: &str,
    note: Option<&str>,
) -> rusqlite::Result<BriefProposal> {
    conn.execute(
        "INSERT INTO roadmap_brief_proposals (project_id, content, note, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(project_id) DO UPDATE SET
           content = excluded.content,
           note = excluded.note,
           created_at = excluded.created_at",
        params![project_id, content, note, now_millis()],
    )?;
    get_proposal(conn, project_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// The project's pending brief ask, if any — at most one by construction.
pub fn get_proposal(
    conn: &Connection,
    project_id: &str,
) -> rusqlite::Result<Option<BriefProposal>> {
    conn.query_row(
        &format!("SELECT {PROPOSAL_COLUMNS} FROM roadmap_brief_proposals WHERE project_id = ?1"),
        [project_id],
        BriefProposal::from_row,
    )
    .optional()
}

/// Remove the ask — the ruling took it. Returns whether a row was removed, so a
/// caller doesn't announce a deletion that didn't happen.
pub fn delete_proposal(conn: &Connection, project_id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM roadmap_brief_proposals WHERE project_id = ?1",
        [project_id],
    )?;
    Ok(n > 0)
}

/// Apply the pending ask: the brief becomes what the PM proposed, and the ask is
/// consumed. `None` when there was nothing pending (already ruled on, in another
/// window or a moment ago).
///
/// Both writes in the caller's single lock scope, so a brief can never be
/// replaced while its ask survives to be accepted twice. No re-validation of a
/// gate, unlike the order ask: a brief depends on nothing that can move
/// underneath it — the board's shape is not part of the document, which is the
/// point of keeping items out of it.
pub fn accept(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<Brief>> {
    let Some(proposal) = get_proposal(conn, project_id)? else {
        return Ok(None);
    };
    let brief = save(conn, project_id, &proposal.content)?;
    delete_proposal(conn, project_id)?;
    Ok(Some(brief))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::get_migrations;

    fn test_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        get_migrations().to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, created_at) VALUES ('p1', 'fletch', 0)",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn a_brief_round_trips_and_a_newer_one_replaces_it() {
        let conn = test_conn();
        assert!(load(&conn, "p1").unwrap().is_none(), "none until written");

        let first = save(&conn, "p1", "# Fletch\n\nAgents, supervised.").unwrap();
        assert_eq!(first.project_id, "p1");
        assert_eq!(first.content, "# Fletch\n\nAgents, supervised.");
        assert_eq!(load(&conn, "p1").unwrap(), Some(first));

        let second = save(&conn, "p1", "# Fletch\n\nRewritten.").unwrap();
        assert_eq!(second.content, "# Fletch\n\nRewritten.");
        assert_eq!(
            load(&conn, "p1").unwrap(),
            Some(second),
            "one document per project — the newer write IS the brief"
        );
    }

    #[test]
    fn an_ask_replaces_the_pending_one_and_deletes_once() {
        let conn = test_conn();
        assert!(get_proposal(&conn, "p1").unwrap().is_none());

        propose(&conn, "p1", "draft one", Some("first pass")).unwrap();
        let second = propose(&conn, "p1", "draft two", None).unwrap();
        assert_eq!(get_proposal(&conn, "p1").unwrap(), Some(second.clone()));
        assert_eq!(second.content, "draft two");
        assert_eq!(
            second.note, None,
            "the replacement's note wins, blank or not"
        );

        assert!(delete_proposal(&conn, "p1").unwrap());
        assert!(
            !delete_proposal(&conn, "p1").unwrap(),
            "second delete is a no-op"
        );
        assert!(get_proposal(&conn, "p1").unwrap().is_none());
    }

    #[test]
    fn accepting_writes_the_brief_and_consumes_the_ask() {
        let conn = test_conn();
        assert!(
            accept(&conn, "p1").unwrap().is_none(),
            "nothing pending is not an error — the ruling was already made"
        );

        propose(&conn, "p1", "## Domains\n\n- roadmap", Some("first")).unwrap();
        let brief = accept(&conn, "p1").unwrap().expect("applied");
        assert_eq!(brief.content, "## Domains\n\n- roadmap");
        assert_eq!(load(&conn, "p1").unwrap(), Some(brief));
        assert!(
            get_proposal(&conn, "p1").unwrap().is_none(),
            "an accepted ask cannot be accepted twice"
        );
    }

    /// Every way a proposed brief can be wrong, and the fact that a real one
    /// isn't. The refusal text is the PM's only way to fix its own call, so each
    /// one has to name the problem.
    #[test]
    fn only_a_usable_brief_is_accepted() {
        assert_eq!(
            clean_content("  # Vision\n\nShip it.  ").unwrap(),
            "# Vision\n\nShip it.",
            "trimmed, and otherwise verbatim markdown"
        );

        let empty = clean_content("   \n ").expect_err("blank is refused");
        assert!(empty.contains("required"), "{empty}");
        assert!(
            empty.contains("erase"),
            "must say the brief can't be cleared from here: {empty}"
        );

        let big = "x".repeat(MAX_CONTENT + 1);
        let over = clean_content(&big).expect_err("over the cap is refused");
        assert!(
            over.contains(&format!("{} bytes", MAX_CONTENT + 1)),
            "{over}"
        );
        assert!(over.contains("32 KiB"), "{over}");

        // Exactly at the cap is fine: the boundary belongs to the document.
        let at = "y".repeat(MAX_CONTENT);
        assert_eq!(clean_content(&at).unwrap().len(), MAX_CONTENT);
    }

    #[test]
    fn deleting_a_project_takes_its_brief_and_its_pending_ask() {
        let conn = test_conn();
        save(&conn, "p1", "the brief").unwrap();
        propose(&conn, "p1", "the ask", None).unwrap();

        conn.execute("DELETE FROM projects WHERE id = 'p1'", [])
            .unwrap();
        assert!(load(&conn, "p1").unwrap().is_none());
        assert!(get_proposal(&conn, "p1").unwrap().is_none());
    }
}
