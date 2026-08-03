//! Pending PM deltas: the `roadmap_proposals` DAO (migration 0031).
//!
//! Why this exists: the PM may suggest, never commit. A new ticket already has
//! a proposal shape (a `proposed` item — a ghost row), but a *revision* to an
//! item that exists — a retitle, a re-slice, a retirement — has nowhere to live
//! without either duplicating the item or letting the PM write it directly.
//! This table is the middle: one pending delta per item (`UNIQUE(item_id)`),
//! written by the PM's RPC ops and resolved only by the user's typed ruling
//! commands, which apply or drop it and record the ruling as item history.
//!
//! A newer proposal *replaces* the pending one rather than queueing behind it:
//! the user rules on the PM's current position, not on a backlog of superseded
//! asks. The replacement keeps the row's `id` (upsert, not delete-and-insert),
//! so the frontend's upsert-by-id sequencer swaps the ask in place instead of
//! ever holding two for one item.
//!
//! Like every roadmap table, absent from the generic CRUD allow-list: a
//! proposal that didn't ride the validated RPC path could carry a patch the
//! ruling would refuse to apply.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{double_option, enum_col, Horizon, ItemPatch};
use crate::database::now_millis;

crate::db_enum! {
    /// What the PM is asking for: patch the item, or remove it from the board.
    ProposalKind {
        Update  => "update",
        Discard => "discard",
    }
}

/// The delta an `update` proposal may carry — the item's *shape*, never its
/// lifecycle. Deliberately not [`ItemPatch`]: status, source, and the run
/// back-links are the app's (and the user's) to move, and `code` is identity.
/// Unknown fields are rejected rather than silently dropped, exactly like the
/// propose op's items — a misspelled `horizen` must fail loudly.
///
/// Same wire semantics as [`ItemPatch`]: an absent field is left alone, and an
/// explicit `null` on `area` clears it ([`double_option`]). Serialization
/// skips absent fields, so the stored `patch_json`'s keys are exactly the
/// fields the proposal changes — which is what the compact listing and the
/// card's diff both quote.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizon: Option<Horizon>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub area: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deps: Option<Vec<String>>,
}

impl ProposalPatch {
    /// Nothing to change — an empty ask is refused at the op, not stored.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.why.is_none()
            && self.horizon.is_none()
            && self.area.is_none()
            && self.accept.is_none()
            && self.deps.is_none()
    }

    /// The names of the fields this patch touches, in the item's own order —
    /// what the op's response and the compact listing quote back to the PM.
    pub fn fields(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.title.is_some() {
            out.push("title");
        }
        if self.why.is_some() {
            out.push("why");
        }
        if self.horizon.is_some() {
            out.push("horizon");
        }
        if self.area.is_some() {
            out.push("area");
        }
        if self.accept.is_some() {
            out.push("accept");
        }
        if self.deps.is_some() {
            out.push("deps");
        }
        out
    }

    /// The item patch an accepted proposal applies. Only the shape fields are
    /// reachable by construction — the rest of [`ItemPatch`] stays default.
    pub fn to_item_patch(&self) -> ItemPatch {
        ItemPatch {
            title: self.title.clone(),
            why: self.why.clone(),
            horizon: self.horizon,
            area: self.area.clone(),
            accept: self.accept.clone(),
            deps: self.deps.clone(),
            ..Default::default()
        }
    }
}

/// One pending proposal, as the frontend sees it (`roadmap:proposal` and
/// `roadmap_list_proposals` carry the same shape). `patch` is the parsed
/// `patch_json` — a real object on the wire, `None` for a discard.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Proposal {
    pub id: String,
    pub item_id: String,
    /// Denormalized off the item so board-scoped listeners filter without a join.
    pub project_id: String,
    pub kind: ProposalKind,
    /// The validated [`ProposalPatch`] as stored; its keys are the changed fields.
    pub patch: Option<Value>,
    /// The PM's one-line rationale. Always present for a discard (the op
    /// requires a reason); optional for an update.
    pub note: Option<String>,
    pub created_at: i64,
}

const COLUMNS: &str = "id, item_id, project_id, kind, patch_json, note, created_at";

impl Proposal {
    /// The changed field names, in the item's own order, for quoting the ask
    /// back to the PM. Parsed from the stored patch rather than read off the
    /// raw JSON's keys — `serde_json` sorts map keys, and "deps, title" is not
    /// how anyone wrote it. Empty for a discard.
    pub fn fields(&self) -> Vec<&'static str> {
        self.patch
            .as_ref()
            .and_then(|v| serde_json::from_value::<ProposalPatch>(v.clone()).ok())
            .map(|p| p.fields())
            .unwrap_or_default()
    }

    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        let raw: Option<String> = r.get("patch_json")?;
        let patch = match raw.as_deref() {
            None | Some("") => None,
            Some(s) => Some(serde_json::from_str(s).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    format!("patch_json: {e}").into(),
                )
            })?),
        };
        Ok(Self {
            id: r.get("id")?,
            item_id: r.get("item_id")?,
            project_id: r.get("project_id")?,
            kind: enum_col(r, "kind", ProposalKind::from_db)?,
            patch,
            note: r.get("note")?,
            created_at: r.get("created_at")?,
        })
    }
}

/// Store the pending proposal for an item, replacing any it already has. Max
/// one per item is the table's `UNIQUE(item_id)`; the upsert keeps the existing
/// row's `id` so a replacement reaches the frontend as the same proposal with
/// new contents, not a second one. Returns the stored row for emitting after
/// the lock drops. Must be called with the connection lock held.
pub fn upsert(
    conn: &Connection,
    project_id: &str,
    item_id: &str,
    kind: ProposalKind,
    patch: Option<&ProposalPatch>,
    note: Option<&str>,
) -> rusqlite::Result<Proposal> {
    let patch_json = patch.and_then(|p| serde_json::to_string(p).ok());
    conn.execute(
        "INSERT INTO roadmap_proposals (id, item_id, project_id, kind, patch_json, note, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(item_id) DO UPDATE SET
           kind = excluded.kind,
           patch_json = excluded.patch_json,
           note = excluded.note,
           created_at = excluded.created_at",
        params![
            uuid::Uuid::new_v4().to_string(),
            item_id,
            project_id,
            kind.as_str(),
            patch_json,
            note,
            now_millis(),
        ],
    )?;
    // Read back by the unique key rather than the maybe-discarded new id.
    for_item(conn, item_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// One proposal by id, or `None` if it has already been ruled on. A
/// replacement keeps the id, so only a ruling (or the item's cascade) makes a
/// held handle go stale.
pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<Proposal>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM roadmap_proposals WHERE id = ?1"),
        [id],
        Proposal::from_row,
    )
    .optional()
}

/// The pending proposal for an item, if any — at most one by construction.
pub fn for_item(conn: &Connection, item_id: &str) -> rusqlite::Result<Option<Proposal>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM roadmap_proposals WHERE item_id = ?1"),
        [item_id],
        Proposal::from_row,
    )
    .optional()
}

/// Every pending proposal on a project's board, oldest ask first — the board
/// snapshot's companion to `store::list`.
pub fn list_for_project(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<Proposal>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM roadmap_proposals WHERE project_id = ?1 ORDER BY created_at, rowid"
    ))?;
    let rows = stmt.query_map([project_id], Proposal::from_row)?;
    rows.collect()
}

/// Remove a proposal — the ruling took it, or it went stale. Returns whether a
/// row was removed, so a caller doesn't announce a deletion that didn't happen.
pub fn delete(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute("DELETE FROM roadmap_proposals WHERE id = ?1", [id])?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::get_migrations;
    use crate::roadmap::store;
    use crate::roadmap::types::NewItem;

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

    fn item(conn: &Connection) -> crate::roadmap::types::RoadmapItem {
        store::create(
            conn,
            "p1",
            &NewItem {
                title: "one".into(),
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn retitle(to: &str) -> ProposalPatch {
        ProposalPatch {
            title: Some(to.into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_second_proposal_replaces_the_first_and_keeps_its_id() {
        let conn = test_conn();
        let it = item(&conn);

        let first = upsert(
            &conn,
            "p1",
            &it.id,
            ProposalKind::Update,
            Some(&retitle("v1")),
            None,
        )
        .unwrap();
        let second = upsert(
            &conn,
            "p1",
            &it.id,
            ProposalKind::Discard,
            None,
            Some("obsolete"),
        )
        .unwrap();

        // Same handle, new ask — the frontend's upsert-by-id swaps it in place.
        assert_eq!(second.id, first.id);
        assert_eq!(second.kind, ProposalKind::Discard);
        assert_eq!(second.patch, None);
        assert_eq!(second.note.as_deref(), Some("obsolete"));
        assert_eq!(list_for_project(&conn, "p1").unwrap(), vec![second]);
    }

    #[test]
    fn the_stored_patch_keys_are_exactly_the_changed_fields() {
        let conn = test_conn();
        let it = item(&conn);
        // An explicit `area: null` survives the round trip as a key that clears.
        let patch: ProposalPatch =
            serde_json::from_str(r#"{"title": "new", "area": null}"#).unwrap();
        assert_eq!(patch.fields(), vec!["title", "area"]);

        let stored = upsert(
            &conn,
            "p1",
            &it.id,
            ProposalKind::Update,
            Some(&patch),
            None,
        )
        .unwrap();
        let obj = stored.patch.as_ref().unwrap().as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert_eq!(obj["title"], "new");
        assert!(obj["area"].is_null());
        // And it parses back into the same clear-the-area patch.
        let back: ProposalPatch = serde_json::from_value(stored.patch.unwrap()).unwrap();
        assert_eq!(back.area, Some(None));
        assert_eq!(back.to_item_patch().area, Some(None));
    }

    #[test]
    fn deleting_an_item_takes_its_pending_proposal() {
        let conn = test_conn();
        let it = item(&conn);
        upsert(
            &conn,
            "p1",
            &it.id,
            ProposalKind::Update,
            Some(&retitle("gone with the row")),
            None,
        )
        .unwrap();
        assert!(store::delete(&conn, &it.id).unwrap());
        assert!(list_for_project(&conn, "p1").unwrap().is_empty());
    }
}
