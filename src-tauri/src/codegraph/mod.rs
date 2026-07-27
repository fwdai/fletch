//! Integration of the `codegraph` code-indexing tool
//! (<https://github.com/colbymchenry/codegraph>) so Fletch's sandboxed agents
//! can query a repo's symbols and call graph over MCP instead of grepping files.
//!
//! **Why the shape here.** The tool is a self-contained release bundle (vendored
//! Node runtime) installed under `~/.fletch/tools/codegraph/` — deliberately
//! *outside* the app-data dir the sandbox policy denies agents from reading, so
//! a sandboxed agent can still `exec` the binary as an MCP stdio server. We never
//! run its `install`/`uninstall` subcommands (they rewrite `~/.claude.json` and
//! other global agent configs) and never put it on `PATH`; the binary is always
//! referenced by absolute path.
//!
//! **The index is never written to the user's source repo.** Instead we keep a
//! throwaway `git clone --shared` *mirror* per repo under
//! `~/.fletch/projects/<project_id>/codegraph/<repo>/`, index that, and copy the
//! resulting `.codegraph/codegraph.db` into each fresh agent checkout. The index
//! db is content-hash based and stores project-root-relative paths, so a db built
//! against the mirror is valid in any other checkout of the same repo; the MCP
//! server running in the workspace picks it up and keeps it fresh with its own
//! file watcher.
//!
//! Everything here is **best-effort**: install, mirror, and copy failures are
//! logged and swallowed. A spawn never fails because indexing didn't work — the
//! agent just falls back to searching files, and a later spawn retries.

mod install;
mod mirror;

pub use install::ensure_installed;
pub use mirror::{advance_mirror, append_git_exclude, copy_index_into, ensure_mirror, mirror_dir};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::agent_profile::McpServerSnapshot;
use crate::error::Result;
use crate::sandbox::EngineKind;
use crate::workspace::tools_root;

/// `settings` key for the user's code-indexing consent. Backend-owned (written
/// only by `set_code_indexing_enabled`, never a frontend `setSetting`), so it
/// uses the snake_case convention like `telemetry_enabled` / `sandbox_engine`.
/// **Default on**: absent or anything but the literal `"false"` means enabled.
pub const SETTING: &str = "code_indexing_enabled";

/// In-memory mirror of the [`SETTING`] value, so the spawn path (deep in agent
/// code, no DB handle) can read it. Seeded at startup and updated by the
/// `set_code_indexing_enabled` command. Defaults to the enabled state.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Update the in-memory enabled mirror (called from startup seed + the toggle).
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether code indexing is currently enabled (the in-memory mirror).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Interpret a raw `code_indexing_enabled` setting value as opt-out: only an
/// explicit `"false"` disables. Shared by the startup seed and the read path.
pub fn parse_enabled(raw: Option<&str>) -> bool {
    raw != Some("false")
}

/// Install root for the verified release bundle: `~/.fletch/tools/codegraph/`.
pub fn install_dir() -> Result<PathBuf> {
    Ok(tools_root()?.join("codegraph"))
}

/// Launcher symlink dir: `~/.fletch/tools/codegraph/bin`.
pub fn bin_dir() -> Result<PathBuf> {
    Ok(install_dir()?.join("bin"))
}

/// Absolute path to the installed `codegraph` binary.
pub fn bin_path() -> Result<PathBuf> {
    Ok(bin_dir()?.join("codegraph"))
}

/// Cheap existence check for the binary — the gate for MCP injection and for
/// skipping a redundant install probe on the hot spawn path.
pub fn is_installed() -> bool {
    bin_path().map(|p| p.exists()).unwrap_or(false)
}

/// Build the codegraph MCP server snapshot for injection, or `None` if the
/// binary isn't installed. stdio server running `codegraph serve --mcp`; the
/// server resolves its project by walking up from its cwd (the agent's checkout)
/// to the nearest `.codegraph/`. `CODEGRAPH_TELEMETRY=0` disables telemetry;
/// `CODEGRAPH_NO_DAEMON` is intentionally *not* set here — the in-workspace
/// daemon/watcher keeps the index fresh live while the agent works.
fn mcp_server_snapshot() -> Option<McpServerSnapshot> {
    let bin = bin_path().ok()?;
    if !bin.exists() {
        return None;
    }
    Some(McpServerSnapshot {
        name: "codegraph".into(),
        transport: "stdio".into(),
        command: bin.to_string_lossy().into_owned(),
        args: vec!["serve".into(), "--mcp".into()],
        env: vec![("CODEGRAPH_TELEMETRY".into(), "0".into())],
        ..Default::default()
    })
}

/// Pure gate for MCP injection, factored out so the decision is testable
/// without touching global state or the filesystem. Inject only when indexing
/// is enabled, the engine is not Docker (a container can't exec the host macOS
/// binary), the binary is installed, the provider can actually *receive* an MCP
/// server, and no server named `"codegraph"` (case-insensitive) already exists
/// — a user-defined one must never be shadowed, and its config key would
/// otherwise collide.
///
/// `can_receive_mcp` is resolved by the caller from
/// `agent::mcp_delivery(provider)` rather than looked up here: it keeps this
/// function free of provider ids and of the `agent` module, and it is the one
/// input that decides whether the snapshot is delivered or silently dropped
/// (pi and antigravity have no MCP surface at all).
fn should_inject(
    enabled: bool,
    engine: EngineKind,
    binary_installed: bool,
    can_receive_mcp: bool,
    existing_names: &[&str],
) -> bool {
    enabled
        && engine != EngineKind::Docker
        && binary_installed
        && can_receive_mcp
        && !existing_names
            .iter()
            .any(|n| n.eq_ignore_ascii_case("codegraph"))
}

/// The outcome of the dynamic codegraph fold.
pub struct McpInjection {
    /// The session's servers, with codegraph appended when the gate allowed
    /// it. Feeds the provider's delivery builder directly.
    pub servers: Vec<McpServerSnapshot>,
    /// Whether codegraph is actually in `servers` — i.e. whether this agent
    /// will really have the tool. Every gate in [`should_inject`] can drop it
    /// silently, so the instruction layer keys off this flag rather than off
    /// the settings toggle (see `instructions::codegraph_block`).
    pub codegraph_available: bool,
}

/// Return `servers` with the codegraph MCP server appended when [`should_inject`]
/// allows it, plus whether that happened. Injection is dynamic — the returned
/// list feeds the config writers directly and is never persisted onto the
/// session snapshot, so the toggle's *current* state always wins for both fresh
/// spawns and resumes.
pub fn inject_mcp_server(
    servers: &[McpServerSnapshot],
    engine: EngineKind,
    provider: &str,
) -> McpInjection {
    let mut out = servers.to_vec();
    let allow = {
        let names: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();
        should_inject(
            enabled(),
            engine,
            is_installed(),
            crate::agent::mcp_delivery(provider).is_some(),
            &names,
        )
    };
    // `is_installed()` was true, so this is `Some` barring a race with an
    // uninstall between the two checks — in which case we simply skip, and the
    // flag stays false so nothing advertises a tool that isn't there.
    let server = if allow { mcp_server_snapshot() } else { None };
    let codegraph_available = server.is_some();
    out.extend(server);
    McpInjection {
        servers: out,
        codegraph_available,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_enabled_is_opt_out() {
        assert!(parse_enabled(None));
        assert!(parse_enabled(Some("true")));
        assert!(parse_enabled(Some("anything")));
        assert!(!parse_enabled(Some("false")));
    }

    #[test]
    fn should_inject_happy_path() {
        assert!(should_inject(
            true,
            EngineKind::SandboxExec,
            true,
            true,
            &[]
        ));
    }

    #[test]
    fn should_inject_requires_enabled() {
        assert!(!should_inject(
            false,
            EngineKind::SandboxExec,
            true,
            true,
            &[]
        ));
    }

    #[test]
    fn should_inject_skips_docker() {
        // A container can't exec the host macOS binary.
        assert!(!should_inject(true, EngineKind::Docker, true, true, &[]));
    }

    #[test]
    fn should_inject_requires_installed_binary() {
        assert!(!should_inject(
            true,
            EngineKind::SandboxExec,
            false,
            true,
            &[]
        ));
    }

    #[test]
    fn should_inject_requires_an_mcp_surface() {
        // pi and antigravity can't consume MCP at all, so a snapshot pushed at
        // them is dropped on the floor — don't index for them and don't tell
        // them the tool exists.
        assert!(!should_inject(
            true,
            EngineKind::SandboxExec,
            true,
            false,
            &[]
        ));
        for provider in ["pi", "antigravity"] {
            assert!(
                crate::agent::mcp_delivery(provider).is_none(),
                "{provider} would now pass the gate"
            );
        }
    }

    #[test]
    fn should_inject_skips_user_defined_collision() {
        // Case-insensitive: a user-defined "CodeGraph" must not be shadowed.
        assert!(!should_inject(
            true,
            EngineKind::SandboxExec,
            true,
            true,
            &["github", "CodeGraph"]
        ));
    }

    #[test]
    fn inject_reports_nothing_for_a_provider_without_mcp() {
        // End to end through the globals: whatever the toggle and the binary
        // say, a provider with no MCP surface gets neither the server nor the
        // instruction flag.
        let injection = inject_mcp_server(&[], EngineKind::SandboxExec, "pi");
        assert!(injection.servers.is_empty());
        assert!(!injection.codegraph_available);
    }
}
