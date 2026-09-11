//! Product brief memory: load / propose / user-accept (invariant 2 on writes).
//!
//! PM may propose only; [`accept`] / [`save`] are the user-gated writers.
//! Behind this seam is replaceable; the three surfaces above are the contract.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use super::store;
use super::types::{ItemStatus, RoadmapItem};
use crate::database::now_millis;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Brief {
    pub project_id: String,
    pub content: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BriefProposal {
    pub project_id: String,
    pub content: String,
    pub note: Option<String>,
    pub created_at: i64,
}

/// Cap for brief content (characters the human will read).
pub const MAX_CONTENT: usize = 32 * 1024;

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

pub fn load(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<Brief>> {
    conn.query_row(
        &format!("SELECT {BRIEF_COLUMNS} FROM roadmap_briefs WHERE project_id = ?1"),
        [project_id],
        Brief::from_row,
    )
    .optional()
}

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

pub fn delete_proposal(conn: &Connection, project_id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM roadmap_brief_proposals WHERE project_id = ?1",
        [project_id],
    )?;
    Ok(n > 0)
}

pub fn accept(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<Brief>> {
    let Some(proposal) = get_proposal(conn, project_id)? else {
        return Ok(None);
    };
    let brief = save(conn, project_id, &proposal.content)?;
    delete_proposal(conn, project_id)?;
    Ok(Some(brief))
}

pub const NOT_DOING_MAX: usize = 30;

pub fn not_doing(items: &[RoadmapItem]) -> (Vec<&RoadmapItem>, usize) {
    let mut rejected: Vec<&RoadmapItem> = items
        .iter()
        .filter(|i| i.status == ItemStatus::Rejected)
        .collect();
    rejected.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
    let dropped = rejected.len().saturating_sub(NOT_DOING_MAX);
    rejected.truncate(NOT_DOING_MAX);
    (rejected, dropped)
}

pub fn product_context(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<String>> {
    let mut sections: Vec<String> = Vec::new();
    if let Some(brief) = load(conn, project_id)? {
        sections.push(format!("## Product brief\n\n{}", brief.content));
    }
    let items = store::list(conn, project_id)?;
    let (rejected, dropped) = not_doing(&items);
    if !rejected.is_empty() {
        let mut lines: Vec<String> = rejected.iter().map(|i| not_doing_line(i)).collect();
        if dropped > 0 {
            lines.push(format!(
                "…and {dropped} older rejected item(s) not shown — the {NOT_DOING_MAX} newest are"
            ));
        }
        sections.push(format!("## Not doing\n\n{}", lines.join("\n")));
    }
    Ok((!sections.is_empty()).then(|| sections.join("\n\n")))
}

fn not_doing_line(item: &RoadmapItem) -> String {
    match item.close_reason.as_deref() {
        Some(reason) => format!(
            "- {} — {} — {}",
            item.code,
            one_line(&item.title),
            one_line(reason)
        ),
        None => format!("- {} — {}", item.code, one_line(&item.title)),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
#[path = "tests/memory.rs"]
mod tests;
