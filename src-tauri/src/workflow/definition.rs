//! Definition storage commands (`wf_def_*`, spec §13): save / list / delete a
//! workflow definition and export/import it as YAML.
//!
//! A definition is a name + hue + a serialized [`Spec`] persisted in the
//! `wf_definition` table (spec §4). `run_count` and `created_at` are preserved
//! across edits (the upsert touches only the mutable columns), matching the v0
//! `workflow_save` semantics. Every save/import runs full §5.2 validation, so a
//! malformed definition can never reach the table (or, later, a launch).

use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use super::now_ms;
use super::spec::{self, Spec};
use super::yaml::{self, ImportReport, LocalAgent};

type Db = Arc<Mutex<Connection>>;

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
    let local_mcp_servers = list_mcp_server_names(&conn)?;
    let local_agents = list_custom_agents(&conn)?;
    Ok(yaml::build_import_report(
        spec,
        &local_skills,
        &local_mcp_servers,
        &local_agents,
    ))
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

fn list_mcp_server_names(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT name FROM mcp_servers")
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
                "SELECT base, model, effort, instructions, skill_ids, mcp_server_ids \
                 FROM custom_agents WHERE id = ?1",
                [&ca_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        agent.custom_agent = None;
        let Some((base, model, effort, instructions, skill_ids_json, mcp_ids_json)) = resolved
        else {
            continue; // dangling id: keep whatever the alias already carried
        };
        agent.base = base;
        agent.model = model.filter(|m| !m.is_empty());
        agent.effort = effort.filter(|e| !e.is_empty());
        agent.instructions = Some(instructions).filter(|i| !i.is_empty());
        agent.skills = resolve_skill_names(conn, &skill_ids_json)?;
        agent.mcp_servers = resolve_mcp_defs(conn, &mcp_ids_json)?;
    }
    Ok(())
}

/// Map a JSON array of MCP server ids (a custom agent's `mcp_server_ids`) to the
/// portable [`McpServerDef`]s embedded in an export — env/header KEY NAMES only,
/// never their secret values (spec §5.3). Ids that no longer resolve are
/// dropped silently: this is the *export* side, so the only reader is the file,
/// and a deleted server simply doesn't travel.
fn resolve_mcp_defs(
    conn: &Connection,
    mcp_ids_json: &str,
) -> Result<Vec<spec::McpServerDef>, String> {
    let ids: Vec<String> = serde_json::from_str(mcp_ids_json).unwrap_or_default();
    let mut defs = Vec::new();
    for id in ids {
        let row = conn
            .query_row(
                "SELECT name, transport, command, env, url, headers FROM mcp_servers WHERE id = ?1",
                [&id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((name, transport, command, env, url, headers)) = row {
            defs.push(spec::McpServerDef {
                name,
                transport,
                command: command.trim().to_string(),
                url: url.trim().to_string(),
                env_keys: pair_keys(&env, '='),
                header_keys: pair_keys(&headers, ':'),
            });
        }
    }
    Ok(defs)
}

/// The KEY names from `KEY=VALUE` / `Name: value` lines, values discarded. The
/// export carries only the shape of an MCP server's secrets, never the secrets
/// (spec §5.3).
fn pair_keys(text: &str, sep: char) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let (k, _) = line.trim().split_once(sep)?;
            let k = k.trim();
            (!k.is_empty()).then(|| k.to_string())
        })
        .collect()
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

// ───────────────────────── spawn deliverables (§3.2) ─────────────────────────

/// Which MCP transports a provider can deliver at spawn — mirrors the app's
/// `MCP_SUPPORT` map in `src/data/providers.ts`; the two must not drift.
fn mcp_support(provider: &str) -> &'static str {
    match provider {
        "claude" => "all",
        "codex" => "stdio",
        _ => "none",
    }
}

fn mcp_attachable(support: &str, transport: &str) -> bool {
    support == "all" || (support == "stdio" && transport != "http")
}

/// What [`resolve_step_deliverables`] found for a step at spawn: the by-value
/// snapshots to deliver, plus everything the definition requested that no
/// longer resolves — the caller journals those as warnings (warn-don't-fail,
/// the same policy as import).
pub(super) struct StepDeliverables {
    pub skills: Vec<crate::agent_profile::SkillSnapshot>,
    pub mcp_servers: Vec<crate::agent_profile::McpServerSnapshot>,
    /// The linked custom agent's reasoning effort, when one resolves. The
    /// scheduler applies it only as a fallback — an explicit `AgentSpec.effort`
    /// override wins (§3.2). `None` when there's no custom agent, its row is
    /// gone, or its effort column is null.
    pub effort: Option<String>,
    /// Skill names/ids the definition requested that no longer resolve
    /// (deleted since the save) — or a descriptive entry when the custom
    /// agent's `skill_ids` column itself is unreadable.
    pub missing_skills: Vec<String>,
    /// MCP server ids the custom agent assigned that no longer resolve
    /// (deleted since the save) — or a descriptive entry when the agent's
    /// `mcp_server_ids` column itself is unreadable. Provider-filtered
    /// transports are *not* listed — that gating is by design and mirrors
    /// the agent editor.
    pub missing_mcp_servers: Vec<String>,
    /// The step's `custom_agent` id when its row no longer resolves — the step
    /// spawns without that agent's skills and MCP servers.
    pub missing_custom_agent: Option<String>,
}

/// Resolve a step's skill/MCP deliverables at spawn (§3.2) — the Rust twin of
/// the app's `snapshotAgentDeliverables`: the custom agent's assigned skills
/// and servers by id, plus the spec's `skills` names resolved against the
/// library, filtered to what the provider can deliver. Snapshots are by value
/// (the same semantics as the draft spawn path), so later library edits never
/// touch a spawned step. Anything that no longer resolves — a skill, or the
/// custom agent itself — is reported on [`StepDeliverables`] so the caller can
/// journal a warning; the step still spawns.
pub(super) fn resolve_step_deliverables(
    conn: &Connection,
    custom_agent_id: Option<&str>,
    skill_names: &[String],
    mcp_server_names: &[String],
    provider: &str,
) -> StepDeliverables {
    let mut skills: Vec<crate::agent_profile::SkillSnapshot> = Vec::new();
    let mut mcp: Vec<crate::agent_profile::McpServerSnapshot> = Vec::new();
    let mut missing_skills: Vec<String> = Vec::new();
    let mut missing_mcp_servers: Vec<String> = Vec::new();
    let mut missing_custom_agent: Option<String> = None;
    let mut effort: Option<String> = None;

    let assigned: (Vec<String>, Vec<String>) = match custom_agent_id {
        None => Default::default(),
        Some(id) => {
            let row = conn
                .query_row(
                    "SELECT skill_ids, mcp_server_ids, effort FROM custom_agents WHERE id = ?1",
                    [id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .optional()
                .ok()
                .flatten();
            match row {
                // A malformed assignment column must not collapse to "nothing
                // requested" — that would skip every missing-deliverable check
                // below and lose the agent's capabilities with no warning. The
                // parse failure lands in the respective missing list as a
                // descriptive entry, so the usual event fires and the timeline
                // says exactly what was unreadable.
                Some((s, m, e)) => {
                    effort = e.filter(|v| !v.is_empty());
                    let skill_ids = match serde_json::from_str(&s) {
                        Ok(v) => v,
                        Err(e) => {
                            missing_skills
                                .push(format!("custom agent '{id}' skill_ids unreadable: {e}"));
                            Vec::new()
                        }
                    };
                    let server_ids = match serde_json::from_str(&m) {
                        Ok(v) => v,
                        Err(e) => {
                            missing_mcp_servers.push(format!(
                                "custom agent '{id}' mcp_server_ids unreadable: {e}"
                            ));
                            Vec::new()
                        }
                    };
                    (skill_ids, server_ids)
                }
                None => {
                    missing_custom_agent = Some(id.to_string());
                    Default::default()
                }
            }
        }
    };

    // Custom-agent skills by id (assignment order), then spec names on top,
    // deduped by name.
    for skill_id in &assigned.0 {
        match skill_row(conn, "id", skill_id) {
            Ok(Some(s)) => {
                if !skills.iter().any(|k| k.name == s.name) {
                    skills.push(s);
                }
            }
            _ => missing_skills.push(skill_id.clone()),
        }
    }
    for name in skill_names {
        match skill_row(conn, "name", name) {
            Ok(Some(s)) => {
                if !skills.iter().any(|k| k.name == s.name) {
                    skills.push(s);
                }
            }
            _ => missing_skills.push(name.clone()),
        }
    }

    let support = mcp_support(provider);
    for server_id in &assigned.1 {
        match mcp_snapshot_row(conn, "id", server_id) {
            Ok(Some(snap)) => {
                if mcp_attachable(support, &snap.transport) && !mcp.iter().any(|m| m.name == snap.name)
                {
                    mcp.push(snap);
                }
            }
            // A dangling id (server deleted since the agent was saved) is a
            // real capability loss — report it; a transport the provider can't
            // run is filtered above by design and stays silent.
            _ => missing_mcp_servers.push(server_id.clone()),
        }
    }
    // Spec-level MCP servers (from an imported workflow's embedded defs) resolve
    // by *name* against the local library — the same warn-don't-fail path skills
    // take. A locally-built definition leaves `mcp_server_names` empty and gets
    // its MCP via the custom-agent ids above; the name dedup keeps the two from
    // double-attaching the same server.
    for name in mcp_server_names {
        match mcp_snapshot_row(conn, "name", name) {
            Ok(Some(snap)) => {
                if mcp_attachable(support, &snap.transport) && !mcp.iter().any(|m| m.name == snap.name)
                {
                    mcp.push(snap);
                }
            }
            _ => missing_mcp_servers.push(name.clone()),
        }
    }
    StepDeliverables {
        skills,
        mcp_servers: mcp,
        missing_skills,
        missing_mcp_servers,
        missing_custom_agent,
        effort,
    }
}

fn skill_row(
    conn: &Connection,
    column: &str,
    value: &str,
) -> Result<Option<crate::agent_profile::SkillSnapshot>, rusqlite::Error> {
    // `column` is a compile-time literal ("id" | "name"), never user input.
    let sql = format!("SELECT name, description, body FROM skills WHERE {column} = ?1 LIMIT 1");
    conn.query_row(&sql, [value], |r| {
        Ok(crate::agent_profile::SkillSnapshot {
            name: r.get(0)?,
            description: r.get(1)?,
            body: r.get(2)?,
        })
    })
    .optional()
}

/// Resolve a registry row into the by-value spawn snapshot — the Rust twin of
/// `snapshotMcpServer` in `src/storage/mcpServers.ts`: the command line is
/// whitespace-split into command + args, env/header lines are parsed into
/// pairs (KEY=VALUE / "Name: value", blanks and malformed lines skipped).
fn mcp_snapshot_row(
    conn: &Connection,
    column: &str,
    value: &str,
) -> Result<Option<crate::agent_profile::McpServerSnapshot>, rusqlite::Error> {
    // `column` is a compile-time literal ("id" | "name"), never user input.
    let sql = format!(
        "SELECT name, transport, command, env, url, headers FROM mcp_servers WHERE {column} = ?1 LIMIT 1"
    );
    conn.query_row(
        &sql,
        [value],
        |r| {
            let command_line: String = r.get(2)?;
            let env_text: String = r.get(3)?;
            let headers_text: String = r.get(5)?;
            let mut tokens = command_line.split_whitespace().map(str::to_string);
            Ok(crate::agent_profile::McpServerSnapshot {
                name: r.get(0)?,
                transport: r.get(1)?,
                command: tokens.next().unwrap_or_default(),
                args: tokens.collect(),
                env: parse_pair_lines(&env_text, '='),
                url: r.get::<_, String>(4)?.trim().to_string(),
                headers: parse_pair_lines(&headers_text, ':'),
            })
        },
    )
    .optional()
}

fn parse_pair_lines(text: &str, sep: char) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (k, v) = line.split_once(sep)?;
            let k = k.trim();
            if k.is_empty() {
                return None;
            }
            Some((k.to_string(), v.trim().to_string()))
        })
        .collect()
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
             CREATE TABLE mcp_servers (id TEXT PRIMARY KEY, name TEXT NOT NULL,
               transport TEXT NOT NULL DEFAULT 'stdio', command TEXT NOT NULL DEFAULT '',
               env TEXT NOT NULL DEFAULT '', url TEXT NOT NULL DEFAULT '',
               headers TEXT NOT NULL DEFAULT '',
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
            [&id],
            row_to_definition,
        )
        .unwrap()
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
    fn export_embeds_mcp_defs_key_only_and_effort() {
        let db = test_db();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mcp_servers (id,name,transport,command,env,url,headers) \
                 VALUES ('m1','gh','stdio','npx -y gh-mcp', \
                         'TOKEN=supersecret' || char(10) || 'REGION=us', '', ''), \
                        ('m2','web','http','','',' https://mcp.example ','X-Key: topsecret')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO custom_agents (id,name,base,effort,mcp_server_ids) \
                 VALUES ('ca-1','Reviewer','claude','high','[\"m1\",\"m2\"]')",
                [],
            )
            .unwrap();
        }
        let mut spec = sample_spec();
        spec.agents.get_mut("coder").unwrap().custom_agent = Some("ca-1".into());
        embed_custom_agents(&db.lock(), &mut spec).unwrap();

        let agent = &spec.agents["coder"];
        assert_eq!(agent.effort.as_deref(), Some("high"));
        assert_eq!(agent.mcp_servers.len(), 2);
        let gh = &agent.mcp_servers[0];
        assert_eq!(gh.name, "gh");
        assert_eq!(gh.transport, "stdio");
        assert_eq!(gh.command, "npx -y gh-mcp");
        // Keys only — the secret values never travel.
        assert_eq!(gh.env_keys, vec!["TOKEN".to_string(), "REGION".to_string()]);
        let web = &agent.mcp_servers[1];
        assert_eq!(web.url, "https://mcp.example");
        assert_eq!(web.header_keys, vec!["X-Key".to_string()]);

        let out = yaml::to_yaml(&spec).unwrap();
        assert!(!out.contains("supersecret"), "env value leaked:\n{out}");
        assert!(!out.contains("topsecret"), "header value leaked:\n{out}");
        assert!(out.contains("TOKEN"), "env key should be present:\n{out}");
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

    // ───────────────────── spawn deliverables (§3.2) ─────────────────────────

    /// A library DB with the columns `resolve_step_deliverables` reads.
    fn library_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE skills (
                id TEXT PRIMARY KEY, name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '', body TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE mcp_servers (
                id TEXT PRIMARY KEY, name TEXT NOT NULL,
                transport TEXT NOT NULL DEFAULT 'stdio',
                command TEXT NOT NULL DEFAULT '', env TEXT NOT NULL DEFAULT '',
                url TEXT NOT NULL DEFAULT '', headers TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE custom_agents (
                id TEXT PRIMARY KEY,
                skill_ids TEXT NOT NULL DEFAULT '[]',
                mcp_server_ids TEXT NOT NULL DEFAULT '[]',
                effort TEXT
             );
             INSERT INTO skills VALUES
                ('sk1', 'code-review', 'review well', '# Review'),
                ('sk2', 'tests-first', 'tests first', '# Tests');
             INSERT INTO mcp_servers VALUES
                ('m1', 'gh', 'stdio', 'npx -y gh-mcp',
                 'TOKEN=t' || char(10) || 'BAD-LINE', '', ''),
                ('m2', 'web', 'http', '', '', ' https://mcp.example ', 'X-Key: abc');
             INSERT INTO custom_agents VALUES
                ('ca1', '[\"sk1\",\"dangling\"]', '[\"m1\",\"m2\",\"gone\"]', 'high'),
                ('ca-corrupt', 'not-json', '{\"nope\"', NULL);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn deliverables_resolve_custom_agent_ids_plus_spec_names() {
        let conn = library_db();
        let d = resolve_step_deliverables(
            &conn,
            Some("ca1"),
            &["tests-first".into(), "code-review".into(), "unknown".into()],
            &[],
            "claude",
        );
        // ca1's sk1 first (assignment order), then the spec name that isn't a
        // duplicate; the dangling id and unknown name are reported as missing.
        let names: Vec<&str> = d.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["code-review", "tests-first"]);
        assert_eq!(d.missing_skills, vec!["dangling", "unknown"]);
        // ca1 carries effort 'high' — inherited by a step that doesn't override.
        assert_eq!(d.effort.as_deref(), Some("high"));
        // ca1's deleted server id is reported — a saved agent must never lose
        // part of its requested capability snapshot silently.
        assert_eq!(d.missing_mcp_servers, vec!["gone"]);
        assert!(d.missing_custom_agent.is_none());
        let (skills, mcp) = (d.skills, d.mcp_servers);
        assert_eq!(skills[0].body, "# Review");
        // claude supports all transports: both servers, parsed.
        assert_eq!(mcp.len(), 2);
        assert_eq!(mcp[0].command, "npx");
        assert_eq!(mcp[0].args, vec!["-y", "gh-mcp"]);
        assert_eq!(mcp[0].env, vec![("TOKEN".to_string(), "t".to_string())]);
        assert_eq!(mcp[1].url, "https://mcp.example");
        assert_eq!(
            mcp[1].headers,
            vec![("X-Key".to_string(), "abc".to_string())]
        );
    }

    #[test]
    fn deliverables_filter_http_servers_for_stdio_only_providers() {
        let conn = library_db();
        let d = resolve_step_deliverables(&conn, Some("ca1"), &[], &[], "codex");
        assert_eq!(d.mcp_servers.len(), 1, "codex delivers stdio only");
        assert_eq!(d.mcp_servers[0].name, "gh");
        // The provider-filtered http server ('m2') is by-design gating, never a
        // missing warning; only the genuinely deleted id is reported.
        assert_eq!(d.missing_mcp_servers, vec!["gone"]);
        let none = resolve_step_deliverables(&conn, Some("ca1"), &[], &[], "cursor").mcp_servers;
        assert!(none.is_empty(), "providers without MCP support get none");
    }

    #[test]
    fn deliverables_without_custom_agent_resolve_names_only() {
        let conn = library_db();
        let d = resolve_step_deliverables(&conn, None, &["code-review".into()], &[], "claude");
        assert_eq!(d.skills.len(), 1);
        assert_eq!(d.skills[0].name, "code-review");
        assert!(d.missing_skills.is_empty(), "everything requested resolved");
        assert!(d.missing_mcp_servers.is_empty());
        assert!(d.missing_custom_agent.is_none());
        assert!(d.effort.is_none(), "no custom agent → no inherited effort");
        assert!(
            d.mcp_servers.is_empty(),
            "no custom agent and no spec-named MCP servers → none"
        );
    }

    #[test]
    fn deliverables_resolve_spec_named_mcp_servers_against_library() {
        // An imported workflow carries MCP by embedded def; at spawn the names
        // resolve against the local library (the warn-don't-fail path skills
        // take), with the local secret values — not the file's key-only shape.
        let conn = library_db();
        let d = resolve_step_deliverables(
            &conn,
            None,
            &[],
            &["gh".into(), "nonexistent".into()],
            "claude",
        );
        assert_eq!(d.mcp_servers.len(), 1);
        assert_eq!(d.mcp_servers[0].name, "gh");
        assert_eq!(
            d.mcp_servers[0].env,
            vec![("TOKEN".to_string(), "t".to_string())]
        );
        assert_eq!(d.missing_mcp_servers, vec!["nonexistent"]);
    }

    #[test]
    fn deliverables_dedupe_mcp_from_custom_agent_and_spec_name() {
        // 'gh' comes via ca1's ids and again via the spec name — attach once.
        let conn = library_db();
        let d = resolve_step_deliverables(&conn, Some("ca1"), &[], &["gh".into()], "claude");
        assert_eq!(d.mcp_servers.iter().filter(|m| m.name == "gh").count(), 1);
    }

    #[test]
    fn deliverables_report_a_deleted_custom_agent() {
        let conn = library_db();
        let d =
            resolve_step_deliverables(&conn, Some("gone"), &["code-review".into()], &[], "claude");
        assert_eq!(d.missing_custom_agent.as_deref(), Some("gone"));
        // Spec-named skills still resolve; only the agent's assignments are lost.
        assert_eq!(d.skills.len(), 1);
        assert!(d.mcp_servers.is_empty());
        assert!(d.missing_skills.is_empty());
        // The agent row itself is the missing thing — its unknowable server
        // assignments are not double-reported.
        assert!(d.missing_mcp_servers.is_empty());
    }

    #[test]
    fn deliverables_report_unreadable_assignment_columns() {
        let conn = library_db();
        // The row exists but both assignment columns are malformed JSON: this
        // must warn per class, not collapse to "nothing requested".
        let d = resolve_step_deliverables(&conn, Some("ca-corrupt"), &[], &[], "claude");
        assert!(d.missing_custom_agent.is_none(), "the row itself resolves");
        assert_eq!(d.missing_skills.len(), 1);
        assert!(
            d.missing_skills[0].contains("'ca-corrupt' skill_ids unreadable"),
            "{:?}",
            d.missing_skills
        );
        assert_eq!(d.missing_mcp_servers.len(), 1);
        assert!(
            d.missing_mcp_servers[0].contains("'ca-corrupt' mcp_server_ids unreadable"),
            "{:?}",
            d.missing_mcp_servers
        );
        assert!(d.skills.is_empty() && d.mcp_servers.is_empty());
    }
}
