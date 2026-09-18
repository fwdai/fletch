//! SQL-injection guards: the table allow-list and the column-name validator.
//! Load-bearing — every dynamic identifier flows through these.

use crate::error::{Error, Result};

const ALLOWED_TABLES: &[&str] = &[
    "accounts",
    "custom_agents",
    "mcp_servers",
    "project_settings",
    "projects",
    "repos",
    "sessions",
    "settings",
    "skills",
    "usage_daily",
    "workspaces",
    "worktrees",
];

pub(crate) fn validate_table(table: &str) -> Result<()> {
    if ALLOWED_TABLES.contains(&table) {
        Ok(())
    } else {
        Err(Error::Other(format!("unknown table: {table}")))
    }
}

pub(crate) fn validate_column(col: &str) -> Result<()> {
    if !col.is_empty()
        && col.len() <= 64
        && col.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        Ok(())
    } else {
        Err(Error::Other(format!("invalid column name: {col}")))
    }
}
