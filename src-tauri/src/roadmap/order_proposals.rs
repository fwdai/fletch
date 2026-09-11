//! Pending whole-board reorder asks (project-scoped; not item `proposals`).
//!
//! Stored codes are the full orderable set; re-validate on accept — the board moves.


use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use super::types::{ItemStatus, RoadmapItem};
use crate::database::now_millis;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OrderProposal {
    pub project_id: String,
    pub codes: Vec<String>,
    pub note: Option<String>,
    pub created_at: i64,
}

const COLUMNS: &str = "project_id, codes_json, note, created_at";

impl OrderProposal {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        let raw: String = r.get("codes_json")?;
        let codes: Vec<String> = serde_json::from_str(&raw).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!("codes_json: {e}").into(),
            )
        })?;
        Ok(Self {
            project_id: r.get("project_id")?,
            codes,
            note: r.get("note")?,
            created_at: r.get("created_at")?,
        })
    }
}

pub fn is_orderable(item: &RoadmapItem) -> bool {
    matches!(
        item.status,
        ItemStatus::Proposed | ItemStatus::Open | ItemStatus::Queued
    )
}

pub fn orderable(items: &[RoadmapItem]) -> Vec<&RoadmapItem> {
    items.iter().filter(|i| is_orderable(i)).collect()
}

pub fn validate_order(codes: &[String], items: &[RoadmapItem]) -> Result<Vec<String>, String> {
    let open = orderable(items);
    let names = |list: &[&str]| list.join(", ");
    if codes.is_empty() {
        let all: Vec<&str> = open.iter().map(|i| i.code.as_str()).collect();
        return Err(format!(
            "`codes` must list every orderable item on this board, in the order you want them \
             built — that is {}",
            if all.is_empty() {
                "nothing: no item here is proposed, open, or queued".to_string()
            } else {
                names(&all)
            }
        ));
    }

    let by_code: HashMap<&str, &RoadmapItem> = items.iter().map(|i| (i.code.as_str(), i)).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut ids = Vec::with_capacity(codes.len());
    let mut unknown: Vec<&str> = Vec::new();
    let mut closed: Vec<String> = Vec::new();
    for code in codes {
        if !seen.insert(code.as_str()) {
            return Err(format!(
                "{code} appears twice — list every code exactly once, in the order you want"
            ));
        }
        match by_code.get(code.as_str()) {
            None => unknown.push(code.as_str()),
            Some(item) if !is_orderable(item) => {
                closed.push(format!("{} is {}", item.code, item.status.as_str()));
            }
            Some(item) => ids.push(item.id.clone()),
        }
    }
    if !unknown.is_empty() {
        return Err(format!(
            "{} is not an item on this board — order only the codes `roadmap_list` returns",
            names(&unknown)
        ));
    }
    if !closed.is_empty() {
        return Err(format!(
            "{} — dispatched, shipped, or rejected items have no place in the queue \
             order; list only the proposed, open, and queued ones",
            closed.join(", ")
        ));
    }
    let missing: Vec<&str> = open
        .iter()
        .filter(|i| !seen.contains(i.code.as_str()))
        .map(|i| i.code.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "the order must name every orderable item, and {} {} missing — send the whole \
             sequence, not the part you want moved",
            names(&missing),
            if missing.len() == 1 { "is" } else { "are" }
        ));
    }
    Ok(ids)
}

pub fn upsert(
    conn: &Connection,
    project_id: &str,
    codes: &[String],
    note: Option<&str>,
) -> rusqlite::Result<OrderProposal> {
    let codes_json = serde_json::to_string(codes)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "INSERT INTO roadmap_order_proposals (project_id, codes_json, note, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(project_id) DO UPDATE SET
           codes_json = excluded.codes_json,
           note = excluded.note,
           created_at = excluded.created_at",
        params![project_id, codes_json, note, now_millis()],
    )?;
    get(conn, project_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<OrderProposal>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM roadmap_order_proposals WHERE project_id = ?1"),
        [project_id],
        OrderProposal::from_row,
    )
    .optional()
}

pub fn delete(conn: &Connection, project_id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM roadmap_order_proposals WHERE project_id = ?1",
        [project_id],
    )?;
    Ok(n > 0)
}

#[cfg(test)]
#[path = "tests/order_proposals.rs"]
mod tests;
