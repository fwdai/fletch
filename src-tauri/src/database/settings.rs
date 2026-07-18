//! Typed accessors over the key-value `settings` and `accounts` tables:
//! per-agent binary overrides, single-setting get/set, and the commit identity.

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::error::Result;

use super::crud::{db_select, db_upsert};

/// `settings` key prefix for per-agent custom binary path overrides. The agent
/// id follows the prefix (e.g. `agent_bin_path_claude`); the value is the raw
/// absolute path the user entered. Shared by the startup loader and the
/// `set_agent_bin_override` command so both agree on the key format.
pub const AGENT_BIN_PREFIX: &str = "agent_bin_path_";

/// Read every `agent_bin_path_*` setting into an id → path map. Called once at
/// startup to seed `bin_resolve`'s in-memory override registry so binary
/// resolution never needs a DB handle. Blank values are skipped (a cleared
/// override).
pub fn load_agent_bin_overrides(conn: &Connection) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let rows = match db_select(conn, "settings", json!({})) {
        Ok(rows) => rows,
        Err(_) => return map,
    };
    for row in rows {
        let key = row.get("key").and_then(Value::as_str).unwrap_or("");
        let value = row.get("value").and_then(Value::as_str).unwrap_or("");
        if let Some(id) = key.strip_prefix(AGENT_BIN_PREFIX) {
            if !value.trim().is_empty() {
                map.insert(id.to_string(), value.to_string());
            }
        }
    }
    map
}

/// Read a single value from the key-value `settings` table. Returns `None` for
/// a missing key (or any read error), so callers can fall back to a default.
pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    db_select(conn, "settings", json!({ "where": { "key": key } }))
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .and_then(|row| row.get("value").and_then(Value::as_str).map(str::to_string))
}

/// Upsert a single `settings` value. Mirrors the frontend `setSetting`, so a
/// value written here is readable by the renderer's `getAllSettings`.
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    db_upsert(
        conn,
        "settings",
        json!({ "key": key, "value": value }),
        "key",
    )?;
    Ok(())
}

/// The signed-in profile's display name and email from the single `accounts`
/// row — the fallback commit identity for repos with no `user.name` /
/// `user.email` configured (see `git_dist::fallback_identity`). Either field
/// may be absent; `None` when there is no account row at all.
pub fn get_account_identity(conn: &Connection) -> Option<(Option<String>, Option<String>)> {
    let row = db_select(conn, "accounts", json!({}))
        .ok()?
        .into_iter()
        .next()?;
    let field = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_string);
    Some((field("name"), field("email")))
}
