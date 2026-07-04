//! Container labels and the dead-instance orphan sweep.
//!
//! Invariant 4 of the sandbox plan: no orphaned containers. Every container
//! Fletch launches carries `fletch.host-pid=<pid>` (which app instance owns
//! it) and `fletch.agent-id=<id>` (which agent it runs). If the app dies
//! without cleanup — crash, force-quit, SIGKILL — its containers keep
//! running; the next startup sweeps them by the same pid-liveness rule the
//! nested-root sweeps use (`sandbox/seatbelt.rs`): remove only containers
//! whose owning pid is gone, never a live side-by-side instance's.

use std::time::Duration;

use crate::error::{Error, Result};

use super::cli;

/// Label carrying the owning Fletch instance's pid.
pub const HOST_PID_LABEL: &str = "fletch.host-pid";

/// Label carrying the agent id a container runs (attribution/debugging; the
/// sweep keys on [`HOST_PID_LABEL`] alone).
#[allow(dead_code)] // slice B2 stamps it on `docker run`
pub const AGENT_ID_LABEL: &str = "fletch.agent-id";

/// `fletch.host-pid=<our pid>` — the value B2 passes to `docker run --label`.
#[allow(dead_code)] // slice B2 is the consumer
pub fn host_pid_label() -> String {
    format!("{HOST_PID_LABEL}={}", std::process::id())
}

/// `fletch.agent-id=<agent_id>` — sibling of [`host_pid_label`] for B2's argv.
#[allow(dead_code)] // slice B2 is the consumer
pub fn agent_id_label(agent_id: &str) -> String {
    format!("{AGENT_ID_LABEL}={agent_id}")
}

/// Listing/inspect are metadata-only; generous next to their usual
/// milliseconds, so tripping one means the daemon is wedged.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// `docker rm -f` also kills a still-running container's process; give the
/// batched removal room without letting a hung daemon pin the sweep thread.
const REMOVE_TIMEOUT: Duration = Duration::from_secs(60);

/// `docker inspect` line format pairing each container id with its owning
/// pid. `index` (rather than a direct field access) yields an empty string
/// when the label is somehow absent, which parses to "no pid" and is skipped
/// — under-reclaiming, never removing something we can't attribute.
const INSPECT_FORMAT: &str = r#"{{.Id}} {{index .Config.Labels "fletch.host-pid"}}"#;

/// Remove every fletch-labeled container whose owning host instance is dead.
/// Returns the number removed. Callers gate on the probe and run this off
/// the main thread — see `sweep_orphans_at_startup` in `docker/mod.rs`.
pub fn sweep_orphans() -> Result<usize> {
    let list = cli::run_docker(
        &["ps", "-aq", "--filter", &format!("label={HOST_PID_LABEL}")],
        QUERY_TIMEOUT,
    )?;
    if !list.status.success() {
        return Err(Error::Other(format!(
            "docker ps failed: {}",
            String::from_utf8_lossy(&list.stderr).trim(),
        )));
    }
    let ids: Vec<&str> = std::str::from_utf8(&list.stdout)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if ids.is_empty() {
        return Ok(0);
    }

    let mut inspect_args = vec!["inspect", "-f", INSPECT_FORMAT];
    inspect_args.extend(&ids);
    let inspected = cli::run_docker(&inspect_args, QUERY_TIMEOUT)?;
    // Don't require a zero exit: inspect exits non-zero if ANY id vanished
    // between ps and inspect (e.g. a `--rm` container finishing), but still
    // prints the rows it found — those are the ones we act on.
    let stdout = String::from_utf8_lossy(&inspected.stdout);
    let orphans = orphaned_ids(&stdout, crate::sandbox::seatbelt::pid_alive);
    if orphans.is_empty() {
        return Ok(0);
    }

    tracing::info!(
        count = orphans.len(),
        "removing containers of dead fletch instances"
    );
    let mut rm_args = vec!["rm", "-f"];
    rm_args.extend(orphans.iter().map(String::as_str));
    let removed = cli::run_docker(&rm_args, REMOVE_TIMEOUT)?;
    if !removed.status.success() {
        return Err(Error::Other(format!(
            "docker rm failed: {}",
            String::from_utf8_lossy(&removed.stderr).trim(),
        )));
    }
    Ok(orphans.len())
}

/// Parse [`INSPECT_FORMAT`] output and keep the ids whose owning pid is
/// provably dead. A missing or unparsable pid means we can't attribute the
/// container, so it is left alone (same under-reclaim bias as
/// `cleanup_nested_state_roots_in`). Pure — the liveness probe is injected
/// for unit tests.
fn orphaned_ids(inspect_stdout: &str, alive: impl Fn(i32) -> bool) -> Vec<String> {
    inspect_stdout
        .lines()
        .filter_map(parse_inspect_line)
        .filter(|(_, pid)| pid.is_some_and(|p| !alive(p)))
        .map(|(id, _)| id)
        .collect()
}

/// One [`INSPECT_FORMAT`] line → `(container_id, owning_pid)`. The pid is
/// `None` when the label was empty or not a number.
fn parse_inspect_line(line: &str) -> Option<(String, Option<i32>)> {
    let mut parts = line.split_whitespace();
    let id = parts.next()?;
    let pid = parts.next().and_then(|p| p.parse::<i32>().ok());
    Some((id.to_string(), pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_argv_shapes() {
        assert_eq!(
            host_pid_label(),
            format!("fletch.host-pid={}", std::process::id()),
        );
        assert_eq!(agent_id_label("agent-42"), "fletch.agent-id=agent-42");
    }

    #[test]
    fn inspect_line_parsing() {
        assert_eq!(
            parse_inspect_line("abc123 4242"),
            Some(("abc123".into(), Some(4242))),
        );
        // Missing label → `index` printed an empty string → no pid.
        assert_eq!(parse_inspect_line("abc123 "), Some(("abc123".into(), None)));
        assert_eq!(parse_inspect_line("abc123"), Some(("abc123".into(), None)));
        // Garbage pid → no pid, not a parse crash.
        assert_eq!(
            parse_inspect_line("abc123 not-a-pid"),
            Some(("abc123".into(), None)),
        );
        assert_eq!(parse_inspect_line(""), None);
    }

    /// The sweep's core rule: dead pid → remove; live pid → keep; and any
    /// container we can't attribute to a pid is kept (under-reclaim bias).
    #[test]
    fn selects_only_provably_dead_owners() {
        let stdout = "aaa 100\nbbb 200\nccc \nddd bogus\n";
        let orphans = orphaned_ids(stdout, |pid| pid == 100);
        assert_eq!(orphans, vec!["bbb".to_string()]);
    }

    /// Integration: a container labeled with a dead pid is swept; one labeled
    /// with our own (live) pid survives.
    /// `FLETCH_DOCKER_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Docker; opt in via FLETCH_DOCKER_TESTS=1"]
    fn sweeps_dead_instance_containers_only() {
        if !crate::sandbox::docker::docker_tests_enabled() {
            return;
        }
        let run = |label: &str, name: &str| {
            let out = cli::run_docker(
                &[
                    "run", "-d", "--label", label, "--name", name, "busybox", "sleep", "60",
                ],
                Duration::from_secs(60),
            )
            .unwrap();
            assert!(
                out.status.success(),
                "docker run failed: {}",
                String::from_utf8_lossy(&out.stderr),
            );
        };
        // 99999998 exceeds macOS's pid range and can't be a live process.
        let dead_name = format!("fletch-test-dead-{}", std::process::id());
        let live_name = format!("fletch-test-live-{}", std::process::id());
        run(&format!("{HOST_PID_LABEL}=99999998"), &dead_name);
        run(&host_pid_label(), &live_name);

        let removed = sweep_orphans().unwrap();
        assert!(removed >= 1, "the dead-pid container should be swept");

        let exists = |name: &str| {
            let out = cli::run_docker(
                &["ps", "-aq", "--filter", &format!("name={name}")],
                Duration::from_secs(10),
            )
            .unwrap();
            !String::from_utf8_lossy(&out.stdout).trim().is_empty()
        };
        assert!(!exists(&dead_name), "dead-instance container must be gone");
        assert!(exists(&live_name), "live-instance container must survive");

        let _ = cli::run_docker(&["rm", "-f", &live_name], Duration::from_secs(30));
    }
}
