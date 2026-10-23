//! The `review_state` table: one JSON document per coder checkout, so the
//! engine's table survives a restart. The engine owns the shape
//! (`engine::Tracked`); this module only stores and loads it.

use rusqlite::{params, Connection};

use super::engine::Tracked;
use super::{now_ms, Key};
use crate::error::Result;

/// `subdir` as the table spells it: `''` for the primary repo.
fn column(subdir: &Option<String>) -> &str {
    subdir.as_deref().unwrap_or("")
}

pub fn save(conn: &Connection, key: &Key, tracked: &Tracked) -> Result<()> {
    conn.execute(
        "INSERT INTO review_state (agent_id, subdir, state, updated_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(agent_id, subdir) DO UPDATE SET state = excluded.state, \
         updated_at = excluded.updated_at",
        params![
            key.0,
            column(&key.1),
            serde_json::to_string(tracked)?,
            now_ms()
        ],
    )?;
    Ok(())
}

/// Every stored checkout. A row whose JSON no longer parses is skipped with a
/// warning rather than failing the load: it is one checkout's memory, not the
/// host's.
pub fn load_all(conn: &Connection) -> Result<Vec<(Key, Tracked)>> {
    let mut stmt = conn.prepare("SELECT agent_id, subdir, state FROM review_state")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (agent_id, subdir, state) = row?;
        match serde_json::from_str::<Tracked>(&state) {
            Ok(tracked) => {
                let subdir = (!subdir.is_empty()).then_some(subdir);
                out.push(((agent_id, subdir), tracked));
            }
            Err(e) => tracing::warn!(error = %e, agent_id, "review state row unreadable; skipped"),
        }
    }
    Ok(out)
}

/// Every checkout of `agent_id`, with the workspace.
pub fn delete_for_agent(conn: &Connection, agent_id: &str) -> Result<()> {
    conn.execute("DELETE FROM review_state WHERE agent_id = ?1", [agent_id])?;
    Ok(())
}
