//! JSON ⇄ SQL marshalling: parameter binding, row decoding, blob hex, and the
//! shared WHERE-clause builder.

use serde_json::{json, Map, Value};

use crate::error::{Error, Result};

use super::validate::validate_column;

pub(crate) fn json_to_sql(value: &Value) -> Result<Box<dyn rusqlite::ToSql>> {
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

pub(crate) fn row_to_json(
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
            rusqlite::types::ValueRef::Blob(b) => Value::String(hex_encode(b)),
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

pub(crate) fn append_where(
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
