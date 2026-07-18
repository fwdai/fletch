//! Container naming, liveness lookups, and the docker-reserved exit-code
//! messages — the small provider-neutral helpers the launch/teardown paths lean
//! on.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::sandbox::docker::cli;

/// Liveness lookups (`docker inspect`).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(5);

/// `Some(v)` only when `v` is present and non-blank — settings rows can hold
/// empty strings, which must fall back to defaults.
pub(super) fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// `fletch-<agent_id>-<8-char nonce>`. The nonce keeps respawns (view switch,
/// binary swap) from colliding with a predecessor container of the same agent
/// that `--rm` hasn't finished reaping yet; hashing in the pid keeps two
/// side-by-side Fletch instances apart even for a same-named agent.
pub(super) fn container_name(agent_id: &str) -> String {
    // Docker names must match [a-zA-Z0-9][a-zA-Z0-9_.-]*; the `fletch-`
    // prefix fixes the first char, sanitize the rest.
    let sanitized: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("fletch-{sanitized}-{}", nonce())
}

/// 8 hex chars from (pid, monotonic counter): unique within a host across
/// concurrently running instances for the lifetime of any one container.
fn nonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::process::id().hash(&mut hasher);
    COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    let hex: String = format!("{:016x}", hasher.finish());
    hex[..8].to_string()
}

/// Whether the daemon says the container is currently running. Errors
/// (container gone, daemon down, timeout) read as not running.
pub(super) fn container_running(name: &str) -> bool {
    match cli::run_docker(
        &["inspect", "-f", "{{.State.Running}}", name],
        INSPECT_TIMEOUT,
    ) {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(e) => {
            tracing::debug!(container = %name, error = %e, "docker inspect failed; treating as dead");
            false
        }
    }
}

/// Poll until the container stops running or `budget` elapses.
pub(super) fn container_gone_within(name: &str, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if !container_running(name) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// User-readable meanings for the docker CLI's reserved exit codes; other
/// codes are the contained agent's own and pass through unmapped. `docker run`
/// relays the agent's own exit status, so an agent that starts fine and later
/// exits 125/126/127 is indistinguishable from a launcher/image failure — the
/// messages name the likely Docker-layer cause but flag the agent-exit
/// possibility so they don't mislead when the container did launch.
///
/// Provider-neutral: the teardown plan carries only the container name, and the
/// sandbox image now varies per provider, so these speak of "the agent binary"
/// rather than naming `claude`.
pub(super) fn describe_exit_code(code: i32) -> Option<String> {
    let msg = match code {
        125 => "Exit 125: Docker could not start the sandbox container — the daemon reported an error (or the agent itself exited 125). Is Docker Desktop still running?",
        126 => "Exit 126: the agent binary in the sandbox image is present but not runnable (or the agent itself exited 126). If you set a custom docker_image, check its agent CLI.",
        127 => "Exit 127: no agent binary on the sandbox image's PATH (or the agent itself exited 127). A custom docker_image must include the launching agent's CLI.",
        _ => return None,
    };
    Some(msg.to_string())
}
