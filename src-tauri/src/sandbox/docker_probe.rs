//! Temporary Docker availability probe for engine selection (slice C1).
//!
//! Replaced by `sandbox/docker/probe.rs` when slice B2 wires the engines
//! together with the full `sandbox/docker/` module (which owns caching, the
//! CLI locator, and the real `DockerEngine`). Kept in its own file so that
//! swap deletes this file without touching the rest of `sandbox/`.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::bin_resolve;
use crate::error::{Error, Result};

use super::engine::{AgentLaunchCtx, EngineKind, LaunchPlan, SandboxEngine};

/// How long the daemon gets to answer `docker version` before we call it
/// down. Un-timed, the CLI blocks for ~30s when Docker Desktop is installed
/// but stopped — far too long for spawn paths and UI polling.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Result of probing the local Docker installation. Serializes to the wire
/// shape the settings UI consumes: `{ "status": "...", "version"?: "..." }`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum DockerAvailability {
    /// Daemon reachable; `version` is the server version it reported.
    Available { version: String },
    /// No docker binary found on this machine.
    NotInstalled,
    /// Binary present but the daemon didn't answer (not running, or hung
    /// past the probe timeout).
    DaemonDown,
}

/// Probe Docker: resolve the CLI (PATH / login-shell PATH / common install
/// dirs, same as agent binaries), then ask the daemon for its server version
/// under a hard timeout.
pub fn availability() -> DockerAvailability {
    let Some(home) = dirs::home_dir() else {
        return DockerAvailability::NotInstalled;
    };
    let Some(docker) = bin_resolve::resolve_bin("docker", &home) else {
        return DockerAvailability::NotInstalled;
    };
    let mut child = match Command::new(&docker)
        .args(["version", "--format", "{{.Server.Version}}"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        // The binary resolved but won't exec — treat as not installed rather
        // than daemon-down: there is nothing to "start".
        Err(_) => return DockerAvailability::NotInstalled,
    };

    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Output is a single short version line, so reading after exit
                // can't deadlock on a full pipe.
                let mut out = String::new();
                if let Some(stdout) = child.stdout.as_mut() {
                    let _ = stdout.read_to_string(&mut out);
                }
                let version = out.trim();
                return if status.success() && !version.is_empty() {
                    DockerAvailability::Available {
                        version: version.to_string(),
                    }
                } else {
                    // CLI present but no server answered.
                    DockerAvailability::DaemonDown
                };
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            // Timed out (or the child handle broke): reap and report down.
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return DockerAvailability::DaemonDown;
            }
        }
    }
}

/// Stand-in for the Docker engine until slice B2 lands its launch path.
/// Selecting docker is availability-gated in settings, but no engine exists
/// yet, so a docker-stamped agent that reaches launch gets a clear error
/// instead of a silent seatbelt downgrade. The setting (and the agent's
/// stamped engine) persist — B2 makes both spawn for real.
pub struct DockerEngineStub;

impl SandboxEngine for DockerEngineStub {
    fn kind(&self) -> EngineKind {
        EngineKind::Docker
    }

    fn launch_agent(&self, _ctx: &AgentLaunchCtx, _agent_bin: &str) -> Result<LaunchPlan> {
        Err(Error::Other(
            "Docker engine not implemented yet — switch the sandbox engine back to sandbox-exec in Settings › General".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn availability_serializes_to_the_wire_shape() {
        let available = DockerAvailability::Available {
            version: "27.3.1".into(),
        };
        assert_eq!(
            serde_json::to_value(available).unwrap(),
            serde_json::json!({ "status": "available", "version": "27.3.1" })
        );
        assert_eq!(
            serde_json::to_value(DockerAvailability::NotInstalled).unwrap(),
            serde_json::json!({ "status": "not-installed" })
        );
        assert_eq!(
            serde_json::to_value(DockerAvailability::DaemonDown).unwrap(),
            serde_json::json!({ "status": "daemon-down" })
        );
    }

    #[test]
    fn stub_engine_refuses_to_launch() {
        let ctx = AgentLaunchCtx {
            agent_id: "test-agent",
            writable_root: Path::new("/tmp/agent"),
            rpc_dir: Path::new("/tmp/rpc"),
            cwd: Path::new("/tmp/agent/repo"),
            home: Path::new("/tmp/home"),
            interactive: false,
        };
        // `match` rather than `expect_err`: `LaunchPlan` isn't `Debug`.
        let err = match DockerEngineStub.launch_agent(&ctx, "claude") {
            Ok(_) => panic!("stub must not launch"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("not implemented yet"));
    }
}
