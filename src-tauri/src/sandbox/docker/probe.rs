//! Daemon availability probe, cached for UI polling.
//!
//! The settings pane (slice C1) polls this to enable/disable the Docker
//! engine option, and spawn paths (B2) gate on it — so it must be cheap to
//! call repeatedly and must never hang: the underlying `docker version` call
//! is bounded at 2s, and results are cached for 5s so a polling UI costs at
//! most one daemon round-trip per window.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::cli;

/// How long a probe result stays fresh. UI polling is expected at ~1s
/// intervals; 5s keeps the daemon traffic negligible while still flipping
/// the UI within a beat of Docker Desktop starting or stopping.
const CACHE_TTL: Duration = Duration::from_secs(5);

/// Hard cap on the `docker version` round-trip. A healthy daemon answers in
/// milliseconds; anything slower is indistinguishable from down for our
/// purposes, and 2s keeps a first uncached call from stalling its caller.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The three states the UI distinguishes: usable now, fixable by starting
/// Docker Desktop, or fixable only by installing it. Serializes to the wire
/// shape the settings UI consumes: `{ "status": "...", "version"?: "..." }`
/// (the `probe_docker_engine` command in `lib.rs` returns it directly).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum DockerAvailability {
    Available {
        #[serde(rename = "version")]
        server_version: String,
    },
    NotInstalled,
    DaemonDown,
}

/// Current Docker availability, at most [`CACHE_TTL`] stale.
///
/// The cache lock is held across the probe itself — deliberate: concurrent
/// callers during a daemon outage would otherwise each burn their own 2s
/// timeout, and serializing them means followers get the fresh cached answer
/// immediately.
pub fn availability() -> DockerAvailability {
    static CACHE: Mutex<Option<(Instant, DockerAvailability)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if let Some((at, cached)) = cache.as_ref() {
        if at.elapsed() < CACHE_TTL {
            return cached.clone();
        }
    }
    let fresh = probe();
    *cache = Some((Instant::now(), fresh.clone()));
    fresh
}

/// One uncached probe: binary present? daemon answering?
fn probe() -> DockerAvailability {
    if cli::docker_bin().is_none() {
        return DockerAvailability::NotInstalled;
    }
    // `docker version --format {{.Server.Version}}` exits non-zero (and prints
    // to stderr) when the daemon is unreachable; a timeout means a socket that
    // accepts but never answers — same user remedy, same classification.
    match cli::run_docker(
        &["version", "--format", "{{.Server.Version}}"],
        PROBE_TIMEOUT,
    ) {
        Ok(out) if out.status.success() => {
            classify_version_stdout(&String::from_utf8_lossy(&out.stdout))
        }
        _ => DockerAvailability::DaemonDown,
    }
}

/// Map a successful `docker version` stdout to availability. Split out of
/// [`probe`] so the parsing is unit-testable without a daemon.
fn classify_version_stdout(stdout: &str) -> DockerAvailability {
    let version = stdout.trim();
    if version.is_empty() {
        // Zero exit but no server version — treat as down rather than
        // inventing an "unknown" state the UI would have to render.
        DockerAvailability::DaemonDown
    } else {
        DockerAvailability::Available {
            server_version: version.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_serializes_to_the_wire_shape() {
        let available = DockerAvailability::Available {
            server_version: "27.3.1".into(),
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
    fn version_stdout_classification() {
        assert_eq!(
            classify_version_stdout("28.1.1\n"),
            DockerAvailability::Available {
                server_version: "28.1.1".into()
            },
        );
        assert_eq!(
            classify_version_stdout("  \n"),
            DockerAvailability::DaemonDown,
            "a zero exit without a server version is not a usable daemon",
        );
    }

    /// Integration: needs Docker Desktop running.
    /// `FLETCH_DOCKER_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Docker; opt in via FLETCH_DOCKER_TESTS=1"]
    fn probe_reports_running_daemon() {
        if !crate::sandbox::docker::docker_tests_enabled() {
            return;
        }
        match availability() {
            DockerAvailability::Available { server_version } => {
                assert!(!server_version.is_empty());
            }
            other => panic!("expected a running daemon, probe said {other:?}"),
        }
    }
}
