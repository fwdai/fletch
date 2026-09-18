//! `roadmap_items` DAO. Call with the connection lock held.
//!
//! Hold/close columns stay off [`super::types::ItemPatch`] so generic edits
//! cannot stop the queue or reject without a ruling.

use rusqlite::{params, Connection, OptionalExtension};

use super::types::{strings_to_col, ItemPatch, ItemSource, ItemStatus, NewItem};
use super::types::{Horizon, RoadmapItem, COLUMNS};
use crate::database::now_millis;

const PREFIX_KEY: &str = "roadmap.code_prefix";

const SEQ_KEY: &str = "roadmap.code_seq";

const FIRST_NUMBER: i64 = 100;

const FALLBACK_PREFIX: &str = "PRJ";

pub fn list(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<RoadmapItem>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM roadmap_items WHERE project_id = ?1 \
         ORDER BY rank, created_at, rowid"
    ))?;
    let rows = stmt.query_map([project_id], RoadmapItem::from_row)?;
    rows.collect()
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<RoadmapItem>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM roadmap_items WHERE id = ?1"),
        [id],
        RoadmapItem::from_row,
    )
    .optional()
}

pub fn missing(id: &str) -> String {
    format!("roadmap item {id} no longer exists")
}

pub fn require(conn: &Connection, id: &str) -> Result<RoadmapItem, String> {
    get(conn, id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| missing(id))
}

pub fn create(conn: &Connection, project_id: &str, new: &NewItem) -> rusqlite::Result<RoadmapItem> {
    let id = uuid::Uuid::new_v4().to_string();
    let code = next_code(conn, project_id)?;
    let rank = next_rank(conn, project_id)?;
    let now = now_millis();
    conn.execute(
        "INSERT INTO roadmap_items
           (id, project_id, code, title, why, horizon, status, rank, area, source,
            accept_json, deps_json, workflow_def_id, issue_url, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
        params![
            id,
            project_id,
            code,
            new.title,
            new.why,
            new.horizon.unwrap_or(Horizon::Later).as_str(),
            new.status.unwrap_or(ItemStatus::Open).as_str(),
            rank,
            new.area,
            new.source.unwrap_or(ItemSource::User).as_str(),
            strings_to_col(&new.accept),
            strings_to_col(&new.deps),
            new.workflow_def_id,
            new.issue_url,
            now,
        ],
    )?;
    get(conn, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn update(
    conn: &Connection,
    id: &str,
    patch: &ItemPatch,
) -> rusqlite::Result<Option<RoadmapItem>> {
    apply(conn, id, patch, None)
}

pub fn update_where_status(
    conn: &Connection,
    id: &str,
    expected: ItemStatus,
    patch: &ItemPatch,
) -> rusqlite::Result<Option<RoadmapItem>> {
    apply(conn, id, patch, Some(expected))
}

fn apply(
    conn: &Connection,
    id: &str,
    patch: &ItemPatch,
    expected: Option<ItemStatus>,
) -> rusqlite::Result<Option<RoadmapItem>> {
    let mut sets: Vec<&str> = Vec::new();
    let mut vals: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut set = |col: &'static str, v: Box<dyn rusqlite::ToSql>| {
        sets.push(col);
        vals.push(v);
    };

    if let Some(v) = &patch.title {
        set("title", Box::new(v.clone()));
    }
    if let Some(v) = &patch.why {
        set("why", Box::new(v.clone()));
    }
    if let Some(v) = patch.horizon {
        set("horizon", Box::new(v.as_str()));
    }
    if let Some(v) = patch.status {
        set("status", Box::new(v.as_str()));
    }
    if let Some(v) = patch.source {
        set("source", Box::new(v.as_str()));
    }
    if let Some(v) = patch.rank {
        set("rank", Box::new(v));
    }
    if let Some(v) = &patch.accept {
        set("accept_json", Box::new(strings_to_col(v)));
    }
    if let Some(v) = &patch.deps {
        set("deps_json", Box::new(strings_to_col(v)));
    }
    if let Some(v) = &patch.area {
        set("area", Box::new(v.clone()));
    }
    if let Some(v) = &patch.agent_id {
        set("agent_id", Box::new(v.clone()));
    }
    if let Some(v) = &patch.workflow_def_id {
        set("workflow_def_id", Box::new(v.clone()));
    }
    if let Some(v) = &patch.run_id {
        set("run_id", Box::new(v.clone()));
    }
    if let Some(v) = &patch.pr_url {
        set("pr_url", Box::new(v.clone()));
    }
    if let Some(v) = patch.pr_number {
        set("pr_number", Box::new(v));
    }

    if sets.is_empty() {
        let row = get(conn, id)?;
        return Ok(match expected {
            Some(status) => row.filter(|r| r.status == status),
            None => row,
        });
    }

    let assignments: Vec<String> = sets
        .iter()
        .enumerate()
        .map(|(i, col)| format!("{col} = ?{}", i + 1))
        .collect();
    let n = vals.len();
    vals.push(Box::new(now_millis()));
    vals.push(Box::new(id.to_string()));
    let guard = match expected {
        Some(status) => {
            vals.push(Box::new(status.as_str()));
            format!(" AND status = ?{}", n + 3)
        }
        None => String::new(),
    };
    let sql = format!(
        "UPDATE roadmap_items SET {}, updated_at = ?{} WHERE id = ?{}{guard}",
        assignments.join(", "),
        n + 1,
        n + 2
    );
    let refs: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| v.as_ref()).collect();
    let changed = conn.execute(&sql, refs.as_slice())?;
    if expected.is_some() && changed == 0 {
        return Ok(None);
    }
    get(conn, id)
}

pub fn delete(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute("DELETE FROM roadmap_items WHERE id = ?1", [id])?;
    Ok(n > 0)
}

// Sets status+close_reason; not available via ItemPatch.
pub fn reject(conn: &Connection, id: &str, reason: &str) -> rusqlite::Result<Option<RoadmapItem>> {
    conn.execute(
        "UPDATE roadmap_items
            SET status = ?1, close_reason = ?2,
                hold_reason = NULL, held_by = NULL, held_at = NULL,
                agent_id = NULL, updated_at = ?3
          WHERE id = ?4",
        params![ItemStatus::Rejected.as_str(), reason, now_millis(), id],
    )?;
    get(conn, id)
}

pub fn reopen(conn: &Connection, id: &str) -> rusqlite::Result<Option<RoadmapItem>> {
    let changed = conn.execute(
        "UPDATE roadmap_items
            SET status = ?1, close_reason = NULL,
                hold_reason = NULL, held_by = NULL, held_at = NULL,
                updated_at = ?2
          WHERE id = ?3 AND status = ?4",
        params![
            ItemStatus::Open.as_str(),
            now_millis(),
            id,
            ItemStatus::Rejected.as_str()
        ],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    get(conn, id)
}

pub(crate) const DECLINED_ISSUES_KEY: &str = "roadmap.declined_issues";

const DECLINED_MAX: usize = 500;

pub fn decline_issue(conn: &Connection, project_id: &str, issue_url: &str) -> rusqlite::Result<()> {
    let mut urls = declined_issues(conn, project_id)?;
    if urls.iter().any(|u| u == issue_url) {
        return Ok(());
    }
    urls.push(issue_url.to_string());
    if urls.len() > DECLINED_MAX {
        urls.drain(..urls.len() - DECLINED_MAX);
    }
    let json = serde_json::to_string(&urls).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3) \
         ON CONFLICT(project_id, key) DO UPDATE SET value = excluded.value",
        params![project_id, DECLINED_ISSUES_KEY, json],
    )?;
    Ok(())
}

fn declined_issues(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<String>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
            params![project_id, DECLINED_ISSUES_KEY],
            |r| r.get(0),
        )
        .optional()?;
    Ok(stored
        .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
        .unwrap_or_default())
}

pub fn next_code(conn: &Connection, project_id: &str) -> rusqlite::Result<String> {
    let prefix = code_prefix(conn, project_id)?;
    let number = next_number(conn, project_id)?;
    conn.execute(
        "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3) \
         ON CONFLICT(project_id, key) DO UPDATE SET value = excluded.value",
        params![project_id, SEQ_KEY, (number + 1).to_string()],
    )?;
    Ok(format!("{prefix}-{number}"))
}

fn next_number(conn: &Connection, project_id: &str) -> rusqlite::Result<i64> {
    let stored: Option<i64> = conn
        .query_row(
            "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
            params![project_id, SEQ_KEY],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|v| v.trim().parse().ok());
    let mut stmt = conn.prepare("SELECT code FROM roadmap_items WHERE project_id = ?1")?;
    let highest = stmt
        .query_map([project_id], |r| r.get::<_, String>(0))?
        .filter_map(|c| c.ok())
        .filter_map(|c| code_number(&c))
        .max()
        .unwrap_or(FIRST_NUMBER - 1);
    Ok(stored.unwrap_or(FIRST_NUMBER).max(highest + 1))
}

pub fn next_rank(conn: &Connection, project_id: &str) -> rusqlite::Result<f64> {
    conn.query_row(
        "SELECT COALESCE(MAX(rank), 0.0) + 1.0 FROM roadmap_items WHERE project_id = ?1",
        [project_id],
        |r| r.get(0),
    )
}

pub fn set_ranks(conn: &Connection, ids: &[String]) -> rusqlite::Result<Vec<RoadmapItem>> {
    let tx = conn.unchecked_transaction()?;
    let now = now_millis();
    let mut out = Vec::with_capacity(ids.len());
    for (n, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE roadmap_items SET rank = ?1, updated_at = ?2 WHERE id = ?3",
            params![(n + 1) as f64, now, id],
        )?;
        if let Some(row) = get(&tx, id)? {
            out.push(row);
        }
    }
    tx.commit()?;
    Ok(out)
}

fn code_number(code: &str) -> Option<i64> {
    code.rsplit('-').next()?.parse().ok()
}

fn code_prefix(conn: &Connection, project_id: &str) -> rusqlite::Result<String> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
            params![project_id, PREFIX_KEY],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(p) = stored.filter(|p| !p.trim().is_empty()) {
        return Ok(p);
    }

    let name: Option<String> = conn
        .query_row(
            "SELECT name FROM projects WHERE id = ?1",
            [project_id],
            |r| r.get(0),
        )
        .optional()?;
    let prefix = derive_prefix(name.as_deref().unwrap_or_default());
    conn.execute(
        "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3) \
         ON CONFLICT(project_id, key) DO UPDATE SET value = excluded.value",
        params![project_id, PREFIX_KEY, prefix],
    )?;
    Ok(prefix)
}

fn derive_prefix(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let candidate: String = if words.len() >= 2 {
        words
            .iter()
            .take(4)
            .filter_map(|w| w.chars().next())
            .collect()
    } else {
        words
            .first()
            .map(|w| w.chars().take(3).collect())
            .unwrap_or_default()
    };
    let candidate = candidate.to_ascii_uppercase();
    if candidate.len() >= 2 {
        candidate
    } else {
        FALLBACK_PREFIX.to_string()
    }
}

#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;
