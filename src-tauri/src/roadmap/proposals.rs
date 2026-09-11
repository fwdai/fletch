//! Pending PM item deltas (`roadmap_proposals`). PM suggests; user rules.
//!
//! One pending ask per item; newer replaces (same id). Shape only — never lifecycle.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{double_option, enum_col, Horizon, ItemPatch};
use crate::database::now_millis;

crate::db_enum! {
    ProposalKind {
        Update  => "update",
        Discard => "discard",
    }
}

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
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.why.is_none()
            && self.horizon.is_none()
            && self.area.is_none()
            && self.accept.is_none()
            && self.deps.is_none()
    }

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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Proposal {
    pub id: String,
    pub item_id: String,
    pub project_id: String,
    pub kind: ProposalKind,
    pub patch: Option<Value>,
    pub note: Option<String>,
    pub created_at: i64,
}

const COLUMNS: &str = "id, item_id, project_id, kind, patch_json, note, created_at";

impl Proposal {
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
    for_item(conn, item_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<Proposal>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM roadmap_proposals WHERE id = ?1"),
        [id],
        Proposal::from_row,
    )
    .optional()
}

pub fn for_item(conn: &Connection, item_id: &str) -> rusqlite::Result<Option<Proposal>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM roadmap_proposals WHERE item_id = ?1"),
        [item_id],
        Proposal::from_row,
    )
    .optional()
}

pub fn list_for_project(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<Proposal>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM roadmap_proposals WHERE project_id = ?1 ORDER BY created_at, rowid"
    ))?;
    let rows = stmt.query_map([project_id], Proposal::from_row)?;
    rows.collect()
}

pub fn delete(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute("DELETE FROM roadmap_proposals WHERE id = ?1", [id])?;
    Ok(n > 0)
}

#[cfg(test)]
#[path = "tests/proposals.rs"]
mod tests;
