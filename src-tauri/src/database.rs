use parking_lot::Mutex;
use rusqlite::{params_from_iter, Connection};
use rusqlite_migration::{Migrations, M};
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::Arc;

use crate::error::{Error, Result};

const ALLOWED_TABLES: &[&str] = &["agents", "agent_repos", "messages", "workspace_repos"];

fn validate_table(table: &str) -> Result<()> {
    if ALLOWED_TABLES.contains(&table) {
        Ok(())
    } else {
        Err(Error::Other(format!("unknown table: {table}")))
    }
}

fn validate_column(col: &str) -> Result<()> {
    if !col.is_empty() && col.len() <= 64 && col.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        Ok(())
    } else {
        Err(Error::Other(format!("invalid column name: {col}")))
    }
}

fn get_migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("../migrations/0001_initial_schema.sql")),
    ])
}

pub fn init(data_dir: &Path) -> Result<Arc<Mutex<Connection>>> {
    std::fs::create_dir_all(data_dir)?;
    let db_path = data_dir.join("quorum.db");
    let mut conn = Connection::open(&db_path)?;

    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = MEMORY;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;

    get_migrations()
        .to_latest(&mut conn)
        .map_err(|e| Error::Other(format!("migration failed: {e}")))?;

    Ok(Arc::new(Mutex::new(conn)))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn json_to_sql(value: &Value) -> Result<Box<dyn rusqlite::ToSql>> {
    match value {
        Value::Null => Ok(Box::new(rusqlite::types::Null)),
        Value::Bool(b) => Ok(Box::new(if *b { 1i64 } else { 0i64 })),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Box::new(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Box::new(f))
            } else {
                Err(Error::Other("unsupported number type".into()))
            }
        }
        Value::String(s) => Ok(Box::new(s.clone())),
        Value::Array(_) | Value::Object(_) => Ok(Box::new(serde_json::to_string(value)?)),
    }
}

fn row_to_json(
    row: &rusqlite::Row,
    columns: &[String],
) -> std::result::Result<Map<String, Value>, rusqlite::Error> {
    let mut map = Map::new();
    for (i, col) in columns.iter().enumerate() {
        let val = match row.get_ref(i)? {
            rusqlite::types::ValueRef::Null => Value::Null,
            rusqlite::types::ValueRef::Integer(n) => json!(n),
            rusqlite::types::ValueRef::Real(f) => json!(f),
            rusqlite::types::ValueRef::Text(s) => {
                Value::String(String::from_utf8_lossy(s).into_owned())
            }
            rusqlite::types::ValueRef::Blob(b) => {
                Value::String(hex_encode(b))
            }
        };
        map.insert(col.clone(), val);
    }
    Ok(map)
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn append_where(
    where_obj: &Map<String, Value>,
    sql: &mut String,
    params: &mut Vec<Box<dyn rusqlite::ToSql>>,
) -> Result<()> {
    let mut clauses = Vec::new();
    for (col, val) in where_obj {
        validate_column(col)?;
        if val.is_null() {
            clauses.push(format!("{col} IS NULL"));
        } else {
            let idx = params.len() + 1;
            clauses.push(format!("{col} = ?{idx}"));
            params.push(json_to_sql(val)?);
        }
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    Ok(())
}

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

    conn.prepare(&sql)?
        .execute(params_from_iter(params))?;

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
        return Err(Error::Other("db_query only allows SELECT statements".into()));
    }

    let sql_params: Vec<Box<dyn rusqlite::ToSql>> = params
        .iter()
        .map(json_to_sql)
        .collect::<Result<_>>()?;

    let mut stmt = conn.prepare(sql_trimmed)?;
    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    let rows: std::result::Result<Vec<_>, _> = stmt
        .query_map(params_from_iter(sql_params), |row| {
            row_to_json(row, &columns)
        })?
        .collect();

    Ok(rows?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Arc<Mutex<Connection>> {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path()).unwrap()
    }

    #[test]
    fn insert_and_select() {
        let db = test_db();
        let conn = db.lock();

        let id = db_insert(
            &conn,
            "agents",
            json!({ "name": "test-agent", "provider": "claude" }),
        )
        .unwrap();

        let rows = db_select(&conn, "agents", json!({ "where": { "id": id } })).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "test-agent");
        assert_eq!(rows[0]["provider"], "claude");
        assert_eq!(rows[0]["status"], "spawning");
    }

    #[test]
    fn update_and_delete() {
        let db = test_db();
        let conn = db.lock();

        let id = db_insert(
            &conn,
            "agents",
            json!({ "name": "a", "provider": "claude" }),
        )
        .unwrap();

        let changed = db_update(
            &conn,
            "agents",
            json!({ "where": { "id": id } }),
            json!({ "status": "running" }),
        )
        .unwrap();
        assert_eq!(changed, 1);

        let rows = db_select(&conn, "agents", json!({ "where": { "id": id } })).unwrap();
        assert_eq!(rows[0]["status"], "running");

        let deleted = db_delete(&conn, "agents", json!({ "where": { "id": id } })).unwrap();
        assert_eq!(deleted, 1);

        let count = db_count(&conn, "agents", json!({})).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn rejects_unknown_table() {
        let db = test_db();
        let conn = db.lock();
        assert!(db_select(&conn, "evil_table", json!({})).is_err());
    }

    #[test]
    fn rejects_invalid_column() {
        let db = test_db();
        let conn = db.lock();
        assert!(
            db_select(&conn, "agents", json!({ "where": { "id; DROP TABLE agents": "x" } }))
                .is_err()
        );
    }

    #[test]
    fn fts_search() {
        let db = test_db();
        let conn = db.lock();

        let agent_id = db_insert(
            &conn,
            "agents",
            json!({ "name": "search-test", "provider": "claude" }),
        )
        .unwrap();

        db_insert(
            &conn,
            "messages",
            json!({
                "agent_id": agent_id,
                "kind": "agent_message",
                "content": "I decided to use polling instead of websockets because the server doesn't support persistent connections",
                "sequence": 1
            }),
        )
        .unwrap();

        db_insert(
            &conn,
            "messages",
            json!({
                "agent_id": agent_id,
                "kind": "user_message",
                "content": "Why did you choose that approach?",
                "sequence": 2
            }),
        )
        .unwrap();

        let results = db_query(
            &conn,
            "SELECT m.id, m.agent_id, m.kind, m.content, m.sequence \
             FROM messages m \
             JOIN messages_fts ON messages_fts.rowid = m.rowid \
             WHERE messages_fts MATCH ?1 \
             ORDER BY rank",
            vec![json!("polling websockets")],
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0]["content"]
            .as_str()
            .unwrap()
            .contains("polling"));
    }

    #[test]
    fn db_query_rejects_non_select() {
        let db = test_db();
        let conn = db.lock();
        assert!(db_query(&conn, "DELETE FROM agents", vec![]).is_err());
        assert!(db_query(&conn, "DROP TABLE agents", vec![]).is_err());
    }

    #[test]
    fn auto_generates_uuid_and_timestamp() {
        let db = test_db();
        let conn = db.lock();

        let id = db_insert(
            &conn,
            "agents",
            json!({ "name": "auto", "provider": "claude" }),
        )
        .unwrap();

        assert!(!id.is_empty());
        assert!(uuid::Uuid::parse_str(&id).is_ok());

        let rows = db_select(&conn, "agents", json!({ "where": { "id": id } })).unwrap();
        let created = rows[0]["created_at"].as_i64().unwrap();
        assert!(created > 0);
    }

    #[test]
    fn cascade_deletes_messages_with_agent() {
        let db = test_db();
        let conn = db.lock();

        let agent_id = db_insert(
            &conn,
            "agents",
            json!({ "name": "cascade", "provider": "claude" }),
        )
        .unwrap();

        db_insert(
            &conn,
            "messages",
            json!({ "agent_id": agent_id, "kind": "user_message", "content": "hello", "sequence": 1 }),
        )
        .unwrap();

        db_delete(&conn, "agents", json!({ "where": { "id": agent_id } })).unwrap();

        let count = db_count(
            &conn,
            "messages",
            json!({ "where": { "agent_id": agent_id } }),
        )
        .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn null_where_clause() {
        let db = test_db();
        let conn = db.lock();

        db_insert(
            &conn,
            "agents",
            json!({ "name": "with-error", "provider": "claude", "last_error": "boom" }),
        )
        .unwrap();

        db_insert(
            &conn,
            "agents",
            json!({ "name": "no-error", "provider": "claude" }),
        )
        .unwrap();

        let rows = db_select(
            &conn,
            "agents",
            json!({ "where": { "last_error": null } }),
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "no-error");
    }
}
