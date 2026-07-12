//! Definition storage commands (`wf_def_*`, spec §13): save / list / delete a
//! workflow definition and export/import it as YAML.
//!
//! A definition is a name + hue + a serialized [`Spec`] persisted in the
//! `wf_definition` table (spec §4). `run_count` and `created_at` are preserved
//! across edits (the upsert touches only the mutable columns), matching the v0
//! `workflow_save` semantics. Every save/import runs full §5.2 validation, so a
//! malformed definition can never reach the table (or, later, a launch).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use super::spec::{self, Spec};
use super::yaml::{self, ImportReport, LocalAgent};

type Db = Arc<Mutex<Connection>>;

/// Epoch milliseconds, matching the core schema's timestamp convention.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn new_id() -> String {
    format!("wf-{}", uuid::Uuid::new_v4())
}

/// A stored definition as returned to the frontend: the presentation columns
/// plus the parsed [`Spec`] (never the raw JSON string).
#[derive(Debug, Clone, Serialize)]
pub struct Definition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub hue: Option<i64>,
    pub spec: Spec,
    pub run_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

fn row_to_definition(r: &Row) -> rusqlite::Result<Definition> {
    let spec_json: String = r.get("spec_json")?;
    let spec: Spec = serde_json::from_str(&spec_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Definition {
        id: r.get("id")?,
        name: r.get("name")?,
        description: r.get("description")?,
        hue: r.get("hue")?,
        spec,
        run_count: r.get("run_count")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

/// Reject an invalid spec with all §5.2 messages joined into one error string.
fn validate_or_err(spec: &Spec) -> Result<(), String> {
    spec::validate(spec).map_err(|errs| errs.join("; "))
}

/// Persist a definition. Validates the spec first; generates an id when none is
/// supplied. On an existing id, only the mutable columns change — `run_count`
/// and `created_at` survive the edit.
#[tauri::command]
pub async fn wf_def_save(
    spec: Spec,
    id: Option<String>,
    hue: Option<i64>,
    db: tauri::State<'_, Db>,
) -> Result<Definition, String> {
    validate_or_err(&spec)?;
    let id = id.unwrap_or_else(new_id);
    let name = spec.name.clone();
    let description = spec.description.clone().unwrap_or_default();
    let spec_json = serde_json::to_string(&spec).map_err(|e| e.to_string())?;
    let now = now_ms();

    let conn = db.lock();
    conn.execute(
        "INSERT INTO wf_definition \
           (id, name, description, hue, spec_json, run_count, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?6) \
         ON CONFLICT(id) DO UPDATE SET \
           name = excluded.name, \
           description = excluded.description, \
           hue = excluded.hue, \
           spec_json = excluded.spec_json, \
           updated_at = excluded.updated_at",
        rusqlite::params![id, name, description, hue, spec_json, now],
    )
    .map_err(|e| e.to_string())?;

    conn.query_row(
        "SELECT id, name, description, hue, spec_json, run_count, created_at, updated_at \
         FROM wf_definition WHERE id = ?1",
        [&id],
        row_to_definition,
    )
    .map_err(|e| e.to_string())
}

/// Every definition, newest-edited first.
#[tauri::command]
pub async fn wf_def_list(db: tauri::State<'_, Db>) -> Result<Vec<Definition>, String> {
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, name, description, hue, spec_json, run_count, created_at, updated_at \
             FROM wf_definition ORDER BY updated_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], row_to_definition)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Delete a definition by id. In-flight runs are unaffected — they hold their
/// own launch-time spec snapshot (spec §4).
#[tauri::command]
pub async fn wf_def_delete(id: String, db: tauri::State<'_, Db>) -> Result<(), String> {
    let conn = db.lock();
    conn.execute("DELETE FROM wf_definition WHERE id = ?1", [&id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Export a definition as portable YAML (spec §5.3). Any alias backed by a local
/// custom agent has its base/model/instructions/skill *names* embedded and its
/// local id stripped, so the file runs on a machine without that custom agent.
#[tauri::command]
pub async fn wf_def_export_yaml(id: String, db: tauri::State<'_, Db>) -> Result<String, String> {
    let conn = db.lock();
    let spec_json: Option<String> = conn
        .query_row(
            "SELECT spec_json FROM wf_definition WHERE id = ?1",
            [&id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let spec_json = spec_json.ok_or_else(|| format!("no workflow definition '{id}'"))?;
    let mut spec: Spec = serde_json::from_str(&spec_json).map_err(|e| e.to_string())?;
    embed_custom_agents(&conn, &mut spec)?;
    yaml::to_yaml(&spec)
}

/// Import a YAML file (spec §13): parse, validate, then resolve agents/skills
/// against the local library. Missing skills and unknown providers are warnings
/// in the returned report, never errors — only malformed YAML or a §5.2
/// violation fails the import.
#[tauri::command]
pub async fn wf_def_import_yaml(
    yaml_text: String,
    db: tauri::State<'_, Db>,
) -> Result<ImportReport, String> {
    let spec = yaml::from_yaml(&yaml_text)?;
    validate_or_err(&spec)?;

    let conn = db.lock();
    let local_skills = list_skill_names(&conn)?;
    let local_agents = list_custom_agents(&conn)?;
    Ok(yaml::build_import_report(spec, &local_skills, &local_agents))
}

// ───────────────────────────── local library lookups ─────────────────────────

fn list_skill_names(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT name FROM skills")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

fn list_custom_agents(conn: &Connection) -> Result<Vec<LocalAgent>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name FROM custom_agents")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(LocalAgent {
                id: r.get(0)?,
                name: r.get(1)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

/// For each alias backed by a local custom agent, overwrite its spec with the
/// custom agent's base/model/instructions and its skills (resolved from ids to
/// names) so the export is self-contained, then clear the local id. A dangling
/// `custom_agent` id (deleted agent) leaves the alias's existing spec as-is.
fn embed_custom_agents(conn: &Connection, spec: &mut Spec) -> Result<(), String> {
    for agent in spec.agents.values_mut() {
        let Some(ca_id) = agent.custom_agent.clone() else {
            continue;
        };
        let resolved = conn
            .query_row(
                "SELECT base, model, instructions, skill_ids FROM custom_agents WHERE id = ?1",
                [&ca_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        agent.custom_agent = None;
        let Some((base, model, instructions, skill_ids_json)) = resolved else {
            continue; // dangling id: keep whatever the alias already carried
        };
        agent.base = base;
        agent.model = model.filter(|m| !m.is_empty());
        agent.instructions = Some(instructions).filter(|i| !i.is_empty());
        agent.skills = resolve_skill_names(conn, &skill_ids_json)?;
    }
    Ok(())
}

/// Map a JSON array of skill ids (a custom agent's `skill_ids`) to skill names,
/// dropping any id that no longer resolves.
fn resolve_skill_names(conn: &Connection, skill_ids_json: &str) -> Result<Vec<String>, String> {
    let ids: Vec<String> = serde_json::from_str(skill_ids_json).unwrap_or_default();
    let mut names = Vec::new();
    for id in ids {
        let name: Option<String> = conn
            .query_row("SELECT name FROM skills WHERE id = ?1", [&id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(name) = name {
            names.push(name);
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §4 `wf_definition` schema, created inline so command logic is
    /// testable before S1's migration lands (this mirrors the frozen contract).
    fn test_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE wf_definition (
               id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
               hue INTEGER, spec_json TEXT NOT NULL, run_count INTEGER NOT NULL DEFAULT 0,
               created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
             CREATE TABLE skills (id TEXT PRIMARY KEY, name TEXT NOT NULL,
               description TEXT NOT NULL DEFAULT '', body TEXT NOT NULL DEFAULT '',
               created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE custom_agents (id TEXT PRIMARY KEY, name TEXT NOT NULL,
               description TEXT NOT NULL DEFAULT '', color INTEGER NOT NULL DEFAULT 0,
               base TEXT NOT NULL, model TEXT, effort TEXT,
               instructions TEXT NOT NULL DEFAULT '',
               skill_ids TEXT NOT NULL DEFAULT '[]', mcp_server_ids TEXT NOT NULL DEFAULT '[]',
               created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        Arc::new(Mutex::new(conn))
    }

    fn sample_spec() -> Spec {
        yaml::from_yaml(
            "version: 1\nname: demo\nagents: { coder: { base: claude } }\n\
             workflow:\n  - step: build\n    agent: coder\n    goal: go\n",
        )
        .unwrap()
    }

    // These exercise the command bodies directly. `tauri::State` can't be
    // constructed in a unit test, so the SQL is factored the same way the
    // commands run it; here we replicate the persist/read path against the
    // in-memory contract table.
    fn save(db: &Db, spec: &Spec, id: Option<&str>) -> Definition {
        let id = id.map(str::to_string).unwrap_or_else(new_id);
        let spec_json = serde_json::to_string(spec).unwrap();
        let now = now_ms();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO wf_definition (id,name,description,hue,spec_json,run_count,created_at,updated_at) \
             VALUES (?1,?2,?3,?4,?5,0,?6,?6) \
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, description=excluded.description, \
               hue=excluded.hue, spec_json=excluded.spec_json, updated_at=excluded.updated_at",
            rusqlite::params![id, spec.name, spec.description.clone().unwrap_or_default(),
                Option::<i64>::None, spec_json, now],
        ).unwrap();
        conn.query_row(
            "SELECT id,name,description,hue,spec_json,run_count,created_at,updated_at \
             FROM wf_definition WHERE id=?1",
            [&id], row_to_definition,
        ).unwrap()
    }

    #[test]
    fn save_persists_and_round_trips_the_spec() {
        let db = test_db();
        let def = save(&db, &sample_spec(), Some("d1"));
        assert_eq!(def.id, "d1");
        assert_eq!(def.name, "demo");
        assert_eq!(def.run_count, 0);
        assert_eq!(def.spec, sample_spec());
    }

    #[test]
    fn resaving_preserves_run_count_and_created_at() {
        let db = test_db();
        let first = save(&db, &sample_spec(), Some("d1"));
        // Simulate a launch bumping run_count.
        db.lock()
            .execute("UPDATE wf_definition SET run_count=5 WHERE id='d1'", [])
            .unwrap();
        let mut edited = sample_spec();
        edited.name = "renamed".into();
        let second = save(&db, &edited, Some("d1"));
        assert_eq!(second.name, "renamed");
        assert_eq!(second.run_count, 5, "run_count survives an edit");
        assert_eq!(second.created_at, first.created_at, "created_at survives");
    }

    #[test]
    fn export_embeds_custom_agent_and_strips_local_id() {
        let db = test_db();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO skills (id,name) VALUES ('sk-1','code-review')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO custom_agents (id,name,base,model,instructions,skill_ids) \
                 VALUES ('ca-1','Reviewer','claude','opus','Be strict.','[\"sk-1\"]')",
                [],
            )
            .unwrap();
        }
        let mut spec = sample_spec();
        spec.agents.get_mut("coder").unwrap().custom_agent = Some("ca-1".into());

        let mut to_export = spec.clone();
        embed_custom_agents(&db.lock(), &mut to_export).unwrap();
        let agent = &to_export.agents["coder"];
        assert_eq!(agent.base, "claude");
        assert_eq!(agent.model.as_deref(), Some("opus"));
        assert_eq!(agent.instructions.as_deref(), Some("Be strict."));
        assert_eq!(agent.skills, vec!["code-review".to_string()]);
        assert!(agent.custom_agent.is_none());

        let out = yaml::to_yaml(&to_export).unwrap();
        assert!(!out.contains("ca-1"));
        assert!(out.contains("code-review"));
    }

    #[test]
    fn dangling_custom_agent_id_is_dropped_gracefully() {
        let db = test_db();
        let mut spec = sample_spec();
        spec.agents.get_mut("coder").unwrap().custom_agent = Some("gone".into());
        embed_custom_agents(&db.lock(), &mut spec).unwrap();
        // Local id cleared; the alias keeps its original base.
        assert!(spec.agents["coder"].custom_agent.is_none());
        assert_eq!(spec.agents["coder"].base, "claude");
    }
}
