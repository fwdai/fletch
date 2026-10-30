//! Integration of the `codegraph` code-indexing tool
//! (<https://github.com/colbymchenry/codegraph>) so Fletch's sandboxed agents
//! can query a repo's symbols and call graph over MCP instead of grepping files.
//!
//! **One shape for every engine that can run it.** The MCP server runs next to
//! the agent's checkout, inside the agent's sandbox, as a stdio child of the
//! agent's CLI: it is what keeps the index in step with the agent's edits (its
//! file watcher), so it has to live where those edits land. Only *which binary*
//! differs, resolved per engine the way the agent CLI itself is
//! ([`server_bin`]): a seatbelt agent runs on this host and execs the host's
//! bundle; a box's guest runs the Linux build of the same release from its
//! tools disk (`fletch_box::tools`). A container engine has neither.
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
//! `~/.fletch/projects/<project_id>/codegraph/<repo>/`, index that on the host,
//! and copy the resulting `.codegraph/codegraph.db` into each fresh agent
//! checkout through the checkout's executor (`crate::exec`): a file copy for a
//! tree on this host, a write over the channel for a box's. The index db is
//! content-hash based and stores project-root-relative paths, so a db built
//! against the mirror is valid in any other checkout of the same repo, on
//! either OS; the MCP server running in the workspace picks it up and keeps it
//! fresh with its own file watcher.
//!
//! Everything here is **best-effort**: install, mirror, and copy failures are
//! logged and swallowed. A spawn never fails because indexing didn't work — the
//! agent just falls back to searching files, and a later spawn retries.

mod install;
mod mirror;

pub use install::{ensure_guest_bundle, ensure_installed, CODEGRAPH_VERSION};
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

/// The codegraph launcher as `agent_id`'s sandbox sees it, when it has one —
/// the one engine-specific input to injection, resolved the way the agent's
/// CLI path is (`agent::spawn`): the host's install for a seatbelt agent,
/// which runs on this host; the Linux build on the tools disk the agent's box
/// runs with (`boxes::codegraph_bin`); nothing for a container, which can't
/// exec the host's macOS binary and has no other.
fn server_bin(engine: EngineKind, agent_id: &str, provider: &str) -> Option<String> {
    match engine {
        EngineKind::SandboxExec => is_installed()
            .then(bin_path)
            .and_then(Result::ok)
            .map(|bin| bin.to_string_lossy().into_owned()),
        EngineKind::Box => crate::sandbox::boxes::codegraph_bin(agent_id, provider),
        EngineKind::Docker | EngineKind::Podman => None,
    }
}

/// The codegraph MCP server snapshot for injection, running `command` (from
/// [`server_bin`]). stdio server running `codegraph serve --mcp`; the
/// server resolves its project by walking up from its cwd (the agent's checkout)
/// to the nearest `.codegraph/`. `CODEGRAPH_TELEMETRY=0` disables telemetry;
/// `CODEGRAPH_NO_DAEMON` is intentionally *not* set here — the in-workspace
/// daemon/watcher keeps the index fresh live while the agent works.
fn mcp_server_snapshot(command: String) -> McpServerSnapshot {
    McpServerSnapshot {
        name: "codegraph".into(),
        transport: "stdio".into(),
        command,
        args: vec!["serve".into(), "--mcp".into()],
        env: vec![("CODEGRAPH_TELEMETRY".into(), "0".into())],
        ..Default::default()
    }
}

/// Pure gate for MCP injection, factored out so the decision is testable
/// without touching global state or the filesystem. Inject only when indexing
/// is enabled, the engine can run codegraph at all, the binary is there for
/// this session's sandbox ([`server_bin`]), the provider can actually *receive*
/// an MCP server, and no server named `"codegraph"` (case-insensitive) already
/// exists — a user-defined one must never be shadowed, and its config key would
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
    binary_available: bool,
    can_receive_mcp: bool,
    existing_names: &[&str],
) -> bool {
    wants_codegraph(enabled, engine, can_receive_mcp, existing_names) && binary_available
}

/// Everything that decides whether codegraph takes part in a session *except*
/// whether the binary is there yet: indexing is on, the engine can run
/// codegraph, the provider's CLI can actually receive an MCP server, and the
/// session hasn't already defined its own `"codegraph"` (case-insensitive) that
/// we must not shadow.
///
/// Factored out because the injection gate and the index-provisioning gate need
/// exactly these four and must never drift — a session that won't get the server
/// must not pay to build an index, and vice versa. They differ on the fifth
/// condition only: `should_inject` requires the binary to already be there,
/// while provisioning is the code path that *installs* it (on the host) or runs
/// before the box's disk carries it, so requiring it there would deadlock the
/// first run.
fn wants_codegraph(
    enabled: bool,
    engine: EngineKind,
    can_receive_mcp: bool,
    existing_names: &[&str],
) -> bool {
    // The server runs beside the checkout, inside the agent's sandbox: on this
    // host for seatbelt, in the guest for a box. A container gets no build of
    // it.
    let engine_runs_codegraph = match engine {
        EngineKind::SandboxExec | EngineKind::Box => true,
        EngineKind::Docker | EngineKind::Podman => false,
    };
    enabled
        && engine_runs_codegraph
        && can_receive_mcp
        && !existing_names
            .iter()
            .any(|n| n.eq_ignore_ascii_case("codegraph"))
}

/// [`wants_codegraph`] resolved against live state, for the index-provisioning
/// gate. `session_servers` is the session's own MCP snapshot — the same list
/// `inject_mcp_server` checks for a user-defined `"codegraph"` collision.
pub fn session_wants_codegraph(
    engine: EngineKind,
    provider: &str,
    session_servers: &[McpServerSnapshot],
) -> bool {
    let names: Vec<&str> = session_servers.iter().map(|s| s.name.as_str()).collect();
    wants_codegraph(
        enabled(),
        engine,
        crate::agent::mcp_delivery(provider).is_some(),
        &names,
    )
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
/// allows it for `agent_id`'s session, plus whether that happened. Injection is
/// dynamic — the returned list feeds the config writers directly and is never
/// persisted onto the session snapshot, so the toggle's *current* state (and,
/// in a box, the tools disk the box runs with) always wins for both fresh
/// spawns and resumes.
pub fn inject_mcp_server(
    servers: &[McpServerSnapshot],
    engine: EngineKind,
    agent_id: &str,
    provider: &str,
) -> McpInjection {
    let mut out = servers.to_vec();
    let server = {
        let names: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();
        let enabled = enabled();
        let can_receive_mcp = crate::agent::mcp_delivery(provider).is_some();
        // Looked up only for a session that wants it: in a box that reads the
        // tools disks, whose names need the provider CLI's probed version.
        let bin = wants_codegraph(enabled, engine, can_receive_mcp, &names)
            .then(|| server_bin(engine, agent_id, provider))
            .flatten();
        let allow = should_inject(enabled, engine, bin.is_some(), can_receive_mcp, &names);
        bin.filter(|_| allow).map(mcp_server_snapshot)
    };
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
    fn should_inject_skips_containers() {
        // A container can't exec the host macOS binary and gets no other.
        assert!(!should_inject(true, EngineKind::Docker, true, true, &[]));
        assert!(!should_inject(true, EngineKind::Podman, true, true, &[]));
    }

    #[test]
    fn a_box_takes_part_like_a_seatbelt_agent() {
        // Same gate for both engines that run the server beside the checkout;
        // only where the binary comes from differs (`server_bin`).
        for engine in [EngineKind::SandboxExec, EngineKind::Box] {
            assert!(wants_codegraph(true, engine, true, &[]), "{engine:?}");
            assert!(should_inject(true, engine, true, true, &[]), "{engine:?}");
            // A box whose tools disk doesn't carry it is told nothing.
            assert!(!should_inject(true, engine, false, true, &[]), "{engine:?}");
        }
    }

    #[test]
    fn the_server_command_is_where_the_agent_runs() {
        // A provider with no CLI to probe: the box branch reads tools disk
        // names, which carry the CLI's version.
        let provider = "no-such-provider";
        // A container has no build of codegraph at all.
        assert!(server_bin(EngineKind::Docker, "a", provider).is_none());
        assert!(server_bin(EngineKind::Podman, "a", provider).is_none());
        // A seatbelt agent execs the host's install, if there is one.
        if let Some(bin) = server_bin(EngineKind::SandboxExec, "a", provider) {
            assert_eq!(PathBuf::from(bin), bin_path().unwrap());
        }
        // A box execs the guest's, from its tools disk, never the host's.
        if let Some(bin) = server_bin(EngineKind::Box, "a", provider) {
            assert_eq!(bin, fletch_box::tools::codegraph_bin_path());
        }
    }

    #[test]
    fn the_snapshot_runs_the_given_command() {
        let snap = mcp_server_snapshot("/opt/fletch/tools/codegraph/bin/codegraph".into());
        assert_eq!(snap.name, "codegraph");
        assert_eq!(snap.command, "/opt/fletch/tools/codegraph/bin/codegraph");
        assert_eq!(snap.args, ["serve", "--mcp"]);
        assert_eq!(snap.transport, "stdio");
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
    fn provisioning_and_injection_agree_on_every_shared_condition() {
        // The index-provisioning gate must reject everything injection rejects,
        // or we pay for a mirror clone + parse the agent can never query. Both
        // go through `wants_codegraph`, so this pins the four shared conditions
        // rather than trusting two hand-written copies to stay in step.
        // A session that defines its own "codegraph" suppresses injection — and
        // so must suppress indexing. This is the case that shipped broken.
        assert!(!wants_codegraph(
            true,
            EngineKind::SandboxExec,
            true,
            &["CodeGraph"]
        ));
        assert!(!should_inject(
            true,
            EngineKind::SandboxExec,
            true,
            true,
            &["CodeGraph"]
        ));

        // The other three shared conditions, same answer on both sides.
        for (enabled, engine, can_mcp) in [
            (false, EngineKind::SandboxExec, true),
            (false, EngineKind::Box, true),
            (true, EngineKind::Docker, true),
            (true, EngineKind::Podman, true),
            (true, EngineKind::SandboxExec, false),
            (true, EngineKind::Box, false),
        ] {
            assert!(!wants_codegraph(enabled, engine, can_mcp, &[]));
            assert!(!should_inject(enabled, engine, true, can_mcp, &[]));
        }

        // All four satisfied: both gates open (injection additionally needs the
        // binary, which provisioning installs itself).
        assert!(wants_codegraph(true, EngineKind::SandboxExec, true, &[]));
    }

    #[test]
    fn provisioning_does_not_require_an_installed_binary() {
        // The deliberate asymmetry: provisioning is the install path, so gating
        // it on `is_installed` would stop the first run from ever bootstrapping.
        assert!(wants_codegraph(true, EngineKind::SandboxExec, true, &[]));
        assert!(!should_inject(
            true,
            EngineKind::SandboxExec,
            false,
            true,
            &[]
        ));
    }

    #[test]
    fn indexing_and_injection_share_one_provider_condition() {
        // `provision_codegraph_index` gates on the same `mcp_delivery` resolver
        // as `should_inject`. If they ever diverge we would either index for an
        // agent that can't query it (wasted clone + parse per spawn) or inject a
        // server with no index behind it. Pinning the shared resolver here keeps
        // the two honest without the supervisor needing a test harness.
        for provider in ["claude", "codex", "opencode"] {
            assert!(
                crate::agent::mcp_delivery(provider).is_some(),
                "{provider} lost MCP delivery; indexing would silently stop too"
            );
        }
        for provider in ["cursor", "pi", "antigravity"] {
            assert!(
                crate::agent::mcp_delivery(provider).is_none(),
                "{provider} gained MCP delivery; it must be indexed again too"
            );
        }
    }

    #[test]
    fn inject_reports_nothing_for_a_provider_without_mcp() {
        // End to end through the globals: whatever the toggle and the binary
        // say, a provider with no MCP surface gets neither the server nor the
        // instruction flag.
        for engine in [EngineKind::SandboxExec, EngineKind::Box] {
            let injection = inject_mcp_server(&[], engine, "agent-1", "pi");
            assert!(injection.servers.is_empty());
            assert!(!injection.codegraph_available);
        }
    }
}
