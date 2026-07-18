//! Provider / tooling handlers: composer slash-command discovery, one-shot
//! `claude` invocations, CLI version probes, portable-git and agent installs,
//! custom-bin validation, and model discovery.

use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::agent::{BinValidation, ProviderProbe, ToolStatus};
use crate::error::{Error, Result};
use crate::supervisor::Supervisor;

use super::files::{expand_tilde, primary_repo_checkout};

/// Discover the user- and project-level slash commands a provider exposes on
/// disk (e.g. Claude's `~/.claude/commands` + `<project>/.claude/commands`),
/// for the composer's `/` autocomplete. `project_dir` is the agent's project
/// root, or None for the new-agent composer before a project is chosen. Empty
/// (never an error) for providers without command discovery or when the dirs
/// are absent.
#[tauri::command]
pub async fn discover_slash_commands(
    provider: String,
    project_dir: Option<String>,
) -> Result<Vec<crate::slash_commands::DiscoveredCommand>> {
    let project = project_dir.as_deref().map(expand_tilde);
    Ok(crate::slash_commands::discover(
        &provider,
        project.as_deref(),
    ))
}

/// Captured output of a one-shot `claude <args>` invocation, run for a local
/// slash command the stream-json session can't service (e.g. `/doctor` →
/// `claude doctor`). Rendered into the chat as a notice; `success` is the exit
/// status so the UI can flag failures.
#[derive(Serialize)]
pub struct ClaudeCommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

/// Run a read-only `claude` subcommand (e.g. `doctor`, `mcp list`) in the
/// agent's checkout and capture its output. Runs unsandboxed like the
/// model-list probes (a read-only CLI query), honoring a per-agent binary
/// override before PATH discovery. `args` is a fixed command vocabulary chosen
/// by the frontend dispatcher, never free user input.
#[tauri::command]
pub async fn run_claude_command(
    supervisor: State<'_, Arc<Supervisor>>,
    agent_id: String,
    args: Vec<String>,
) -> Result<ClaudeCommandOutput> {
    let (_, checkout) = primary_repo_checkout(&supervisor, &agent_id)?;
    let home = dirs::home_dir().ok_or_else(|| Error::Other("no home directory".into()))?;
    let bin = match crate::bin_resolve::resolve_agent_override(&agent_id, &home) {
        Some(Ok(path)) => path,
        _ => crate::bin_resolve::resolve_bin("claude", &home)
            .ok_or_else(|| Error::Other("claude binary not found".into()))?,
    };

    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(&args).current_dir(&checkout).kill_on_drop(true);
    if let Some(env) = crate::bin_resolve::login_shell_env() {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }

    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
    let out = tokio::time::timeout(TIMEOUT, cmd.output())
        .await
        .map_err(|_| Error::Other(format!("claude {} timed out", args.join(" "))))?
        .map_err(|e| Error::Other(format!("run claude {}: {e}", args.join(" "))))?;

    Ok(ClaudeCommandOutput {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        success: out.status.success(),
    })
}

/// Probe every known provider's CLI binary: resolve its path, run `--version`,
/// and return what was found. Missing or uninstalled providers return `None`
/// for both fields; the frontend falls back to hardcoded defaults.
#[tauri::command]
pub async fn probe_provider_versions() -> Vec<ProviderProbe> {
    crate::agent::probe_all_providers().await
}

/// Resolve a required CLI and probe its `--version`. Drives the first-run
/// readiness check. For `git` this reflects unified resolution (system or the
/// portable dist — see `git_dist`); other tools are presence-only.
#[tauri::command]
pub async fn check_cli(name: String) -> ToolStatus {
    tokio::task::spawn_blocking(move || crate::agent::check_cli(&name))
        .await
        .unwrap_or(ToolStatus {
            installed: false,
            version: None,
            path: None,
            source: None,
        })
}

/// Manually (re)trigger portable-git resolution/installation. The startup
/// bootstrap may have failed (offline first launch, blocked network) — this
/// gives the readiness UI a retry that doesn't require an app restart. Emits
/// the same `git-dist:state` events as the startup path, so existing
/// listeners render its progress unchanged.
#[tauri::command]
pub async fn git_dist_install(app: AppHandle) -> Result<()> {
    use tauri::Emitter;
    crate::git_dist::resolve_or_install(move |payload| {
        let _ = app.emit("git-dist:state", payload);
    })
    .await
    .map_err(Error::Other)
}

/// Run the pinned official installer for an agent CLI (see `agent_install`),
/// streaming progress via `agent-install:state` events. Resolves when the
/// installer exits; the frontend re-probes providers afterwards to confirm
/// the binary is now detectable.
#[tauri::command]
pub async fn install_agent(app: AppHandle, id: String) -> Result<()> {
    use tauri::Emitter;
    crate::agent_install::install(id, move |payload| {
        let _ = app.emit("agent-install:state", payload);
    })
    .await
    .map_err(Error::Other)
}

/// Validate a candidate custom agent binary path: is it an executable file,
/// and what `--version` does it report? The providers settings UI calls this
/// before saving a path override so it can show immediate inline feedback
/// (green version on success, error on failure) and block a broken save.
#[tauri::command]
pub async fn validate_agent_bin(path: String) -> BinValidation {
    tokio::task::spawn_blocking(move || crate::agent::validate_bin(&path))
        .await
        .unwrap_or(BinValidation {
            executable: false,
            version: None,
        })
}

/// Discover the models each agent CLI reports it supports (raw ids + any cheap
/// metadata the CLI provides). The frontend enriches these against models.dev
/// to build the unified catalog. Never errors — an absent/broken CLI just
/// contributes no models.
#[tauri::command]
pub async fn discover_supported_models() -> Vec<crate::model_catalog::AgentModels> {
    crate::model_catalog::discover_supported_models().await
}
