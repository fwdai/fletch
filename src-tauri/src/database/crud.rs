//! The generic table CRUD layer (`db_insert`/`select`/`update`/`delete`/
//! `count`/`upsert`/`query`) the IPC surface and storage modules build on.

use rusqlite::{params_from_iter, Connection};
use serde_json::{json, Map, Value};

use crate::error::{Error, Result};

use super::connection::now_millis;
use super::marshal::{append_where, json_to_sql, row_to_json};
use super::validate::{validate_column, validate_table};

// ── Public API ──────────────────────────────────────────────────────────────

pub fn db_insert(conn: &Connection, table: &str, mut data: Value) -> Result<String> {
    validate_table(table)?;
    let obj = data
        .as_object_mut()
        .ok_or_else(|| Error::Other("data must be a JSON object".into()))?;

    let id = match obj.get("id").and_then(|v| v.as_str()) {
        Some(existing) => existing.to_string(),
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            obj.insert("id".to_string(), json!(id));
            id
        }
    };

    let now = now_millis();
    obj.entry("created_at").or_insert(json!(now));

    let mut columns = Vec::new();
    let mut placeholders = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    for (i, (col, val)) in obj.iter().enumerate() {
        validate_column(col)?;
        columns.push(col.as_str());
        placeholders.push(format!("?{}", i + 1));
        params.push(json_to_sql(val)?);
    }

    let sql = format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        columns.join(", "),
        placeholders.join(", ")
    );

    conn.prepare(&sql)?.execute(params_from_iter(params))?;

    Ok(id)
}

pub fn db_select(conn: &Connection, table: &str, query: Value) -> Result<Vec<Map<String, Value>>> {
    validate_table(table)?;
    let q = query
        .as_object()
        .ok_or_else(|| Error::Other("query must be a JSON object".into()))?;

    let mut sql = format!("SELECT * FROM {table}");
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(w) = q.get("where").and_then(|v| v.as_object()) {
        if !w.is_empty() {
            append_where(w, &mut sql, &mut params)?;
        }
    }

    if let Some(col) = q.get("orderBy").and_then(|v| v.as_str()) {
        validate_column(col)?;
        let dir = match q.get("orderDirection").and_then(|v| v.as_str()) {
            Some(d) if d.eq_ignore_ascii_case("desc") => "DESC",
            _ => "ASC",
        };
        sql.push_str(&format!(" ORDER BY {col} {dir}"));
    }

    if let Some(n) = q.get("limit").and_then(|v| v.as_i64()) {
        if n > 0 {
            sql.push_str(&format!(" LIMIT {n}"));
        }
    }

    if let Some(n) = q.get("offset").and_then(|v| v.as_i64()) {
        if n > 0 {
            sql.push_str(&format!(" OFFSET {n}"));
        }
    }

    let mut stmt = conn.prepare(&sql)?;
    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    let rows: std::result::Result<Vec<_>, _> = stmt
        .query_map(params_from_iter(params), |row| row_to_json(row, &columns))?
        .collect();

    Ok(rows?)
}

pub fn db_update(conn: &Connection, table: &str, query: Value, data: Value) -> Result<usize> {
    validate_table(table)?;
    let q = query
        .as_object()
        .ok_or_else(|| Error::Other("query must be a JSON object".into()))?;
    let d = data
        .as_object()
        .ok_or_else(|| Error::Other("data must be a JSON object".into()))?;

    if d.is_empty() {
        return Err(Error::Other("cannot update with empty data".into()));
    }

    let mut set_clauses = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    for (col, val) in d {
        validate_column(col)?;
        let idx = params.len() + 1;
        set_clauses.push(format!("{col} = ?{idx}"));
        params.push(json_to_sql(val)?);
    }

    let mut sql = format!("UPDATE {table} SET {}", set_clauses.join(", "));

    if let Some(w) = q.get("where").and_then(|v| v.as_object()) {
        if !w.is_empty() {
            append_where(w, &mut sql, &mut params)?;
        }
    }

    let changed = conn.prepare(&sql)?.execute(params_from_iter(params))?;
    Ok(changed)
}

pub fn db_delete(conn: &Connection, table: &str, query: Value) -> Result<usize> {
    validate_table(table)?;
    let q = query
        .as_object()
        .ok_or_else(|| Error::Other("query must be a JSON object".into()))?;

    let mut sql = format!("DELETE FROM {table}");
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(w) = q.get("where").and_then(|v| v.as_object()) {
        if !w.is_empty() {
            append_where(w, &mut sql, &mut params)?;
        }
    }

    let changed = conn.prepare(&sql)?.execute(params_from_iter(params))?;
    Ok(changed)
}

pub fn db_count(conn: &Connection, table: &str, query: Value) -> Result<i64> {
    validate_table(table)?;
    let q = query
        .as_object()
        .ok_or_else(|| Error::Other("query must be a JSON object".into()))?;

    let mut sql = format!("SELECT COUNT(*) FROM {table}");
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(w) = q.get("where").and_then(|v| v.as_object()) {
        if !w.is_empty() {
            append_where(w, &mut sql, &mut params)?;
        }
    }

    let count: i64 = conn.query_row(&sql, params_from_iter(params), |row| row.get(0))?;
    Ok(count)
}

/// INSERT ... ON CONFLICT(conflict_col) DO UPDATE SET ...
/// `data` contains all columns for the insert. On conflict, every column
/// in `data` except the conflict column is updated.
pub fn db_upsert(
    conn: &Connection,
    table: &str,
    data: Value,
    conflict_column: &str,
) -> Result<String> {
    validate_table(table)?;
    let conflict_cols: Vec<&str> = conflict_column.split(',').map(|s| s.trim()).collect();
    if conflict_cols.is_empty() {
        return Err(Error::Other("conflict_column must not be empty".into()));
    }
    for col in &conflict_cols {
        validate_column(col)?;
    }
    let obj = data
        .as_object()
        .ok_or_else(|| Error::Other("data must be a JSON object".into()))?;

    if obj.is_empty() {
        return Err(Error::Other("cannot upsert with empty data".into()));
    }

    let mut columns = Vec::new();
    let mut placeholders = Vec::new();
    let mut update_clauses = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    for (i, (col, val)) in obj.iter().enumerate() {
        validate_column(col)?;
        columns.push(col.as_str());
        placeholders.push(format!("?{}", i + 1));
        params.push(json_to_sql(val)?);
        if !conflict_cols.contains(&col.as_str()) {
            update_clauses.push(format!("{col} = excluded.{col}"));
        }
    }

    let conflict_target = conflict_cols.join(", ");
    let sql = if update_clauses.is_empty() {
        format!(
            "INSERT INTO {table} ({}) VALUES ({}) ON CONFLICT({conflict_target}) DO NOTHING",
            columns.join(", "),
            placeholders.join(", "),
        )
    } else {
        format!(
            "INSERT INTO {table} ({}) VALUES ({}) ON CONFLICT({conflict_target}) DO UPDATE SET {}",
            columns.join(", "),
            placeholders.join(", "),
            update_clauses.join(", ")
        )
    };

    conn.prepare(&sql)?.execute(params_from_iter(params))?;

    let id = conflict_cols
        .first()
        .and_then(|c| obj.get(*c))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(id)
}

pub fn db_query(
    conn: &Connection,
    sql: &str,
    params: Vec<Value>,
) -> Result<Vec<Map<String, Value>>> {
    let sql_trimmed = sql.trim();
    if !sql_trimmed
        .get(..6)
        .map(|s| s.eq_ignore_ascii_case("select"))
        .unwrap_or(false)
    {
        return Err(Error::Other(
            "db_query only allows SELECT statements".into(),
        ));
    }

    let sql_params: Vec<Box<dyn rusqlite::ToSql>> =
        params.iter().map(json_to_sql).collect::<Result<_>>()?;

    let mut stmt = conn.prepare(sql_trimmed)?;
    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    let rows: std::result::Result<Vec<_>, _> = stmt
        .query_map(params_from_iter(sql_params), |row| {
            row_to_json(row, &columns)
        })?
        .collect();

    Ok(rows?)
}
