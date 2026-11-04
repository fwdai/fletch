//! Turning a step's agent alias into a spawn request.

use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;
use serde_json::json;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::workflow::driver::SpawnReq;
use crate::workflow::journal::journal_event;
use crate::workflow::spec::{AgentSpec, Spec, Step};
use crate::workflow::types::event_type;

pub(crate) fn resolve_agent<'a>(spec: &'a Spec, step: &Step) -> Result<&'a AgentSpec> {
    spec.agents.get(&step.agent).ok_or_else(|| {
        Error::Other(format!(
            "step '{}' references unknown agent '{}'",
            step.id, step.agent
        ))
    })
}

/// An explicit `AgentSpec` override, treating a blank string as unset so a
/// blank `model`/`effort`/`instructions` in a hand-authored or imported spec
/// falls back to the linked custom agent's value instead of spawning with an
/// empty argument. The check is trim-based, so a whitespace-only value (`"   "`,
/// reachable from hand-authored YAML) is also treated as unset; the original
/// (untrimmed) value is returned when it has content. Matches the custom-agent
/// side's normalization in `resolve_step_deliverables`.
fn nonblank(value: &Option<String>) -> Option<String> {
    value.clone().filter(|s| !s.trim().is_empty())
}

/// The spawn request for one step's agent: the alias's provider and overrides,
/// its custom agent's deliverables, and the run workspace to adopt. Journals a
/// warning on `warn_exec_id` for every deliverable that no longer resolves.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_spawn_req(
    conn: &Connection,
    engine: Option<&Arc<EngineCtx>>,
    agent_spec: &AgentSpec,
    fork_base: &str,
    repo: &Path,
    workspace: &Path,
    run_id: &str,
    warn_exec_id: Option<&str>,
) -> SpawnReq {
    let mcp_server_names: Vec<String> = agent_spec
        .mcp_servers
        .iter()
        .map(|d| d.name.clone())
        .collect();
    let deliverables = crate::workflow::definition::resolve_step_deliverables(
        conn,
        agent_spec.custom_agent.as_deref(),
        &agent_spec.skills,
        &mcp_server_names,
        &agent_spec.base,
    );
    if let Some(exec_id) = warn_exec_id {
        if let Some(ca_id) = &deliverables.missing_custom_agent {
            journal_event(
                conn,
                engine,
                run_id,
                event_type::CUSTOM_AGENT_MISSING,
                Some(exec_id),
                &json!({ "custom_agent": ca_id }),
            );
        }
        if !deliverables.missing_skills.is_empty() {
            journal_event(
                conn,
                engine,
                run_id,
                event_type::SKILLS_MISSING,
                Some(exec_id),
                &json!({ "skills": deliverables.missing_skills }),
            );
        }
        if !deliverables.missing_mcp_servers.is_empty() {
            journal_event(
                conn,
                engine,
                run_id,
                event_type::MCP_SERVERS_MISSING,
                Some(exec_id),
                &json!({ "mcp_servers": deliverables.missing_mcp_servers }),
            );
        }
        // A by-name match that hit multiple same-named rows: the step ran
        // against a deterministic pick (lowest id), but say so — the user may
        // have meant a different one (warn-don't-fail).
        if !deliverables.ambiguous_skills.is_empty() {
            journal_event(
                conn,
                engine,
                run_id,
                event_type::SKILLS_AMBIGUOUS,
                Some(exec_id),
                &json!({ "skills": deliverables.ambiguous_skills }),
            );
        }
        if !deliverables.ambiguous_mcp_servers.is_empty() {
            journal_event(
                conn,
                engine,
                run_id,
                event_type::MCP_SERVERS_AMBIGUOUS,
                Some(exec_id),
                &json!({ "mcp_servers": deliverables.ambiguous_mcp_servers }),
            );
        }
    }
    SpawnReq {
        repo_path: repo.to_path_buf(),
        provider: agent_spec.base.clone(),
        // The alias's explicit model/effort/instructions win; otherwise inherit
        // the linked custom agent's values (§3.2). Resolved here rather than in
        // the driver so the one place that reads the custom_agents row owns the
        // fallback — and so a live custom-agent step spawns identically to the
        // same alias after YAML export+import (which inlines these onto the
        // AgentSpec via `embed_custom_agents`).
        //
        // A *blank* explicit value (`""`, e.g. from a hand-authored/imported
        // YAML) is treated as unset, so it falls through to the custom agent's
        // value rather than spawning with an empty argument. This matches the
        // empty→None normalization the custom-agent side already applies (see
        // `resolve_step_deliverables` / `embed_custom_agents`).
        model: nonblank(&agent_spec.model).or(deliverables.model.clone()),
        effort: nonblank(&agent_spec.effort).or(deliverables.effort.clone()),
        instructions: nonblank(&agent_spec.instructions).or(deliverables.instructions.clone()),
        custom_agent_id: agent_spec.custom_agent.clone(),
        skills: deliverables.skills,
        mcp_servers: deliverables.mcp_servers,
        fork_base: Some(fork_base.to_string()),
        owner_run_id: run_id.to_string(),
        // The step agent works in the run repo's tree rather than cloning its
        // own, so the whole run shares one checkout and one line of commits.
        // `fork_base` stays as the record of where this step started.
        existing_workspace: Some(workspace.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library DB with one custom agent carrying a model/effort/instructions,
    /// for the `build_spawn_req` precedence checks below.
    fn spawn_req_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE skills (id TEXT PRIMARY KEY, name TEXT NOT NULL,
                    description TEXT NOT NULL DEFAULT '', body TEXT NOT NULL DEFAULT '');
                 CREATE TABLE mcp_servers (id TEXT PRIMARY KEY, name TEXT NOT NULL,
                    transport TEXT NOT NULL DEFAULT 'stdio', command TEXT NOT NULL DEFAULT '',
                    env TEXT NOT NULL DEFAULT '', url TEXT NOT NULL DEFAULT '',
                    headers TEXT NOT NULL DEFAULT '');
                 CREATE TABLE custom_agents (id TEXT PRIMARY KEY,
                    skill_ids TEXT NOT NULL DEFAULT '[]', mcp_server_ids TEXT NOT NULL DEFAULT '[]',
                    effort TEXT, model TEXT, instructions TEXT NOT NULL DEFAULT '');
                 INSERT INTO custom_agents (id, effort, model, instructions)
                    VALUES ('ca1', 'high', 'opus', 'Be thorough.');",
        )
        .unwrap();
        conn
    }

    fn agent_spec(custom_agent: Option<&str>) -> AgentSpec {
        AgentSpec {
            base: "claude".into(),
            model: None,
            effort: None,
            instructions: None,
            skills: vec![],
            mcp_servers: vec![],
            custom_agent: custom_agent.map(str::to_string),
        }
    }

    #[test]
    fn build_spawn_req_inherits_custom_agent_model_effort_instructions() {
        // The coherency bar: a live custom-agent-backed alias must spawn with the
        // same model/effort/instructions the export+import path inlines onto the
        // AgentSpec — sourced here from the custom_agents row as a fallback.
        let conn = spawn_req_conn();
        let dummy = Path::new("/tmp/repo");
        let req = build_spawn_req(
            &conn,
            None,
            &agent_spec(Some("ca1")),
            "base",
            dummy,
            dummy,
            "r",
            None,
        );
        assert_eq!(req.model.as_deref(), Some("opus"));
        assert_eq!(req.effort.as_deref(), Some("high"));
        assert_eq!(req.instructions.as_deref(), Some("Be thorough."));
    }

    #[test]
    fn build_spawn_req_explicit_alias_values_win_over_custom_agent() {
        let conn = spawn_req_conn();
        let mut spec = agent_spec(Some("ca1"));
        spec.model = Some("sonnet".into());
        spec.effort = Some("low".into());
        spec.instructions = Some("Override brief.".into());
        let dummy = Path::new("/tmp/repo");
        let req = build_spawn_req(&conn, None, &spec, "base", dummy, dummy, "r", None);
        assert_eq!(req.model.as_deref(), Some("sonnet"));
        assert_eq!(req.effort.as_deref(), Some("low"));
        assert_eq!(req.instructions.as_deref(), Some("Override brief."));
    }

    #[test]
    fn build_spawn_req_blank_explicit_values_fall_back_to_custom_agent() {
        // A blank explicit override — empty *or* whitespace-only (both reachable
        // from hand-authored/imported YAML) — must not block the custom agent's
        // value or reach spawn as an empty argument; it's treated as unset.
        let conn = spawn_req_conn();
        let dummy = Path::new("/tmp/repo");
        for blank in ["", "   ", "\t\n"] {
            let mut spec = agent_spec(Some("ca1"));
            spec.model = Some(blank.to_string());
            spec.effort = Some(blank.to_string());
            spec.instructions = Some(blank.to_string());
            let req = build_spawn_req(&conn, None, &spec, "base", dummy, dummy, "r", None);
            assert_eq!(req.model.as_deref(), Some("opus"), "blank {blank:?}");
            assert_eq!(req.effort.as_deref(), Some("high"), "blank {blank:?}");
            assert_eq!(
                req.instructions.as_deref(),
                Some("Be thorough."),
                "blank {blank:?}"
            );
        }
    }

    #[test]
    fn build_spawn_req_dangling_custom_agent_inherits_nothing() {
        // A deleted custom agent leaves the alias's own (here unset) values —
        // unchanged behavior, never a resolution error.
        let conn = spawn_req_conn();
        let dummy = Path::new("/tmp/repo");
        let req = build_spawn_req(
            &conn,
            None,
            &agent_spec(Some("gone")),
            "base",
            dummy,
            dummy,
            "r",
            None,
        );
        assert!(req.model.is_none());
        assert!(req.effort.is_none());
        assert!(req.instructions.is_none());
    }
}
