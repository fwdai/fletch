//! Agent lifecycle: the argument defaults every spawn funnels through.

use std::path::PathBuf;
use std::sync::Arc;

use crate::error::Result;
use crate::host::EngineCtx;
use crate::supervisor::{SpawnRequest, Supervisor};
use crate::workspace::{AgentRecord, AgentView};

/// The user-spawn field mapping: the argument defaults every caller funnels
/// through on the way to `Supervisor::spawn_agent`. Shared by the desktop's
/// `spawn_agent` command and the remote dispatcher, which passes `None` for the
/// custom-agent fields the mobile surface does not expose.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_agent_impl(
    sup: Arc<Supervisor>,
    ctx: Arc<EngineCtx>,
    view: Option<AgentView>,
    repo_path: String,
    provider: Option<String>,
    name: Option<String>,
    effort: Option<String>,
    model: Option<String>,
    instructions: Option<String>,
    custom_agent_id: Option<String>,
    skills: Option<Vec<crate::agent_profile::SkillSnapshot>>,
    mcp_servers: Option<Vec<crate::agent_profile::McpServerSnapshot>>,
    fork_base: Option<String>,
    issue_ref: Option<String>,
    purpose: Option<String>,
) -> Result<AgentRecord> {
    sup.spawn_agent(
        ctx,
        SpawnRequest {
            view: view.unwrap_or_default(),
            repo_path: PathBuf::from(repo_path),
            provider: provider.unwrap_or_else(|| "claude".to_string()),
            name,
            effort,
            model,
            instructions,
            // Forked-conversation context is set only by the fork path.
            forked_context: None,
            custom_agent_id,
            skills: skills.unwrap_or_default(),
            mcp_servers: mcp_servers.unwrap_or_default(),
            fork_base,
            // User-initiated spawns fork from the source repo, not a run repo,
            // and are never run-owned; the scheduler sets both for a step spawn.
            run_repo: None,
            owner_run_id: None,
            // Adopting a shared run workspace is a workflow-kernel-only path.
            existing_workspace: None,
            // Carrying another workspace's working tree is a fork-only path.
            carry_from: None,
            // Set when the spawn originates from a Home-inbox issue.
            issue_ref,
            purpose,
        },
    )
    .await
}
