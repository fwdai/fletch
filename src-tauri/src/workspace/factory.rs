//! Value constructors and provider classification used at agent creation.

use super::*;

/// Per-turn agents run one process per turn, assign their own session id
/// (captured from their first turn's events rather than generated up
/// front), and render only in the structured (Custom) view for now. The
/// canonical set is the `agent::PER_TURN_AGENTS` descriptor table.
pub fn is_per_turn_provider(provider: &str) -> bool {
    crate::agent::per_turn_descriptor(provider).is_some()
}

/// Build a fresh AgentRecord with one primary tracked repo.
pub fn new_agent_record(
    id: String,
    name: String,
    provider: String,
    primary: TrackedRepo,
    task: String,
    view: AgentView,
) -> AgentRecord {
    // Claude attaches to a session id we generate up front (passed as
    // `--session-id`). Per-turn agents (codex, cursor) assign their own id
    // on the first turn (captured from their events), so they start with
    // none.
    let session_id = if is_per_turn_provider(&provider) {
        None
    } else {
        Some(uuid::Uuid::new_v4().to_string())
    };
    AgentRecord {
        id,
        // Populated by WorkspaceManager::add_agent (looked up from the
        // primary repo path) — empty here because callers don't know it.
        project_id: String::new(),
        name,
        provider,
        repos: vec![primary],
        task,
        status: AgentStatus::Spawning,
        view,
        session_id,
        effort: None,
        model: None,
        instructions: None,
        forked_context: None,
        custom_agent_id: None,
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        // Stamped by the spawn path (`supervisor::lifecycle::spawn_agent`)
        // from the live setting — callers building records directly (tests)
        // default to the pre-selection NULL, which spawns under sandbox-exec.
        sandbox_engine: None,
        // Set by the workflow scheduler at step spawn; a plain spawn leaves it
        // unowned so the agent shows in the normal sidebar.
        owner_run_id: None,
        // Set by the issue-intake spawn path; a plain spawn has no origin issue.
        issue_ref: None,
        created_at: Utc::now().to_rfc3339(),
        last_error: None,
        archive: None,
    }
}
