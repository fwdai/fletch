//! Podman availability probe, cached for UI polling.
//!
//! Same contract as [`docker::probe`](crate::sandbox::docker::probe): the
//! settings pane polls this and spawn paths gate on it, so it must be cheap to
//! call repeatedly and must never hang.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::cli;

/// How long a probe result stays fresh, against ~1s UI polling.
const CACHE_TTL: Duration = Duration::from_secs(5);

/// Hard cap on the `podman info` round-trip — a running machine answers in
/// milliseconds, so anything slower is indistinguishable from down.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The three states the UI distinguishes: usable now, fixable by starting the
/// `podman machine`, or fixable only by installing Podman.
/// [`MachineDown`](Self::MachineDown) also covers a Linux install whose user
/// socket is unreachable — one remedy the user can act on either way.
///
/// Serializes to the wire shape the settings UI consumes:
/// `{ "status": "...", "version"?: "..." }`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum PodmanAvailability {
    Available {
        #[serde(rename = "version")]
        server_version: String,
    },
    NotInstalled,
    MachineDown,
}

/// Current Podman availability, at most [`CACHE_TTL`] stale.
///
/// The cache lock is held across the probe deliberately: concurrent callers
/// would otherwise each burn their own timeout on a down machine.
pub fn availability() -> PodmanAvailability {
    static CACHE: Mutex<Option<(Instant, PodmanAvailability)>> = Mutex::new(None);
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

fn probe() -> PodmanAvailability {
    if cli::podman_bin().is_none() {
        return PodmanAvailability::NotInstalled;
    }
    // `info`, not `version`: `version` reports the client build from the binary
    // alone and answers happily with the machine stopped.
    match cli::run_podman(&["info", "--format", "{{.Version.Version}}"], PROBE_TIMEOUT) {
        Ok(out) if out.status.success() => {
            classify_version_stdout(&String::from_utf8_lossy(&out.stdout))
        }
        _ => PodmanAvailability::MachineDown,
    }
}

/// Map a successful `podman info` stdout to availability.
fn classify_version_stdout(stdout: &str) -> PodmanAvailability {
    let version = stdout.trim();
    if version.is_empty() {
        // Zero exit but no version: down, rather than an "unknown" state the UI
        // would have to render.
        PodmanAvailability::MachineDown
    } else {
        PodmanAvailability::Available {
            server_version: version.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_serializes_to_the_wire_shape() {
        let available = PodmanAvailability::Available {
            server_version: "5.3.1".into(),
        };
        assert_eq!(
            serde_json::to_value(available).unwrap(),
            serde_json::json!({ "status": "available", "version": "5.3.1" })
        );
        assert_eq!(
            serde_json::to_value(PodmanAvailability::NotInstalled).unwrap(),
            serde_json::json!({ "status": "not-installed" })
        );
        assert_eq!(
            serde_json::to_value(PodmanAvailability::MachineDown).unwrap(),
            serde_json::json!({ "status": "machine-down" })
        );
    }

    #[test]
    fn version_stdout_classification() {
        assert_eq!(
            classify_version_stdout("5.3.1\n"),
            PodmanAvailability::Available {
                server_version: "5.3.1".into()
            },
        );
        assert_eq!(
            classify_version_stdout("  \n"),
            PodmanAvailability::MachineDown,
            "a zero exit without a version is not a usable machine",
        );
    }
}
